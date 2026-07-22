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
mod post_process;
mod rbf;
pub(crate) mod recover;
mod remove;
pub(crate) mod reorg;
pub(crate) mod submit;

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
    /// The reject reason used when an in-flight candidate is superseded by a
    /// higher-fee-rate one. One constant so the register-time and
    /// submit-time paths produce the identical message.
    pub(crate) const SUPERSEDED_BY_HIGHER_FEE_CANDIDATE: &'static str =
        "superseded by higher-fee-rate in-flight candidate";

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
            if let Err(err) = self.relay.block_assembler_sender.try_send(message) {
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
        let queue = self.pipeline.queues.verify_queue.read().await;
        queue.contains_key(&tx.proposal_short_id())
    }

    /// Read-lock the pool and run `f` without cloning the snapshot.
    pub(crate) async fn read_tx_pool<U, F: FnOnce(&TxPool) -> U>(&self, f: F) -> U {
        let tx_pool = self.pool.tx_pool.read().await;
        f(&tx_pool)
    }

    /// Read-lock the pool and return the result along with a cloned snapshot.
    pub(crate) async fn read_tx_pool_with_snapshot<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.pool.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(&self, tx: &TransactionView) -> Result<(), Reject> {
        // Malformed peer banning lives in the after_process remote-reject
        // path (`handle_remote_reject`); keeping it out of this shared
        // check keeps the ban decision in exactly one place.
        non_contextual_verify(&self.pool.consensus, tx)
    }

    /// Common pre-flight checks shared by all transaction submission paths.
    ///
    /// Runs non-contextual verification and rejects duplicates that are
    /// already in any pipeline queue (ordered resolve, pre-check, verify)
    /// or the orphan pool.
    pub(crate) async fn check_tx_basic_validity(&self, tx: &TransactionView) -> Result<(), Reject> {
        self.non_contextual_verify(tx).await?;

        let dup = || Reject::Duplicated(tx.hash());
        let id = tx.proposal_short_id();

        {
            let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
            if ordered.contains_key(&id) {
                return Err(dup());
            }
        }

        if self.pipeline.queues.pre_check_queue.contains_key(&id) {
            return Err(dup());
        }

        if self.verify_queue_contains(tx).await {
            return Err(dup());
        }

        if self.waiting_room_contains(tx).await {
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
        // Preflight failures go through after_process too: the remote error
        // triple (ban / relay / recent_reject) must apply here exactly like
        // it does to failures deeper in the pipeline, otherwise a peer can
        // resubmit the same malformed tx forever with no consequence.
        if let Err(reject) = self.check_tx_basic_validity(&tx).await {
            self.reject_with_after_process(tx, source, reject.clone())
                .await;
            return Err(reject);
        }
        self.classify_and_enqueue_with_full_reject_notification(tx, source)
            .await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        if let Err(reject) = self.check_tx_basic_validity(&tx).await {
            self.reject_with_after_process(tx, TxSource::Proposal, reject.clone())
                .await;
            return Err(reject);
        }
        self.classify_and_enqueue_with_full_reject_notification(tx, TxSource::Proposal)
            .await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        source: TxSource,
    ) -> Result<Completed, Reject> {
        if let Err(reject) = self.check_tx_basic_validity(&tx).await {
            // Same side effects as a pipeline rejection — in particular the
            // local-duplicate re-broadcast in `after_process_local`.
            self.reject_with_after_process(tx, source, reject.clone())
                .await;
            return Err(reject);
        }

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
        if let Err(e) = self.relay.tx_relay_sender.send(result) {
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
        // Hold the recovery lock for the *whole* reorg — the write-lock
        // section, the callbacks, and the retained-transaction recovery —
        // so `save_pool` can never persist a half-updated pool: a snapshot
        // taken between the write-lock section and the recovery would
        // silently lose the detached transactions that have not been
        // re-added yet. Lock order: recovery_lock before tx_pool, and
        // nothing holding tx_pool ever acquires recovery_lock.
        let _recovery_guard = self.recovery_lock.lock().await;

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

        let reorg::ReorgOutcome {
            reject_events,
            silently_removed,
            notify_events,
        } = {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.pool.tx_pool.write().await;

            reorg::update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                mine_mode,
            )
        };

        // Dispatch reject callbacks outside the write lock, then clean up
        // in-flight RBF candidates targeting inputs that have just been freed
        // by the reorg (committed, expired, detached, or evicted transactions).
        // The reconcile's silently-removed entries freed their inputs too and
        // must join the same cleanup — it is driven by this call alone.
        for (entry, reject) in &reject_events {
            self.relay.callbacks.call_reject(entry, reject.clone());
        }
        // Proposed/pending notifications collected inside the write-lock
        // section are dispatched here, outside the lock (user callbacks
        // must never run in-lock: a blocking or re-entering callback
        // would stall the whole pool).
        for (entry, status) in &notify_events {
            match status {
                crate::component::pool_map::Status::Proposed => {
                    self.relay.callbacks.call_proposed(entry)
                }
                _ => self.relay.callbacks.call_pending(entry),
            }
        }
        self.cleanup_rbf_for_removed_entries(
            reject_events
                .iter()
                .map(|(entry, _)| entry)
                .chain(silently_removed.iter()),
        )
        .await;

        self.remove_orphan_txs_by_attach(&attached).await;
        // Remove the newly committed transactions from every pipeline
        // structure with the full terminal sequence (queue removal,
        // registration removal, and finalize semantics for the candidates
        // an attached winner held: their race is lost for real — relayed,
        // but not recorded).
        let attached_ids: Vec<ProposalShortId> =
            attached.iter().map(|tx| tx.proposal_short_id()).collect();
        self.remove_attached_from_pipeline(&attached_ids).await;

        // Recover detached transactions through the direct per-tx entry point.
        // Dependent transactions must be processed after their parents have
        // already been submitted to the pool; a topological sort guarantees this
        // ordering. Using `process_tx_direct` here keeps the recovery logic simple
        // and correct while still releasing the tx-pool write lock between
        // transactions.
        {
            Self::sort_txs_by_dependencies(&mut retain);
            let mut chunk_rx = self.pipeline.chunk_rx.clone();
            for tx in retain {
                let outcome = self
                    .process_tx_direct_outcome(tx.clone(), TxSource::Local, Some(&mut chunk_rx))
                    .await;
                // Only a definitive failure may cascade-remove dependents.
                // `Superseded` means the tx is merely held by a stronger
                // in-flight RBF registration (its fate follows the winner's),
                // and `Duplicated` means it is already back in the pool
                // (concurrent resubmission, or this same reorg retried after
                // a panic) — cascading on either would evict healthy
                // dependents and emit spurious Dead rejections.
                let reject = match outcome {
                    Ok(crate::process::submit::VerifySubmitOutcome::Committed(_))
                    | Err(Reject::Duplicated(_)) => {
                        // The detached tx is back in the pool. Wake up any
                        // orphans that depend on it (including via cell dep).
                        self.process_orphan_tx(&tx).await;
                        continue;
                    }
                    Ok(crate::process::submit::VerifySubmitOutcome::Superseded) => {
                        debug!(
                            "reorg re-add {} held by a stronger in-flight RBF candidate",
                            tx.hash()
                        );
                        continue;
                    }
                    Err(reject) => reject,
                };
                {
                    debug!("reorg re-add failed: {}", reject);
                    // The detached tx could not be re-added: any in-pool
                    // transactions referencing its outputs — as inputs *or*
                    // as cell deps — can never resolve now and would sit in
                    // the pool as zombies until expiry (the template
                    // builder filters them out every round). Cascade-remove
                    // them; callbacks are dispatched outside the write lock.
                    let mut cascaded = Vec::new();
                    {
                        let mut tx_pool = self.pool.tx_pool.write().await;
                        for out_point in tx.output_pts() {
                            let mut dependents: HashSet<ProposalShortId> = HashSet::new();
                            if let Some(id) = tx_pool
                                .pool_map
                                .out_point_index
                                .get_input_ref(&out_point)
                                .cloned()
                            {
                                dependents.insert(id);
                            }
                            if let Some(ids) =
                                tx_pool.pool_map.out_point_index.get_deps_ref(&out_point)
                            {
                                dependents.extend(ids.iter().cloned());
                            }
                            for child_id in dependents {
                                let removed =
                                    tx_pool.pool_map.remove_entry_and_descendants(&child_id);
                                cascaded.extend(
                                    removed.into_iter().map(|entry| (entry, out_point.clone())),
                                );
                            }
                        }
                    }
                    // These entries released their inputs outside the main
                    // reorg reconciliation outcome. Remove speculative RBF
                    // registrations targeting those inputs as well; otherwise
                    // a ghost candidate can keep future replacements blocked
                    // after its conflict target no longer exists in the pool.
                    self.cleanup_rbf_for_removed_entries(cascaded.iter().map(|(entry, _)| entry))
                        .await;
                    for (entry, out_point) in cascaded {
                        debug!(
                            "cascade-remove pool tx {}: its reference {:?} died with the failed re-add",
                            entry.transaction().hash(),
                            out_point,
                        );
                        self.relay.callbacks.call_reject(
                            &entry,
                            Reject::Resolve(ckb_types::core::error::OutPointError::Dead(out_point)),
                        );
                    }
                    // Orphans parked on this tx as their missing parent can
                    // never resolve either — remove them with the same
                    // Reject notification the orphan-eviction route sends.
                    let orphan_entries = {
                        let mut room = self.pipeline.waiting_room.write().await;
                        let ids: Vec<ProposalShortId> =
                            room.find_by_parent(&tx).into_iter().cloned().collect();
                        ids.into_iter()
                            .filter_map(|id| room.remove(&id))
                            .collect::<Vec<_>>()
                    };
                    for entry in orphan_entries {
                        self.send_result_to_relayer(TxVerificationResult::Reject {
                            tx_hash: entry.tx.hash(),
                        });
                    }
                    self.after_process(tx, TxSource::Local, &Err(reject)).await;
                }
            }
        }
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        // Same lock as the reorg recovery: an in-flight reorg must finish
        // re-adding its detached transactions before the pool is cleared,
        // otherwise the freshly cleared pool would be repopulated by the
        // recovery and `clear_pool` would return with a non-empty pool.
        let _recovery_guard = self.recovery_lock.lock().await;
        {
            let mut tx_pool = self.pool.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
        }
        self.clear_pipeline_queues().await;
        // reset block_assembler
        if self
            .relay
            .block_assembler_sender
            .send(BlockAssemblerMessage::Reset(new_snapshot))
            .await
            .is_err()
        {
            error!("block_assembler receiver dropped");
        }
    }

    pub(crate) async fn save_pool(&self) {
        // Wait for any in-flight reorg recovery to finish so the persisted
        // file always represents a complete recovery point: a snapshot
        // taken mid-recovery would silently lose the detached transactions
        // that have not been re-added yet. Lock order: recovery_lock before
        // tx_pool, and nothing holding tx_pool ever acquires recovery_lock.
        let _recovery_guard = self.recovery_lock.lock().await;
        let mut tx_pool = self.pool.tx_pool.write().await;
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
                    self.pool
                        .tx_pool
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
