//! Generate a new block

mod builder;
mod candidate_uncles;
mod cell_liveness;
mod dao;
mod json;
mod notify;
mod process;
mod state;

#[cfg(test)]
mod tests;

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError;
use crate::util::block_offload;
pub use candidate_uncles::CandidateUncles;
use cell_liveness::CellLivenessMemo;
use ckb_app_config::BlockAssemblerConfig;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_jsonrpc_types::BlockTemplate as JsonBlockTemplate;
use ckb_logger::{debug, trace, warn};
use ckb_reward_calculator::RewardCalculator;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    bytes,
    core::{
        Capacity, EpochExt, ScriptHashType, TransactionBuilder, TransactionView, UncleBlockView,
    },
    packed::{
        self, Bytes, CellInput, CellOutput, CellbaseWitness, ProposalShortId, Script, Transaction,
    },
    prelude::*,
};
use http_body_util::Full;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use std::collections::HashSet;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicU64, Ordering},
};
use std::{cmp, iter};
use tokio::sync::{Mutex, RwLock};

use crate::TxPool;
pub(crate) use builder::BlockTemplateBuilder;
pub(crate) use process::process;
pub(crate) use state::{CurrentTemplate, TemplateSize};

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
    /// Shared per-tip memo of chain-cell liveness for `calc_dao`. Uses a
    /// `std::sync::Mutex` because the critical sections are short and never
    /// cross `.await`.
    cell_liveness_memo: Arc<StdMutex<CellLivenessMemo>>,
    pub(crate) poster: Arc<Client<HttpConnector, Full<bytes::Bytes>>>,
    /// Test-only observation point for the external miner-notification
    /// boundary. Production builds pay no field or atomic-operation cost.
    #[cfg(test)]
    pub(crate) notify_count: Arc<AtomicU64>,
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

        let work_id = AtomicU64::new(0);
        let cell_liveness_memo = Arc::new(StdMutex::new(CellLivenessMemo::default()));
        let current = Self::build_base_template(
            &config,
            &work_id,
            snapshot,
            &current_epoch,
            vec![],
            &cell_liveness_memo,
        )
        .expect("build initial blank template for BlockAssembler");

        Ok(Self {
            config: Arc::new(config),
            work_id: Arc::new(work_id),
            candidate_uncles: Arc::new(Mutex::new(CandidateUncles::new())),
            current: Arc::new(RwLock::new(Arc::new(current))),
            version: Arc::new(AtomicU64::new(0)),
            template_lock: Arc::new(Mutex::new(())),
            cell_liveness_memo,
            poster: Arc::new(
                Client::builder(hyper_util::rt::TokioExecutor::new())
                    .build::<_, Full<bytes::Bytes>>(HttpConnector::new()),
            ),
            #[cfg(test)]
            notify_count: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Build a fresh base template from `snapshot`.
    ///
    /// The returned template contains no transactions or proposals; it only has
    /// the cellbase, DAO, extension and (optionally) uncles. `uncles` is empty
    /// during initial construction and is populated by `reset_template` after a
    /// reorg. Sharing this path avoids duplicating the cellbase, extension, DAO
    /// and size calculations between `new` and `reset_template`.
    fn build_base_template(
        config: &BlockAssemblerConfig,
        work_id: &AtomicU64,
        snapshot: Arc<Snapshot>,
        current_epoch: &EpochExt,
        uncles: Vec<UncleBlockView>,
        memo: &StdMutex<CellLivenessMemo>,
    ) -> Result<CurrentTemplate, AnyError> {
        let tip_header = snapshot.tip_header();
        let mut builder = BlockTemplateBuilder::new(&snapshot, current_epoch)?;

        let cellbase = Self::build_cellbase(config, &snapshot)?;
        let extension = Self::build_extension(&snapshot)?;
        let basic_block_size =
            Self::basic_block_size(cellbase.data(), &uncles, iter::empty(), extension.clone());
        let uncles_size = Self::uncles_size(&uncles);

        let (dao, _checked_txs, _failed_txs) =
            Self::calc_dao(&snapshot, current_epoch, cellbase.clone(), vec![], memo)?;

        builder
            .transactions(vec![])
            .proposals(vec![])
            .cellbase(cellbase)
            .uncles(uncles)
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
            uncles: uncles_size,
            total: basic_block_size,
        };

        Ok(CurrentTemplate {
            template,
            size,
            snapshot,
            epoch: current_epoch.clone(),
        })
    }

    pub(crate) async fn update_full(&self, tx_pool: &RwLock<TxPool>) -> Result<bool, AnyError> {
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

        let (proposals, uncles, txs, basic_size) = {
            let tx_pool_reader = tx_pool.read().await;
            if current.snapshot.tip_hash() != tx_pool_reader.snapshot().tip_hash() {
                return Ok(false);
            }

            let proposals = tx_pool_reader.package_proposals(consensus.max_block_proposals_limit());
            let proposal_set: HashSet<ProposalShortId> = proposals.iter().cloned().collect();
            let uncles = Self::filter_uncles_conflicting_with_proposals(
                &current.snapshot,
                &current_template.uncles,
                &proposal_set,
            );

            let basic_size = Self::basic_block_size(
                current_template.cellbase.data(),
                &uncles,
                proposals.iter(),
                current_template.extension.clone(),
            );

            let txs_size_limit = max_block_bytes
                .checked_sub(basic_size)
                .ok_or(BlockAssemblerError::Overflow)?;

            let max_block_cycles = consensus.max_block_cycles();
            let (txs, _txs_size, _cycles) =
                tx_pool_reader.package_txs(max_block_cycles, txs_size_limit);
            (proposals, uncles, txs, basic_size)
        };

        let proposals_size = proposals.len() * ProposalShortId::serialized_size();
        let uncles_size = Self::uncles_size(&uncles);
        let (dao, checked_txs, failed_txs) = Self::calc_dao(
            &current.snapshot,
            &current.epoch,
            current_template.cellbase.clone(),
            txs,
            &self.cell_liveness_memo,
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
            .set_uncles(uncles)
            .set_proposals(proposals)
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
        new_current.size.uncles = uncles_size;

        trace!(
            "[BlockAssembler] update_full {} uncles-{} proposals-{} txs-{}",
            new_current.template.number,
            new_current.template.uncles.len(),
            new_current.template.proposals.len(),
            new_current.template.transactions.len(),
        );

        // `expected_version == None` means unconditional swap, so this always
        // returns true; propagate it for clarity and future-proofing.
        let swapped = self.try_swap_template(new_current, None).await;

        Ok(swapped)
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

    pub(crate) async fn reset_template(&self, snapshot: Arc<Snapshot>) -> Result<(), AnyError> {
        // Serialize with `update_full` so that a reorg finalization and an
        // explicit reset never interleave with each other. Partial updates
        // (`update_uncles/proposals/transactions`) do not take this lock and
        // remain concurrent.
        //
        // The swap is always unconditional (`expected_version = None`): both
        // callers (reorg finalization and the management `Reset` message,
        // e.g. `clear_pool`) must not be dropped by the version check — the
        // pool state they rebuild from has already changed.
        let _template_guard = self.template_lock.lock().await;

        let consensus = snapshot.consensus();
        let current_epoch = consensus
            .next_epoch_ext(snapshot.tip_header(), &snapshot.borrow_as_data_loader())
            .expect("tip header's epoch should be stored")
            .epoch();

        let uncles = self.prepare_uncles(&snapshot, &current_epoch).await;
        let new_blank = Self::build_base_template(
            &self.config,
            &self.work_id,
            snapshot,
            &current_epoch,
            uncles,
            &self.cell_liveness_memo,
        )?;

        trace!(
            "[BlockAssembler] reset_template {} uncles-{} proposals-{} txs-{}",
            new_blank.template.number,
            new_blank.template.uncles.len(),
            new_blank.template.proposals.len(),
            new_blank.template.transactions.len(),
        );

        self.try_swap_template(new_blank, None).await;
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
        // Serialize with `update_full`/`reset_template`: `update_full`
        // carries the uncle set forward from the template it read, so an
        // uncles update landing between its read and its (unconditional)
        // swap would be silently reverted. The other partial updates are
        // rebuilt from the pool on every `update_full`, so they do not
        // need this lock.
        let _template_guard = self.template_lock.lock().await;
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

        let prepared = self.prepare_uncles(&current.snapshot, &current.epoch).await;
        let proposals: HashSet<ProposalShortId> =
            current.template.proposals.iter().cloned().collect();
        let mut uncles = Self::filter_uncles_conflicting_with_proposals(
            &current.snapshot,
            &prepared,
            &proposals,
        );
        let compatible_non_empty = !uncles.is_empty();
        // Truncate to the longest fitting suffix of the prepared (ordered)
        // candidate list instead of dropping the whole update when the full
        // set overshoots the budget. The size accounting uses the
        // `serialized_size_without_uncle_proposals` basis — the same basis
        // as the consensus block-bytes limit.
        let mut new_uncle_size = Self::uncles_size(&uncles);
        let mut new_total_size = current.size.calc_total_by_uncles(new_uncle_size);
        while !uncles.is_empty() && new_total_size > max_block_bytes {
            let dropped = uncles.pop().expect("uncles is non-empty");
            new_uncle_size = new_uncle_size.saturating_sub(Self::uncle_size(&dropped));
            new_total_size = current.size.calc_total_by_uncles(new_uncle_size);
        }
        if compatible_non_empty && uncles.is_empty() {
            // Nothing fits at all: keep the current set (the original
            // all-or-nothing behavior for this case).
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
        let mut proposals = {
            let tx_pool_reader = tx_pool.read().await;
            if current.snapshot.tip_hash() != tx_pool_reader.snapshot().tip_hash() {
                return;
            }
            tx_pool_reader.package_proposals(consensus.max_block_proposals_limit())
        };
        let proposal_set: HashSet<ProposalShortId> = proposals.iter().cloned().collect();
        let uncles = Self::filter_uncles_conflicting_with_proposals(
            &current.snapshot,
            &current.template.uncles,
            &proposal_set,
        );

        let new_uncles_size = Self::uncles_size(&uncles);
        let base_total_size = current
            .size
            .calc_total_by_uncles_and_proposals(new_uncles_size, 0);
        let max_block_bytes = consensus.max_block_bytes() as usize;
        let Some((new_proposals_size, new_total_size)) =
            Self::fit_proposal_prefix(&mut proposals, base_total_size, max_block_bytes)
        else {
            return;
        };

        self.apply_partial_update(current, version, "update_proposals", |builder, size| {
            builder.set_uncles(uncles).set_proposals(proposals);
            size.uncles = new_uncles_size;
            size.proposals = new_proposals_size;
            size.total = new_total_size;
            true
        })
        .await;
    }

    /// Keep the highest-scored proposal prefix that fits the remaining block
    /// bytes. Returning `None` means the non-proposal template already exceeds
    /// the limit. Exact fits are valid.
    fn fit_proposal_prefix(
        proposals: &mut Vec<ProposalShortId>,
        base_total_size: usize,
        max_block_bytes: usize,
    ) -> Option<(usize, usize)> {
        let available = max_block_bytes.checked_sub(base_total_size)?;
        let id_size = ProposalShortId::serialized_size();
        let fit_count = (available / id_size).min(proposals.len());
        proposals.truncate(fit_count);
        let proposals_size = fit_count * id_size;
        Some((proposals_size, base_total_size + proposals_size))
    }

    /// Update the transaction set of the current block template.
    ///
    /// Because this is a partial update, the block extension cannot change
    /// without a tip change. The extension already stored in the current
    /// template is reused instead of recomputing the MMR root on every update.
    /// A tip mismatch aborts the update early.
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
        let txs = {
            let tx_pool_reader = tx_pool.read().await;
            if current.snapshot.tip_hash() != tx_pool_reader.snapshot().tip_hash() {
                return Ok(());
            }

            // The extension cannot change without a tip change, so reuse the one
            // already stored in the current template instead of recomputing it
            // from the snapshot.
            let basic_block_size = Self::basic_block_size(
                current_template.cellbase.data(),
                &current_template.uncles,
                current_template.proposals.iter(),
                current_template.extension.clone(),
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
            &self.cell_liveness_memo,
        ) {
            Ok((dao, checked_txs, _failed_txs)) => {
                let new_txs_size = Self::checked_entries_size(&checked_txs)?;
                let new_total_size = current.size.calc_total_by_txs(new_txs_size);
                self.apply_partial_update(
                    current,
                    version,
                    "update_transactions",
                    |builder, size| {
                        // `from_template` already copied the extension; only
                        // transactions and DAO need to change here.
                        builder.set_transactions(checked_txs).dao(dao);
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
            let (target_lock, block_reward) = block_offload(|| {
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

    /// Keep proposal selection live even when miners omit optional uncles.
    ///
    /// Pending transactions are selected as top-level proposals first. Any
    /// template uncle carrying one of those short ids is omitted from this
    /// template, because otherwise `package_proposals` would have to suppress
    /// the id and a miner that drops the uncle could repeat that suppression
    /// indefinitely. Descendants of an omitted uncle are also omitted unless
    /// their parent is independently known on the main chain or as an already
    /// embedded uncle.
    fn filter_uncles_conflicting_with_proposals(
        snapshot: &Snapshot,
        uncles: &[UncleBlockView],
        proposals: &HashSet<ProposalShortId>,
    ) -> Vec<UncleBlockView> {
        let mut included_hashes = HashSet::with_capacity(uncles.len());
        let mut compatible = Vec::with_capacity(uncles.len());

        for uncle in uncles {
            let conflicts = uncle
                .data()
                .proposals()
                .into_iter()
                .any(|id| proposals.contains(&id));
            if conflicts {
                continue;
            }

            let parent_hash = uncle.header().parent_hash();
            if snapshot.is_main_chain(&parent_hash)
                || snapshot.is_uncle(&parent_hash)
                || included_hashes.contains(&parent_hash)
            {
                included_hashes.insert(uncle.hash());
                compatible.push(uncle.clone());
            }
        }

        compatible
    }

    /// The byte contribution of one uncle on the template's accounting
    /// basis (`serialized_size_without_uncle_proposals`, the same basis as
    /// `basic_block_size` and the consensus block-bytes limit): the packed
    /// in-block size minus its proposal ids.
    fn uncle_size(uncle: &UncleBlockView) -> usize {
        UncleBlockView::serialized_size_in_block()
            .saturating_sub(uncle.data().proposals().len() * ProposalShortId::serialized_size())
    }

    fn uncles_size(uncles: &[UncleBlockView]) -> usize {
        uncles.iter().map(Self::uncle_size).sum()
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
}
