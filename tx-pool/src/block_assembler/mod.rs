//! Generate a new block

mod candidate_uncles;
mod process;

#[cfg(test)]
mod tests;

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError;
pub use candidate_uncles::CandidateUncles;
use ckb_app_config::BlockAssemblerConfig;
use ckb_dao::DaoCalculator;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_jsonrpc_types::{
    BlockTemplate as JsonBlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate,
};
use ckb_logger::{debug, error, trace};
use ckb_reward_calculator::RewardCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    bytes,
    core::{
        BlockNumber, Capacity, Cycle, EpochExt, EpochNumberWithFraction, ScriptHashType,
        TransactionBuilder, TransactionView, UncleBlockView, Version,
        cell::{OverlayCellChecker, TransactionsChecker},
    },
    packed::{
        self, Byte32, Bytes, CellInput, CellOutput, CellbaseWitness, ProposalShortId, Script,
        Transaction,
    },
    prelude::*,
};
use http_body_util::Full;
use hyper::{Method, Request};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use std::{cmp, iter};
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};
use tokio::task::block_in_place;
use tokio::time::timeout;

use crate::TxPool;
pub(crate) use process::process;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct TemplateSize {
    pub(crate) txs: usize,
    pub(crate) proposals: usize,
    pub(crate) uncles: usize,
    pub(crate) total: usize,
}

impl TemplateSize {
    pub(crate) fn calc_total_by_proposals(&self, new_proposals_size: usize) -> usize {
        if new_proposals_size > self.proposals {
            self.total
                .saturating_add(new_proposals_size - self.proposals)
        } else {
            self.total
                .saturating_sub(self.proposals - new_proposals_size)
        }
    }

    pub(crate) fn calc_total_by_uncles(&self, new_uncles_size: usize) -> usize {
        if new_uncles_size > self.uncles {
            self.total.saturating_add(new_uncles_size - self.uncles)
        } else {
            self.total.saturating_sub(self.uncles - new_uncles_size)
        }
    }

    pub(crate) fn calc_total_by_txs(&self, new_txs_size: usize) -> usize {
        if new_txs_size > self.txs {
            self.total.saturating_add(new_txs_size - self.txs)
        } else {
            self.total.saturating_sub(self.txs - new_txs_size)
        }
    }
}

#[derive(Clone)]
pub(crate) struct CurrentTemplate {
    pub(crate) template: BlockTemplate,
    pub(crate) size: TemplateSize,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) epoch: EpochExt,
}

/// Block generator
#[derive(Clone)]
pub struct BlockAssembler {
    pub(crate) config: Arc<BlockAssemblerConfig>,
    pub(crate) work_id: Arc<AtomicU64>,
    pub(crate) candidate_uncles: Arc<Mutex<CandidateUncles>>,
    pub(crate) current: Arc<Mutex<CurrentTemplate>>,
    pub(crate) poster: Arc<Client<HttpConnector, Full<bytes::Bytes>>>,
}

impl BlockAssembler {
    /// Construct new block generator
    pub fn new(config: BlockAssemblerConfig, snapshot: Arc<Snapshot>) -> Self {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let current_epoch = consensus
            .next_epoch_ext(tip_header, &snapshot.borrow_as_data_loader())
            .expect("tip header's epoch should be stored")
            .epoch();
        let mut builder = BlockTemplateBuilder::new(&snapshot, &current_epoch);

        let cellbase = Self::build_cellbase(&config, &snapshot)
            .expect("build cellbase for BlockAssembler initial");

        let extension =
            Self::build_extension(&snapshot).expect("build extension for BlockAssembler initial");
        let basic_block_size =
            Self::basic_block_size(cellbase.data(), &[], iter::empty(), extension.clone());

        let (dao, _checked_txs, _failed_txs) =
            Self::calc_dao(&snapshot, &current_epoch, cellbase.clone(), vec![])
                .expect("calc_dao for BlockAssembler initial");

        let work_id = AtomicU64::new(0);

        builder
            .transactions(vec![])
            .proposals(vec![])
            .cellbase(cellbase)
            .work_id(work_id.fetch_add(1, Ordering::SeqCst))
            .current_time(cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1))
            .dao(dao);
        if let Some(data) = extension {
            builder.extension(data);
        }
        let template = builder.build();

