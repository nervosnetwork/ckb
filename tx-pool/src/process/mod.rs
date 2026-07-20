use crate::component::entry::TxEntry;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pool_map::Status;
use crate::constants::{GAP_PROPOSAL_INDEX, PROPOSED_PROPOSAL_INDEX};
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use crate::util::non_contextual_verify;
use ckb_error::{AnyError, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::{debug, error, info};
use ckb_snapshot::Snapshot;
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        BlockView, Capacity, EstimateMode, FeeRate, HeaderView, TransactionView,
        cell::ResolvedTransaction,
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{TxVerifyEnv, cache::Completed};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::mpsc;

mod classify;
mod orphan;
mod post_process;
mod rbf;
mod remove;
mod reorg;
mod submit;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

/// Map a pool status to the verification environment used for contextual checks.
fn status_to_verify_env(status: Status, header: &HeaderView) -> TxVerifyEnv {
    match status {
        Status::Pending => TxVerifyEnv::new_submit(header),
        Status::Gap => TxVerifyEnv::new_proposed(header, GAP_PROPOSAL_INDEX),
        Status::Proposed => TxVerifyEnv::new_proposed(header, PROPOSED_PROPOSAL_INDEX),
    }
}

/// Map a pool status to the block-assembler notification that should be sent
/// when a transaction reaches this status.
fn status_to_block_assembler_message(status: Status) -> Option<BlockAssemblerMessage> {
    match status {
        Status::Pending => Some(BlockAssemblerMessage::Pending),
        Status::Proposed => Some(BlockAssemblerMessage::Proposed),
        Status::Gap => None,
    }
}

impl TxPoolService {
    pub(crate) async fn get_block_template(&self) -> Result<BlockTemplate, AnyError> {
        if let Some(ref block_assembler) = self.block_assembler {
            Ok(block_assembler.get_current().await)
        } else {
            Err(InternalErrorKind::Config
                .other("BlockAssembler disabled")
                .into())
        }
    }

    pub(crate) async fn notify_block_assembler(&self, status: Status) {
        if !self.should_notify_block_assembler() {
            return;
        }
        if let Some(message) = status_to_block_assembler_message(status) {
            // try_send on purpose: the consumer deduplicates messages into
            // Pending/Proposed/Uncle variants on each interval, so dropping a
            // duplicate notification when the channel is full is harmless.
            // Using send().await here would backpressure verify workers
            // whenever the block-assembler loop is slow.
            if let Err(err) = self.block_assembler_sender.try_send(message) {
                match err {
                    mpsc::error::TrySendError::Full(_) => {
                        debug!("block_assembler channel full, skip duplicate notification")
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        error!("block_assembler receiver dropped")
                    }
                }
            }
        }
    }

    pub(crate) async fn verify_queue_contains(&self, tx: &TransactionView) -> bool {
        let queue = self.queues.verify_queue.read().await;
        queue.contains_key(&tx.proposal_short_id())
    }

    /// Read-lock the pool and run `f` without cloning the snapshot.
    pub(crate) async fn read_tx_pool<U, F: FnOnce(&TxPool) -> U>(&self, f: F) -> U {
        let tx_pool = self.tx_pool.read().await;
        f(&tx_pool)
    }

    /// Read-lock the pool and return the result along with a cloned snapshot.
    pub(crate) async fn read_tx_pool_with_snapshot<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(
        &self,
        tx: &TransactionView,
        source: TxSource,
    ) -> Result<(), Reject> {
        if let Err(reject) = non_contextual_verify(&self.consensus, tx) {
            if reject.is_malformed_tx()
                && let Some(peer) = source.peer()
            {
                self.ban_malformed(peer, format!("reject {reject}")).await;
            }
            return Err(reject);
        }
        Ok(())
    }

    /// Common pre-flight checks shared by all transaction submission paths.
    ///
    /// Runs non-contextual verification and rejects duplicates that are
    /// already in any pipeline queue (ordered resolve, pre-check, verify)
    /// or the orphan pool.
    pub(crate) async fn check_tx_basic_validity(
        &self,
        tx: &TransactionView,
        source: TxSource,
    ) -> Result<(), Reject> {
        self.non_contextual_verify(tx, source).await?;

        let dup = || Reject::Duplicated(tx.hash());
        let id = tx.proposal_short_id();

        {
            let ordered = self.queues.ordered_resolve_queue.read().await;
            if ordered.contains_key(&id) {
                return Err(dup());
            }
        }

        if self.queues.pre_check_queue.contains_key(&id) {
            return Err(dup());
        }

        if self.verify_queue_contains(tx).await {
            return Err(dup());
        }

        if self.orphan_contains(tx).await {
            return Err(dup());
        }

        Ok(())
    }

    async fn classify_and_enqueue_with_full_reject_notification(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        let tx_hash = tx.hash();
        let ret = self.classify_and_enqueue_tx_spawn(tx, source).await;

        if matches!(ret, Err(Reject::Full(_))) {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash });
        }

        ret
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<bool, Reject> {
        self.check_tx_basic_validity(&tx, source).await?;
        self.classify_and_enqueue_with_full_reject_notification(tx, source)
            .await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        self.check_tx_basic_validity(&tx, TxSource::Proposal)
            .await?;
        self.classify_and_enqueue_with_full_reject_notification(tx, TxSource::Proposal)
            .await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<Completed, Reject> {
        self.check_tx_basic_validity(&tx, source).await?;

        let ret = self.process_tx_direct(tx.clone(), source, None).await;
        self.after_process(tx, source, &ret).await;
        ret
    }

    pub(crate) fn put_recent_reject(&self, tx_hash: &Byte32, reject: &Reject) {
        if let Some(ref recent_reject) = self.aux.recent_reject
            && let Err(e) = recent_reject.put(tx_hash, reject.clone())
        {
            error!(
                "Failed to record recent_reject {} {} {}",
                tx_hash, reject, e
            );
        }
    }

    pub(crate) fn send_result_to_relayer(&self, result: TxVerificationResult) {
        if let Err(e) = self.tx_relay_sender.send(result) {
            error!("tx-pool tx_relay_sender internal error {}", e);
        }
    }
    pub(crate) fn sort_txs_by_dependencies(txs: &mut Vec<TransactionView>) {
        Self::sort_by_dependencies(txs, |tx| tx);
    }

    /// Topologically sort a list of items that wrap transactions so that
    /// parents are placed before their children. `tx_of` extracts the
    /// transaction reference from each item.
    pub(crate) fn sort_by_dependencies<T>(
        items: &mut Vec<T>,
        tx_of: impl Fn(&T) -> &TransactionView,
    ) {
        if items.len() <= 1 {
            return;
        }

        let mut output_to_index: HashMap<OutPoint, usize> =
            HashMap::with_capacity(items.len().saturating_mul(2));
        for (i, item) in items.iter().enumerate() {
            let tx_hash = tx_of(item).hash();
            for idx in 0..tx_of(item).outputs().len() {
                output_to_index.insert(OutPoint::new(tx_hash.clone(), idx as u32), i);
            }
        }

        let mut in_degree = vec![0usize; items.len()];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
        for (i, item) in items.iter().enumerate() {
            let tx = tx_of(item);
            for input in tx.input_pts_iter() {
                if let Some(&parent) = output_to_index.get(&input)
                    && parent != i
                {
                    in_degree[i] += 1;
                    children[parent].push(i);
                }
            }
            for dep in tx.cell_deps_iter() {
                let out_point = dep.out_point();
                if let Some(&parent) = output_to_index.get(&out_point)
                    && parent != i
                {
                    in_degree[i] += 1;
                    children[parent].push(i);
                }
            }
        }

        let mut ready: VecDeque<usize> = (0..items.len()).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted = Vec::with_capacity(items.len());
        while let Some(i) = ready.pop_front() {
            sorted.push(i);
            for &child in &children[i] {
                in_degree[child] -= 1;
                if in_degree[child] == 0 {
                    ready.push_back(child);
                }
            }
        }

        if sorted.len() != items.len() {
            // A cycle should never happen in valid detached blocks, but if it
            // does we keep the original order rather than losing transactions.
            return;
        }

        let mut remaining: Vec<Option<T>> = items.drain(..).map(Some).collect();
        items.extend(
            sorted
                .into_iter()
                .map(|i| remaining[i].take().expect("index valid")),
        );
    }

    pub(crate) async fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposal_id: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
    ) {
        let mine_mode = self.block_assembler.is_some();
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        let detached_headers: HashSet<Byte32> = detached_blocks
            .iter()
            .map(|blk| blk.header().hash())
            .collect();

        for blk in detached_blocks {
            detached.extend(blk.transactions().into_iter().skip(1))
        }

        for blk in attached_blocks {
            self.aux.fee_estimator.commit_block(&blk);
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let mut retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let reject_events;
        {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.tx_pool.write().await;

            reject_events = reorg::update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                &self.callbacks,
                mine_mode,
            );
        }

        // Dispatch reject callbacks outside the write lock, then clean up
        // in-flight RBF candidates targeting inputs that have just been freed
        // by the reorg (committed, expired, detached, or evicted transactions).
        for (entry, reject) in &reject_events {
            self.callbacks.call_reject(entry, reject.clone());
        }
        self.cleanup_rbf_for_removed_entries(reject_events.iter().map(|(entry, _)| entry))
            .await;

        self.remove_orphan_txs_by_attach(&attached).await;
        {
            let mut queue = self.queues.verify_queue.write().await;
            queue.remove_txs(attached.iter().map(|tx| tx.proposal_short_id()));
        }

        // Recover detached transactions through the direct per-tx entry point.
        // Dependent transactions must be processed after their parents have
        // already been submitted to the pool; a topological sort guarantees this
        // ordering. Using `process_tx_direct` here keeps the recovery logic simple
        // and correct while still releasing the tx-pool write lock between
        // transactions.
        {
            Self::sort_txs_by_dependencies(&mut retain);
            let mut chunk_rx = self.chunk_rx.clone();
            for tx in retain {
                let ret = self
                    .process_tx_direct(tx.clone(), TxSource::Local, Some(&mut chunk_rx))
                    .await;
                if let Err(ref reject) = ret {
                    debug!("reorg re-add failed: {}", reject);
                    self.after_process(tx, TxSource::Local, &ret).await;
                } else {
                    // The detached tx is now back in the pool. Wake up any
                    // orphans that depend on it (including via cell dep).
                    self.process_orphan_tx(&tx).await;
                }
            }
        }
    }

    /// Clear all pipeline queues without touching the already-accepted pool.
    ///
    /// Locks are acquired one at a time in the documented hierarchy
    /// (`ordered_resolve_queue → rbf_candidates → verify_queue → orphan`),
    /// with the synchronous `pre_check_queue` mutex last. Each guard is
    /// released immediately after its `clear()`, so there is no deadlock
    /// risk, but the operation is *not* atomic: workers may keep moving
    /// transactions between queues while the clear is in progress, and
    /// transactions already popped by a worker are unaffected. Callers that
    /// need a guaranteed-empty pipeline must additionally quiesce the
    /// pipeline workers (not implemented here).
    pub(crate) async fn clear_pipeline_queues(&self) {
        self.queues.ordered_resolve_queue.write().await.clear();
        self.queues.rbf_candidates.write().await.clear();
        self.queues.verify_queue.write().await.clear();
        self.orphan.write().await.clear();
        // `pre_check_queue` uses a std::sync::Mutex, independent of the async
        // lock hierarchy; keep it last so it can never be held across an
        // `.await`.
        self.queues.pre_check_queue.clear();
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
        }
        self.clear_pipeline_queues().await;
        // reset block_assembler
        if self
            .block_assembler_sender
            .send(BlockAssemblerMessage::Reset(new_snapshot))
            .await
            .is_err()
        {
            error!("block_assembler receiver dropped");
        }
    }

    pub(crate) async fn save_pool(&self) {
        let mut tx_pool = self.tx_pool.write().await;
        if let Err(err) = tx_pool.save_into_file() {
            error!("failed to save pool, error: {:?}", err)
        } else {
            info!("TxPool saved successfully")
        }
    }

    pub(crate) async fn update_ibd_state(&self, in_ibd: bool) {
        self.aux.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AnyError> {
        let all_entry_info = self
            .read_tx_pool(|tx_pool| tx_pool.get_all_entry_info())
            .await;
        match self
            .aux
            .fee_estimator
            .estimate_fee_rate(estimate_mode, all_entry_info)
        {
            Ok(fee_rate) => Ok(fee_rate),
            Err(err) => {
                if enable_fallback {
                    let target_blocks =
                        FeeEstimator::target_blocks_for_estimate_mode(estimate_mode);
                    self.tx_pool
                        .read()
                        .await
                        .estimate_fee_rate(target_blocks)
                        .map_err(Into::into)
                } else {
                    Err(err.into())
                }
            }
        }
    }
}

