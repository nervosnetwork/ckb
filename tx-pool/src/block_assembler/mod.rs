//! Generate a new block

mod builder;
mod candidate_uncles;
mod json;
mod notify;
mod process;
mod state;

#[cfg(test)]
mod tests;

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError;
pub use candidate_uncles::CandidateUncles;
use ckb_app_config::BlockAssemblerConfig;
use ckb_dao::DaoCalculator;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_jsonrpc_types::BlockTemplate as JsonBlockTemplate;
use ckb_logger::{debug, error, trace, warn};
use ckb_reward_calculator::RewardCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    bytes,
    core::{
        Capacity, EpochExt, ScriptHashType, TransactionBuilder, TransactionView, UncleBlockView,
        cell::{OverlayCellChecker, TransactionsChecker},
    },
    packed::{
        self, Byte32, Bytes, CellInput, CellOutput, CellbaseWitness, OutPoint, ProposalShortId,
        Script, Transaction,
    },
    prelude::*,
};
use http_body_util::Full;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use std::collections::HashSet;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::{cmp, iter};
use tokio::sync::{Mutex, RwLock};
use tokio::task::block_in_place;

use crate::TxPool;
pub(crate) use builder::BlockTemplateBuilder;
pub(crate) use process::process;
pub(crate) use state::{CurrentTemplate, TemplateSize};

type FailedTxs = (ProposalShortId, Option<OutPoint>);
type CalcDaoResult = Result<(Byte32, Vec<TxEntry>, Vec<FailedTxs>), AnyError>;

/// Block generator
#[derive(Clone)]
pub struct BlockAssembler {
    pub(crate) config: Arc<BlockAssemblerConfig>,
    pub(crate) work_id: Arc<AtomicU64>,
    pub(crate) candidate_uncles: Arc<Mutex<CandidateUncles>>,
    /// Current template snapshot. Readers clone the inner `Arc` under a read
    /// lock; updaters build a new `CurrentTemplate` without holding the lock,
    /// then swap the `Arc` under a write lock.
    pub(crate) current: Arc<RwLock<Arc<CurrentTemplate>>>,
    /// Monotonic version used by non-forced updates to detect concurrent
    /// reorgs. A successful write increments this counter.
    pub(crate) version: Arc<AtomicU64>,
    /// Serializes `reset_template` with the read-and-swap window of
    /// `update_full`. `reset_template` holds this lock for its whole duration
    /// so that the `version` read for non-force resets is consistent with the
    /// subsequent swap; `update_full` holds it from the read of `current` until
    /// its own swap. This prevents a reset from swapping between
    /// `update_full`'s read and swap while still allowing partial updates
    /// (`update_uncles/proposals/transactions`) to run concurrently.
    pub(crate) template_lock: Arc<Mutex<()>>,
    pub(crate) poster: Arc<Client<HttpConnector, Full<bytes::Bytes>>>,
}

impl BlockAssembler {
    /// Construct new block generator
    pub fn new(
        config: BlockAssemblerConfig,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, BlockAssemblerError> {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let current_epoch = consensus
            .next_epoch_ext(tip_header, &snapshot.borrow_as_data_loader())
            .expect("tip header's epoch should be stored")
            .epoch();
        let mut builder = BlockTemplateBuilder::new(&snapshot, &current_epoch)?;

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
            .current_time(cmp::max(
                unix_time_as_millis(),
                tip_header
                    .timestamp()
                    .checked_add(1)
                    .ok_or(BlockAssemblerError::Overflow)?,
            ))
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

        Ok(Self {
            config: Arc::new(config),
            work_id: Arc::new(work_id),
            candidate_uncles: Arc::new(Mutex::new(CandidateUncles::new())),
            current: Arc::new(RwLock::new(Arc::new(current))),
            version: Arc::new(AtomicU64::new(0)),
            template_lock: Arc::new(Mutex::new(())),
            poster: Arc::new(
                Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build::<_, Full<bytes::Bytes>>(HttpConnector::new()),
            ),
        })
    }

    pub(crate) async fn update_full(&self, tx_pool: &RwLock<TxPool>) -> Result<(), AnyError> {
        // Serialize with `reset_template` so that the snapshot we read here
        // cannot be reset between the read and the final unconditional swap.
        // Partial updates remain concurrent because they do not take this lock.
        let _template_guard = self.template_lock.lock().await;

        // Reorg finalization has the highest priority: it never checks the
        // version and always succeeds, so a concurrent non-forced update can
        // never overwrite the reorg result.
        let current = self.current.read().await.clone();
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
            for (id, out_point) in failed_txs {
                //"The main reason why a proposed transaction here
                // cannot pass the resolve check is very likely that
                // its ancestor has not been proposed.
                // Therefore, we don't handle it actively—instead,
                // we wait for the ancestor to be re-proposed or
                // to be removed on timeout.
                debug!(
                    "Committing tx {} resolving check failed, out_point {:?}",
                    id, out_point
                );
            }
        }

        let txs_size = Self::checked_entries_size(&checked_txs)?;
        let total_size = basic_size
            .checked_add(txs_size)
            .ok_or(BlockAssemblerError::Overflow)?;

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

        let mut new_current = (*current).clone();
        new_current.template = builder.build();
        new_current.size.txs = txs_size;
        new_current.size.total = total_size;
        new_current.size.proposals = proposals_size;

        trace!(
            "[BlockAssembler] update_full {} uncles-{} proposals-{} txs-{}",
            new_current.template.number,
            new_current.template.uncles.len(),
            new_current.template.proposals.len(),
            new_current.template.transactions.len(),
        );

        self.try_swap_template(new_current, None).await;

        Ok(())
    }