        let size = TemplateSize {
            txs: 0,
            proposals: 0,
            uncles: 0,
            total: basic_block_size,
        };

        let current = CurrentTemplate {
            template,
            size,
            snapshot,
            epoch: current_epoch,
        };

        Self {
            config: Arc::new(config),
            work_id: Arc::new(work_id),
            candidate_uncles: Arc::new(Mutex::new(CandidateUncles::new())),
            current: Arc::new(Mutex::new(current)),
            poster: Arc::new(
                Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build::<_, Full<bytes::Bytes>>(HttpConnector::new()),
            ),
        }
    }

    pub(crate) async fn update_full(&self, tx_pool: &RwLock<TxPool>) -> Result<(), AnyError> {
        let mut current = self.current.lock().await;
        let consensus = current.snapshot.consensus();
        let max_block_bytes = consensus.max_block_bytes() as usize;

        let current_template = &current.template;
        let uncles = &current_template.uncles;

        let (proposals, txs, basic_size) = {
            let tx_pool_reader = tx_pool.read().await;
            if current.snapshot.tip_hash() != tx_pool_reader.snapshot().tip_hash() {
                return Ok(());
            }
            let proposals =
                tx_pool_reader.package_proposals(consensus.max_block_proposals_limit(), uncles);

            let basic_size = Self::basic_block_size(
                current_template.cellbase.data(),
                uncles,
                proposals.iter(),
                current_template.extension.clone(),
            );

            let txs_size_limit = max_block_bytes
                .checked_sub(basic_size)
                .ok_or(BlockAssemblerError::Overflow)?;

            let max_block_cycles = consensus.max_block_cycles();
            let (txs, _txs_size, _cycles) =
                tx_pool_reader.package_txs(max_block_cycles, txs_size_limit);
            (proposals, txs, basic_size)
        };

        let proposals_size = proposals.len() * ProposalShortId::serialized_size();
        let (dao, checked_txs, failed_txs) = Self::calc_dao(
            &current.snapshot,
            &current.epoch,
            current_template.cellbase.clone(),
            txs,
        )?;
        if !failed_txs.is_empty() {
            let mut tx_pool_writer = tx_pool.write().await;
            for id in failed_txs {
                tx_pool_writer.remove_tx(&id);
            }
        }

        let txs_size = checked_txs.iter().map(|tx| tx.size).sum();
        let total_size = basic_size + txs_size;

        let mut builder = BlockTemplateBuilder::from_template(&current.template);
        builder
            .set_proposals(Vec::from_iter(proposals))
            .set_transactions(checked_txs)
            .work_id(self.work_id.fetch_add(1, Ordering::SeqCst))
            .current_time(cmp::max(
                unix_time_as_millis(),
                current.template.current_time,
            ))
            .dao(dao);

        current.template = builder.build();
        current.size.txs = txs_size;
        current.size.total = total_size;
        current.size.proposals = proposals_size;

        trace!(
            "[BlockAssembler] update_full {} uncles-{} proposals-{} txs-{}",
            current.template.number,
            current.template.uncles.len(),
            current.template.proposals.len(),
            current.template.transactions.len(),
        );

        Ok(())
    }

    pub(crate) async fn update_blank(&self, snapshot: Arc<Snapshot>) -> Result<(), AnyError> {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let current_epoch = consensus
            .next_epoch_ext(tip_header, &snapshot.borrow_as_data_loader())
            .expect("tip header's epoch should be stored")
            .epoch();
        let mut builder = BlockTemplateBuilder::new(&snapshot, &current_epoch);

        let cellbase = Self::build_cellbase(&self.config, &snapshot)?;
        let uncles = self.prepare_uncles(&snapshot, &current_epoch).await;
        let uncles_size = uncles.len() * UncleBlockView::serialized_size_in_block();

        let extension = Self::build_extension(&snapshot)?;
        let basic_block_size =
            Self::basic_block_size(cellbase.data(), &uncles, iter::empty(), extension.clone());

        let (dao, _checked_txs, _failed_txs) =
            Self::calc_dao(&snapshot, &current_epoch, cellbase.clone(), vec![])?;

        builder
            .transactions(vec![])
            .proposals(vec![])
            .cellbase(cellbase)
            .uncles(uncles)
            .work_id(self.work_id.fetch_add(1, Ordering::SeqCst))
            .current_time(cmp::max(unix_time_as_millis(), tip_header.timestamp() + 1))
            .dao(dao);
        if let Some(data) = extension {
            builder.extension(data);
        }
        let template = builder.build();

        trace!(
            "[BlockAssembler] update_blank {} uncles-{} proposals-{} txs-{}",
            template.number,
            template.uncles.len(),
            template.proposals.len(),
            template.transactions.len(),
        );

        let size = TemplateSize {
            txs: 0,
            proposals: 0,
            uncles: uncles_size,
            total: basic_block_size,
        };

        let new_blank = CurrentTemplate {
            template,
            size,
            snapshot,
            epoch: current_epoch,
        };

        *self.current.lock().await = new_blank;
        Ok(())
    }

    pub(crate) async fn update_uncles(&self) {
        let mut current = self.current.lock().await;
        let consensus = current.snapshot.consensus();
        let max_block_bytes = consensus.max_block_bytes() as usize;
        let max_uncles_num = consensus.max_uncles_num();
        let current_uncles_num = current.template.uncles.len();
        if current_uncles_num < max_uncles_num {
            let remain_size = max_block_bytes.saturating_sub(current.size.total);

            if remain_size > UncleBlockView::serialized_size_in_block() {
                let uncles = self.prepare_uncles(&current.snapshot, &current.epoch).await;

                let new_uncle_size = uncles.len() * UncleBlockView::serialized_size_in_block();
                let new_total_size = current.size.calc_total_by_uncles(new_uncle_size);

                if new_total_size < max_block_bytes {
                    let mut builder = BlockTemplateBuilder::from_template(&current.template);
                    builder
                        .set_uncles(uncles)
                        .work_id(self.work_id.fetch_add(1, Ordering::SeqCst))
                        .current_time(cmp::max(
                            unix_time_as_millis(),
                            current.template.current_time,
                        ));
                    current.template = builder.build();
                    current.size.uncles = new_uncle_size;
                    current.size.total = new_total_size;

                    trace!(
                        "[BlockAssembler] update_uncles-{} epoch-{} uncles-{} proposals-{} txs-{}",
                        current.template.number,
                        current.template.epoch.number(),
                        current.template.uncles.len(),
                        current.template.proposals.len(),
                        current.template.transactions.len(),
                    );
                }
            }
        }
    }

    pub(crate) async fn update_proposals(&self, tx_pool: &RwLock<TxPool>) {
        let mut current = self.current.lock().await;
        let consensus = current.snapshot.consensus();
        let uncles = &current.template.uncles;
        let proposals = {
            let tx_pool_reader = tx_pool.read().await;
            if current.snapshot.tip_hash() != tx_pool_reader.snapshot().tip_hash() {
                return;
            }
            tx_pool_reader.package_proposals(consensus.max_block_proposals_limit(), uncles)
        };

        let new_proposals_size = proposals.len() * ProposalShortId::serialized_size();
        let new_total_size = current.size.calc_total_by_proposals(new_proposals_size);
        let max_block_bytes = consensus.max_block_bytes() as usize;
        if new_total_size < max_block_bytes {
            let mut builder = BlockTemplateBuilder::from_template(&current.template);
            builder
                .set_proposals(Vec::from_iter(proposals))
                .work_id(self.work_id.fetch_add(1, Ordering::SeqCst))
                .current_time(cmp::max(
                    unix_time_as_millis(),
                    current.template.current_time,
                ));
            current.template = builder.build();
            current.size.proposals = new_proposals_size;
            current.size.total = new_total_size;

            trace!(
                "[BlockAssembler] update_proposals-{} epoch-{} uncles-{} proposals-{} txs-{}",
                current.template.number,
                current.template.epoch.number(),
                current.template.uncles.len(),
                current.template.proposals.len(),
                current.template.transactions.len(),
            );
        }
    }

    pub(crate) async fn update_transactions(
        &self,
        tx_pool: &RwLock<TxPool>,
    ) -> Result<(), AnyError> {
        let mut current = self.current.lock().await;
        let consensus = current.snapshot.consensus();
        let current_template = &current.template;
        let max_block_bytes = consensus.max_block_bytes() as usize;
        let extension = Self::build_extension(&current.snapshot)?;
        let txs = {
            let tx_pool_reader = tx_pool.read().await;
            if current.snapshot.tip_hash() != tx_pool_reader.snapshot().tip_hash() {
                return Ok(());
            }

            let basic_block_size = Self::basic_block_size(
                current_template.cellbase.data(),
                &current_template.uncles,
                current_template.proposals.iter(),
                extension.clone(),
            );

            let txs_size_limit = max_block_bytes.checked_sub(basic_block_size);

            if txs_size_limit.is_none() {
                return Ok(());
            }

            let max_block_cycles = consensus.max_block_cycles();
            let (txs, _txs_size, _cycles) = tx_pool_reader
                .package_txs(max_block_cycles, txs_size_limit.expect("overflow checked"));
            txs
        };

        if let Ok((dao, checked_txs, _failed_txs)) = Self::calc_dao(
            &current.snapshot,
            &current.epoch,
            current_template.cellbase.clone(),
            txs,
        ) {
            let new_txs_size = checked_txs.iter().map(|tx| tx.size).sum();
            let new_total_size = current.size.calc_total_by_txs(new_txs_size);
            let mut builder = BlockTemplateBuilder::from_template(&current.template);
            builder
                .set_transactions(checked_txs)
                .work_id(self.work_id.fetch_add(1, Ordering::SeqCst))
                .current_time(cmp::max(
                    unix_time_as_millis(),
                    current.template.current_time,
                ))
                .dao(dao);
            if let Some(data) = extension {
                builder.extension(data);
            }
            current.template = builder.build();
            current.size.txs = new_txs_size;
            current.size.total = new_total_size;

            trace!(
                "[BlockAssembler] update_transactions-{} epoch-{} uncles-{} proposals-{} txs-{}",
                current.template.number,
                current.template.epoch.number(),
                current.template.uncles.len(),
                current.template.proposals.len(),
                current.template.transactions.len(),
            );
        }
        Ok(())
    }

    pub(crate) async fn get_current(&self) -> JsonBlockTemplate {
        let current = self.current.lock().await;
        (&current.template).into()
    }

    pub(crate) fn build_cellbase_witness(
        config: &BlockAssemblerConfig,
        snapshot: &Snapshot,
    ) -> CellbaseWitness {
        let hash_type: ScriptHashType = config.hash_type.clone().into();
        let cellbase_lock = Script::new_builder()
            .args(config.args.as_bytes().pack())
            .code_hash(config.code_hash.pack())
            .hash_type(hash_type.into())
            .build();
        let tip = snapshot.tip_header();

        let mut message = vec![];
        if let Some(version) = snapshot.compute_versionbits(tip) {
            message.extend_from_slice(&version.to_le_bytes());
            message.extend_from_slice(b" ");
        }
        if config.use_binary_version_as_message_prefix {
            message.extend_from_slice(config.binary_version.as_bytes());
        }
        if !config.message.is_empty() {
            message.extend_from_slice(b" ");
            message.extend_from_slice(config.message.as_bytes());
        }

        CellbaseWitness::new_builder()
            .lock(cellbase_lock)
            .message(message.pack())
            .build()
    }

    /// Miner mined block H(c), the block reward will be finalized at H(c + w_far + 1).
    /// Miner specify own lock in cellbase witness.
    /// The cellbase have only one output,
    /// miner should collect the block reward for finalize target H(max(0, c - w_far - 1))
    pub(crate) fn build_cellbase(
        config: &BlockAssemblerConfig,
        snapshot: &Snapshot,
    ) -> Result<TransactionView, AnyError> {
        let tip = snapshot.tip_header();
        let candidate_number = tip.number() + 1;
        let cellbase_witness = Self::build_cellbase_witness(config, snapshot);

        let tx = {
            let (target_lock, block_reward) = block_in_place(|| {
                RewardCalculator::new(snapshot.consensus(), snapshot).block_reward_to_finalize(tip)
            })?;
            let input = CellInput::new_cellbase_input(candidate_number);
            let output = CellOutput::new_builder()
                .capacity(block_reward.total.pack())
                .lock(target_lock)
                .build();

            let witness = cellbase_witness.as_bytes().pack();
            let no_finalization_target =
                candidate_number <= snapshot.consensus().finalization_delay_length();
            let tx_builder = TransactionBuilder::default().input(input).witness(witness);
            let insufficient_reward_to_create_cell = output.is_lack_of_capacity(Capacity::zero())?;
            if no_finalization_target || insufficient_reward_to_create_cell {
                tx_builder.build()
            } else {
                tx_builder
                    .output(output)
                    .output_data(Bytes::default())
                    .build()
            }
        };

        Ok(tx)
    }

    pub(crate) fn build_extension(snapshot: &Snapshot) -> Result<Option<packed::Bytes>, AnyError> {
        let tip_header = snapshot.tip_header();
        // The use of the epoch number of the tip here leads to an off-by-one bug,
        // so be careful, it needs to be preserved for consistency reasons and not fixed directly.
        let mmr_activate = snapshot
            .consensus()
            .rfc0044_active(tip_header.epoch().number());
        if mmr_activate {
            let chain_root = snapshot
                .chain_root_mmr(tip_header.number())
                .get_root()
                .map_err(|e| InternalErrorKind::MMR.other(e))?;
            let bytes = chain_root.calc_mmr_hash().as_bytes().pack();
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn prepare_uncles(
        &self,
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
    ) -> Vec<UncleBlockView> {
        let mut guard = self.candidate_uncles.lock().await;
        guard.prepare_uncles(snapshot, current_epoch)
    }

    pub(crate) fn basic_block_size<'a>(
        cellbase: Transaction,
        uncles: &[UncleBlockView],
        proposals: impl Iterator<Item = &'a ProposalShortId>,
        extension_opt: Option<packed::Bytes>,
    ) -> usize {
        let empty_dao = packed::Byte32::default();
        let raw_header = packed::RawHeader::new_builder().dao(empty_dao).build();
        let header = packed::Header::new_builder().raw(raw_header).build();
        let block = if let Some(extension) = extension_opt {
            packed::BlockV1::new_builder()
                .header(header)
                .transactions(vec![cellbase].pack())
                .uncles(uncles.iter().map(|u| u.data()).pack())
                .proposals(proposals.cloned().collect::<Vec<_>>().pack())
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::Block::new_builder()
                .header(header)
                .transactions(vec![cellbase].pack())
                .uncles(uncles.iter().map(|u| u.data()).pack())
                .proposals(proposals.cloned().collect::<Vec<_>>().pack())
                .build()
        };
        block.serialized_size_without_uncle_proposals()
    }

    fn calc_dao(
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
        cellbase: TransactionView,
        entries: Vec<TxEntry>,
    ) -> Result<(Byte32, Vec<TxEntry>, Vec<ProposalShortId>), AnyError> {
        let tip_header = snapshot.tip_header();
        let consensus = snapshot.consensus();
        let mut seen_inputs = HashSet::new();
        let mut transactions_checker = TransactionsChecker::new(iter::once(&cellbase));

        let mut checked_failed_txs = vec![];
        let checked_entries: Vec<_> = block_in_place(|| {
            entries
                .into_iter()
                .filter_map(|entry| {
                    let overlay_cell_checker =
                        OverlayCellChecker::new(&transactions_checker, snapshot);
                    if let Err(err) =
                        entry
                            .rtx
                            .check(&mut seen_inputs, &overlay_cell_checker, snapshot)
                    {
                        error!(
                            "Resolving transactions while building block template, \
                             tip_number: {}, tip_hash: {}, tx_hash: {}, error: {:?}",
                            tip_header.number(),
                            tip_header.hash(),
                            entry.transaction().hash(),
                            err
                        );
                        checked_failed_txs.push(entry.proposal_short_id());
                        None
                    } else {
                        transactions_checker.insert(entry.transaction());
                        Some(entry)
                    }
                })
                .collect()
        });

        let dummy_cellbase_entry = TxEntry::dummy_resolve(cellbase, 0, Capacity::zero(), 0);
        let entries_iter = iter::once(&dummy_cellbase_entry)
            .chain(checked_entries.iter())
            .map(|entry| entry.rtx.as_ref());

        // Generate DAO fields here
        let dao = DaoCalculator::new(consensus, &snapshot.borrow_as_data_loader())
            .dao_field_with_current_epoch(entries_iter, tip_header, current_epoch)?;

        Ok((dao, checked_entries, checked_failed_txs))
    }

    pub(crate) async fn notify(&self) {
        if !self.need_to_notify() {
            return;
        }
        let template = self.get_current().await;
        if let Ok(template_json) = serde_json::to_string(&template) {
            let notify_timeout = Duration::from_millis(self.config.notify_timeout_millis);
            for url in &self.config.notify {
                if let Ok(req) = Request::builder()
                    .method(Method::POST)
                    .uri(url.as_ref())
                    .header("content-type", "application/json")
                    .body(Full::new(template_json.to_owned().into()))
                {
                    let client = Arc::clone(&self.poster);
                    let url = url.to_owned();
                    tokio::spawn(async move {
                        let _resp =
                            timeout(notify_timeout, client.request(req))
                                .await
                                .map_err(|_| {
                                    ckb_logger::warn!(
                                        "block assembler notifying {} timed out",
                                        url
                                    );
                                });
                    });
                }
            }

            for script in &self.config.notify_scripts {
                let script = script.to_owned();
                let template_json = template_json.to_owned();
                tokio::spawn(async move {
                    // Errors
                    // This future will return an error if the child process cannot be spawned
                    // or if there is an error while awaiting its status.

                    // On Unix platforms this method will fail with std::io::ErrorKind::WouldBlock
                    // if the system process limit is reached
                    // (which includes other applications running on the system).
                    match timeout(
                        notify_timeout,
                        Command::new(&script).arg(template_json).status(),
                    )
                    .await
                    {
                        Ok(ret) => match ret {
                            Ok(status) => debug!("the command exited with: {}", status),
                            Err(e) => error!("the script {} failed to spawn {}", script, e),
                        },
                        Err(_) => {
                            ckb_logger::warn!("block assembler notifying {} timed out", script)
                        }
                    }
                });
            }
        }
    }

    fn need_to_notify(&self) -> bool {
        !self.config.notify.is_empty() || !self.config.notify_scripts.is_empty()
    }
}