pub(crate) struct PreCheckedTx {
    /// Tip hash at the time the transaction was pre-checked.
    pub(crate) pre_resolve_tip: Byte32,
    /// Fully resolved transaction.
    pub(crate) rtx: Arc<ResolvedTransaction>,
    /// Current status (fresh / gap / proposed) relative to the proposal window.
    pub(crate) status: Status,
    /// Transaction fee.
    pub(crate) fee: Capacity,
    /// Transaction size in bytes as serialized in a block.
    pub(crate) tx_size: usize,
}

/// Transactions recovered from the conflict cache after a failed or partial
/// RBF replacement, paired with their original pipeline source.
pub(crate) type RecoveredTxs = Vec<(TransactionView, TxSource)>;

/// Outcome of [`TxPoolService::try_submit_entry`].
pub(crate) type SubmitEntryOutcome = (Result<(), Reject>, RecoveredTxs, Vec<(TxEntry, Reject)>);

type ResolveResult = Result<(Arc<ResolvedTransaction>, Status), Reject>;

/// Assemble a [`PreCheckedTx`] from its parts; shared by the fast
/// (chain-only) and locked (pool-overlay) pre-check paths.
fn make_pre_checked_tx(
    pre_resolve_tip: Byte32,
    rtx: Arc<ResolvedTransaction>,
    status: Status,
    fee: Capacity,
    tx_size: usize,
) -> PreCheckedTx {
    PreCheckedTx {
        pre_resolve_tip,
        rtx,
        status,
        fee,
        tx_size,
    }
}

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> Status {
    if snapshot.proposals().contains_proposed(short_id) {
        Status::Proposed
    } else if snapshot.proposals().contains_gap(short_id) {
        Status::Gap
    } else {
        Status::Pending
    }
}

fn resolve_tx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    tx: TransactionView,
    rbf: bool,
) -> ResolveResult {
    let short_id = tx.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool
        .resolve_tx_from_pool(tx, rbf)
        .map(|rtx| (rtx, tx_status))
}