    /// Swap the current template if `expected_version` is still valid.
    ///
    /// `expected_version` of `None` means the swap is unconditional (used for
    /// reorg finalization). Returns `true` if the swap happened.
    async fn try_swap_template(
        &self,
        new_current: CurrentTemplate,
        expected_version: Option<u64>,
    ) -> bool {
        let mut guard = self.current.write().await;
        if expected_version.is_some_and(|expected| self.version.load(Ordering::SeqCst) != expected)
        {
            return false;
        }
        *guard = Arc::new(new_current);
        self.version.fetch_add(1, Ordering::SeqCst);
        true
    }

    pub(crate) async fn reset_template(
        &self,
        snapshot: Arc<Snapshot>,
        force: bool,
    ) -> Result<(), AnyError> {
        // Serialize with `update_full` so that a reorg finalization and an
        // explicit reset never interleave with each other. We must hold the
        // lock while reading `version` for non-force resets: otherwise a
        // concurrent `update_full` could increment `version` between the read
        // and the swap and cause the reset to be silently ignored.
        // Partial updates (`update_uncles/proposals/transactions`) do not take
        // this lock and remain concurrent.
        let _template_guard = self.template_lock.lock().await;

        // Non-forced blank updates (Reset messages) must not overwrite a reorg
        // that happened while they were building.
        let version = if force {
            0
        } else {
            self.version.load(Ordering::SeqCst)
        };

        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let current_epoch = consensus
            .next_epoch_ext(tip_header, &snapshot.borrow_as_data_loader())
            .expect("tip header's epoch should be stored")
            .epoch();
        let mut builder = BlockTemplateBuilder::new(&snapshot, &current_epoch)?;

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
            .current_time(cmp::max(
                unix_time_as_millis(),
                tip_header
                    .timestamp()
                    .checked_add(1)
                    .ok_or(BlockAssemblerError::Overflow)?,
            ))
            .dao(dao);
        if let Some(data) = extension {
            builder.extension(data);
        }
        let template = builder.build();

        trace!(
            "[BlockAssembler] reset_template {} uncles-{} proposals-{} txs-{}",
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

        let expected_version = if force { None } else { Some(version) };
        self.try_swap_template(new_blank, expected_version).await;
        Ok(())
    }

    /// Apply a partial update to the current block template.
    ///
    /// The `update` closure receives a builder and the mutable size summary. It
    /// should configure the builder with the new content and update `size`
    /// accordingly. Returning `false` aborts the update without swapping.
    async fn apply_partial_update<F>(
        &self,
        current: Arc<CurrentTemplate>,
        version: u64,
        label: &'static str,
        update: F,
    ) where
        F: FnOnce(&mut BlockTemplateBuilder, &mut TemplateSize) -> bool,
    {
        let mut builder = BlockTemplateBuilder::from_template(&current.template);
        let mut size = current.size;
        if !update(&mut builder, &mut size) {
            return;
        }

        builder
            .work_id(self.work_id.fetch_add(1, Ordering::SeqCst))
            .current_time(cmp::max(
                unix_time_as_millis(),
                current.template.current_time,
            ));

        let mut new_current = (*current).clone();
        new_current.template = builder.build();
        new_current.size = size;

        trace!(
            "[BlockAssembler] {}-{} epoch-{} uncles-{} proposals-{} txs-{}",
            label,
            new_current.template.number,
            new_current.template.epoch.number(),
            new_current.template.uncles.len(),
            new_current.template.proposals.len(),
            new_current.template.transactions.len(),
        );

        self.try_swap_template(new_current, Some(version)).await;
    }

    pub(crate) async fn update_uncles(&self) {
        let (current, version) = {
            let guard = self.current.read().await;
            (Arc::clone(&*guard), self.version.load(Ordering::SeqCst))
        };
        let consensus = current.snapshot.consensus();
        let max_block_bytes = consensus.max_block_bytes() as usize;
        let max_uncles_num = consensus.max_uncles_num();
        let current_uncles_num = current.template.uncles.len();
        if current_uncles_num >= max_uncles_num {
            return;
        }

        let remain_size = max_block_bytes.saturating_sub(current.size.total);
        if remain_size <= UncleBlockView::serialized_size_in_block() {
            return;
        }

        let uncles = self.prepare_uncles(&current.snapshot, &current.epoch).await;
        let new_uncle_size = uncles.len() * UncleBlockView::serialized_size_in_block();
        let new_total_size = current.size.calc_total_by_uncles(new_uncle_size);
        if new_total_size >= max_block_bytes {
            return;
        }

        self.apply_partial_update(current, version, "update_uncles", |builder, size| {
            builder.set_uncles(uncles);
            size.uncles = new_uncle_size;
            size.total = new_total_size;
            true
        })
        .await;
    }

    pub(crate) async fn update_proposals(&self, tx_pool: &RwLock<TxPool>) {
        let (current, version) = {
            let guard = self.current.read().await;
            (Arc::clone(&*guard), self.version.load(Ordering::SeqCst))
        };
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
        if new_total_size >= max_block_bytes {
            return;
        }

        self.apply_partial_update(current, version, "update_proposals", |builder, size| {
            builder.set_proposals(Vec::from_iter(proposals));
            size.proposals = new_proposals_size;
            size.total = new_total_size;
            true
        })
        .await;
    }

    pub(crate) async fn update_transactions(
        &self,
        tx_pool: &RwLock<TxPool>,
    ) -> Result<(), AnyError> {
        let (current, version) = {
            let guard = self.current.read().await;
            (Arc::clone(&*guard), self.version.load(Ordering::SeqCst))
        };
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

        match Self::calc_dao(
            &current.snapshot,
            &current.epoch,
            current_template.cellbase.clone(),
            txs,
        ) {
            Ok((dao, checked_txs, _failed_txs)) => {
                let new_txs_size = Self::checked_entries_size(&checked_txs)?;
                let new_total_size = current.size.calc_total_by_txs(new_txs_size);
                self.apply_partial_update(
                    current,
                    version,
                    "update_transactions",
                    |builder, size| {
                        builder.set_transactions(checked_txs).dao(dao);
                        if let Some(data) = extension {
                            builder.extension(data);
                        }
                        size.txs = new_txs_size;
                        size.total = new_total_size;
                        true
                    },
                )
                .await;
            }
            Err(err) => {
                warn!(
                    "[BlockAssembler] update_transactions: calc_dao failed, \
                     keeping previous transactions and DAO: {err}"
                );
            }
        }
        Ok(())
    }

    pub(crate) async fn get_current(&self) -> JsonBlockTemplate {
        // Only clone the inner Arc while holding the read lock; the lock is
        // released immediately after this statement.
        let current = self.current.read().await.clone();
        (&current.template).into()
    }

    pub(crate) fn build_cellbase_witness(
        config: &BlockAssemblerConfig,
        snapshot: &Snapshot,
    ) -> CellbaseWitness {
        let hash_type: ScriptHashType = config.hash_type.into();
        let cellbase_lock = Script::new_builder()
            .args(config.args.as_bytes())
            .code_hash(&config.code_hash)
            .hash_type(hash_type)
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
            .message(message)
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
        let candidate_number = tip
            .number()
            .checked_add(1)
            .ok_or(BlockAssemblerError::Overflow)?;
        let cellbase_witness = Self::build_cellbase_witness(config, snapshot);

        let tx = {
            let (target_lock, block_reward) = block_in_place(|| {
                RewardCalculator::new(snapshot.consensus(), snapshot).block_reward_to_finalize(tip)
            })?;
            let input = CellInput::new_cellbase_input(candidate_number);
            let output = CellOutput::new_builder()
                .capacity(block_reward.total)
                .lock(target_lock)
                .build();

            let witness = cellbase_witness.as_bytes();
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
            let bytes = chain_root.calc_mmr_hash().as_bytes().into();
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
                .transactions(vec![cellbase])
                .uncles(uncles.iter().map(|u| u.data()).collect::<Vec<_>>())
                .proposals(proposals.cloned().collect::<Vec<_>>())
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::Block::new_builder()
                .header(header)
                .transactions(vec![cellbase])
                .uncles(uncles.iter().map(|u| u.data()).collect::<Vec<_>>())
                .proposals(proposals.cloned().collect::<Vec<_>>())
                .build()
        };
        block.serialized_size_without_uncle_proposals()
    }

    fn checked_entries_size(entries: &[TxEntry]) -> Result<usize, BlockAssemblerError> {
        entries.iter().try_fold(0usize, |sum, tx| {
            sum.checked_add(tx.size)
                .ok_or(BlockAssemblerError::Overflow)
        })
    }

    fn calc_dao(
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
        cellbase: TransactionView,
        entries: Vec<TxEntry>,
    ) -> CalcDaoResult {
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
                        // Returning the out_point makes debugging easier and provides better logs.
                        checked_failed_txs
                            .push((entry.proposal_short_id(), err.out_point().cloned()));
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
}