#[derive(Clone)]
pub(crate) struct BlockTemplate {
    pub(crate) version: Version,
    pub(crate) compact_target: u32,
    pub(crate) number: BlockNumber,
    pub(crate) epoch: EpochNumberWithFraction,
    pub(crate) parent_hash: Byte32,
    pub(crate) cycles_limit: Cycle,
    pub(crate) bytes_limit: u64,
    pub(crate) uncles_count_limit: u8,

    // option
    pub(crate) uncles: Vec<UncleBlockView>,
    pub(crate) transactions: Vec<TxEntry>,
    pub(crate) proposals: Vec<ProposalShortId>,
    pub(crate) cellbase: TransactionView,
    pub(crate) work_id: u64,
    pub(crate) dao: Byte32,
    pub(crate) current_time: u64,
    pub(crate) extension: Option<Bytes>,
}

impl<'a> From<&'a BlockTemplate> for JsonBlockTemplate {
    fn from(template: &'a BlockTemplate) -> JsonBlockTemplate {
        JsonBlockTemplate {
            version: template.version.into(),
            compact_target: template.compact_target.into(),
            number: template.number.into(),
            epoch: template.epoch.into(),
            parent_hash: template.parent_hash.unpack(),
            cycles_limit: template.cycles_limit.into(),
            bytes_limit: template.bytes_limit.into(),
            uncles_count_limit: u64::from(template.uncles_count_limit).into(),
            uncles: template.uncles.iter().map(uncle_to_template).collect(),
            transactions: template
                .transactions
                .iter()
                .map(tx_entry_to_template)
                .collect(),
            proposals: template.proposals.iter().map(Into::into).collect(),
            cellbase: cellbase_to_template(&template.cellbase),
            work_id: template.work_id.into(),
            dao: template.dao.clone().into(),
            current_time: template.current_time.into(),
            extension: template.extension.as_ref().map(Into::into),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BlockTemplateBuilder {
    pub(crate) version: Version,
    pub(crate) compact_target: u32,
    pub(crate) number: BlockNumber,
    pub(crate) epoch: EpochNumberWithFraction,
    pub(crate) parent_hash: Byte32,
    pub(crate) cycles_limit: Cycle,
    pub(crate) bytes_limit: u64,
    pub(crate) uncles_count_limit: u8,

    // option
    pub(crate) uncles: Vec<UncleBlockView>,
    pub(crate) transactions: Vec<TxEntry>,
    pub(crate) proposals: Vec<ProposalShortId>,
    pub(crate) cellbase: Option<TransactionView>,
    pub(crate) work_id: Option<u64>,
    pub(crate) dao: Option<Byte32>,
    pub(crate) current_time: Option<u64>,
    pub(crate) extension: Option<Bytes>,
}

impl BlockTemplateBuilder {
    pub(crate) fn new(snapshot: &Snapshot, current_epoch: &EpochExt) -> Self {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let tip_hash = tip_header.hash();
        let candidate_number = tip_header.number() + 1;

        let version = consensus.block_version();
        let max_block_bytes = consensus.max_block_bytes();
        let cycles_limit = consensus.max_block_cycles();
        let uncles_count_limit = consensus.max_uncles_num() as u8;

        Self {
            version,
            compact_target: current_epoch.compact_target(),

            number: candidate_number,
            epoch: current_epoch.number_with_fraction(candidate_number),
            parent_hash: tip_hash,
            cycles_limit,
            bytes_limit: max_block_bytes,
            uncles_count_limit,
            // option
            uncles: vec![],
            transactions: vec![],
            proposals: vec![],
            cellbase: None,
            work_id: None,
            dao: None,
            current_time: None,
            extension: None,
        }
    }

    pub(crate) fn from_template(template: &BlockTemplate) -> Self {
        Self {
            version: template.version,
            compact_target: template.compact_target,
            number: template.number,
            epoch: template.epoch,
            parent_hash: template.parent_hash.clone(),
            cycles_limit: template.cycles_limit,
            bytes_limit: template.bytes_limit,
            uncles_count_limit: template.uncles_count_limit,
            extension: template.extension.clone(),
            // option
            uncles: template.uncles.clone(),
            transactions: template.transactions.clone(),
            proposals: template.proposals.clone(),
            cellbase: Some(template.cellbase.clone()),
            work_id: None,
            dao: Some(template.dao.clone()),
            current_time: None,
        }
    }

    pub(crate) fn uncles(&mut self, uncles: impl IntoIterator<Item = UncleBlockView>) -> &mut Self {
        self.uncles.extend(uncles);
        self
    }

    pub(crate) fn set_uncles(&mut self, uncles: Vec<UncleBlockView>) -> &mut Self {
        self.uncles = uncles;
        self
    }

    pub(crate) fn transactions(
        &mut self,
        transactions: impl IntoIterator<Item = TxEntry>,
    ) -> &mut Self {
        self.transactions.extend(transactions);
        self
    }

    pub(crate) fn set_transactions(&mut self, transactions: Vec<TxEntry>) -> &mut Self {
        self.transactions = transactions;
        self
    }

    pub(crate) fn proposals(
        &mut self,
        proposals: impl IntoIterator<Item = ProposalShortId>,
    ) -> &mut Self {
        self.proposals.extend(proposals);
        self
    }

    pub(crate) fn set_proposals(&mut self, proposals: Vec<ProposalShortId>) -> &mut Self {
        self.proposals = proposals;
        self
    }

    pub(crate) fn cellbase(&mut self, cellbase: TransactionView) -> &mut Self {
        self.cellbase = Some(cellbase);
        self
    }

    pub(crate) fn work_id(&mut self, work_id: u64) -> &mut Self {
        self.work_id = Some(work_id);
        self
    }

    pub(crate) fn dao(&mut self, dao: Byte32) -> &mut Self {
        self.dao = Some(dao);
        self
    }

    pub(crate) fn current_time(&mut self, current_time: u64) -> &mut Self {
        self.current_time = Some(current_time);
        self
    }

    #[allow(dead_code)]
    pub(crate) fn extension(&mut self, extension: Bytes) -> &mut Self {
        self.extension = Some(extension);
        self
    }

    pub(crate) fn build(self) -> BlockTemplate {
        assert!(self.cellbase.is_some(), "cellbase must be set");
        assert!(self.work_id.is_some(), "work_id must be set");
        assert!(self.current_time.is_some(), "current_time must be set");
        assert!(self.dao.is_some(), "dao must be set");

        BlockTemplate {
            version: self.version,
            compact_target: self.compact_target,

            number: self.number,
            epoch: self.epoch,
            parent_hash: self.parent_hash,
            cycles_limit: self.cycles_limit,
            bytes_limit: self.bytes_limit,
            uncles_count_limit: self.uncles_count_limit,
            uncles: self.uncles,
            transactions: self.transactions,
            proposals: self.proposals,
            cellbase: self.cellbase.expect("cellbase assert checked"),
            work_id: self.work_id.expect("work_id assert checked"),
            dao: self.dao.expect("dao assert checked"),
            current_time: self.current_time.expect("current_time assert checked"),
            extension: self.extension,
        }
    }
}

pub(crate) fn uncle_to_template(uncle: &UncleBlockView) -> UncleTemplate {
    UncleTemplate {
        hash: uncle.hash().unpack(),
        required: false,
        proposals: uncle
            .data()
            .proposals()
            .into_iter()
            .map(Into::into)
            .collect(),
        header: uncle.data().header().into(),
    }
}

pub(crate) fn tx_entry_to_template(entry: &TxEntry) -> TransactionTemplate {
    TransactionTemplate {
        hash: entry.transaction().hash().unpack(),
        required: false, // unimplemented
        cycles: Some(entry.cycles.into()),
        depends: None, // unimplemented
        data: entry.transaction().data().into(),
    }
}

pub(crate) fn cellbase_to_template(tx: &TransactionView) -> CellbaseTemplate {
    CellbaseTemplate {
        hash: tx.hash().unpack(),
        cycles: None,
        data: tx.data().into(),
    }
}
