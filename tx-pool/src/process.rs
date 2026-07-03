use crate::callback::Callbacks;
use crate::component::entry::TxEntry;
#[cfg(feature = "pipeline")]
use crate::component::flight_tracker::FlightTracker;
use crate::component::orphan::Entry as OrphanEntry;
use crate::component::pool_map::Status;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::service::{BlockAssemblerMessage, TxPoolService, TxVerificationResult};
use crate::try_or_return_with_snapshot;
use crate::util::{
    check_tx_fee, check_tx_fee_with_min_fee_rate, check_txid_collision, is_missing_input,
    non_contextual_verify, time_relative_verify, verify_rtx,
};
use ckb_error::{AnyError, InternalErrorKind};
use ckb_fee_estimator::FeeEstimator;
use ckb_jsonrpc_types::BlockTemplate;
use ckb_logger::Level::Trace;
use ckb_logger::{debug, error, info, log_enabled_target, trace_target, warn};
use ckb_network::PeerIndex;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        BlockView, Capacity, Cycle, EstimateMode, FeeRate, HeaderView, TransactionView,
        cell::{ResolvedTransaction, resolve_transaction},
    },
    packed::{Byte32, ProposalShortId},
};
use ckb_util::LinkedHashSet;
use ckb_verification::{
    TxVerifyEnv,
    cache::{CacheEntry, Completed},
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// A list for plug target for `plug_entry` method
pub enum PlugTarget {
    /// Pending pool
    Pending,
    /// Proposed pool
    Proposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxStatus {
    Fresh,
    Gap,
    Proposed,
}

impl TxStatus {
    fn with_env(self, header: &HeaderView) -> TxVerifyEnv {
        match self {
            TxStatus::Fresh => TxVerifyEnv::new_submit(header),
            TxStatus::Gap => TxVerifyEnv::new_proposed(header, 0),
            TxStatus::Proposed => TxVerifyEnv::new_proposed(header, 1),
        }
    }
}

/// A classification job that is offloaded to the pre-check worker pool.
#[cfg(feature = "pipeline")]
#[derive(Clone)]
pub(crate) struct PreCheckJob {
    pub tx: TransactionView,
    pub is_proposal_tx: bool,
    pub remote: Option<(Cycle, PeerIndex)>,
}

/// 256mb for total_tx_size limit, default max_tx_pool_size is 180mb.
#[cfg(feature = "pipeline")]
const DEFAULT_MAX_PRE_CHECK_QUEUE_TX_SIZE: usize = 256_000_000;

/// A small multi-consumer queue used by the pre-check worker pool.
///
/// It is intentionally kept separate from the ordered resolve queue: jobs here
/// are independent and can be processed in any order, while the ordered queue
/// must retry missing-input txs in arrival order.
///
/// `tokio::sync::Notify` is used for wake-ups; a permit is stored if a job is
/// pushed before any worker is waiting, so a worker that calls `notified()`
/// afterwards will wake up immediately.
///
/// The queue is bounded by total serialized tx size so a flood of large remote
/// txs cannot grow it without limit.
///
/// Workers are cancelled via the `CancellationToken`; `pop()` returns `None`
/// once the token is cancelled so the worker loop can exit cleanly.
#[cfg(feature = "pipeline")]
struct PreCheckQueueState {
    inner: std::collections::VecDeque<PreCheckJob>,
    index: std::collections::HashSet<ProposalShortId>,
    flight: FlightTracker,
}

#[cfg(feature = "pipeline")]
pub(crate) struct PreCheckQueue {
    state: std::sync::Mutex<PreCheckQueueState>,
    ready: tokio::sync::Notify,
    cancel: ckb_stop_handler::CancellationToken,
    total_tx_size: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "pipeline")]
impl PreCheckQueue {
    pub(crate) fn new(cancel: ckb_stop_handler::CancellationToken) -> Self {
        Self {
            state: std::sync::Mutex::new(PreCheckQueueState {
                inner: std::collections::VecDeque::new(),
                index: std::collections::HashSet::new(),
                flight: FlightTracker::new(),
            }),
            ready: tokio::sync::Notify::new(),
            cancel,
            total_tx_size: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn tx_size(job: &PreCheckJob) -> usize {
        job.tx.data().serialized_size_in_block()
    }

    /// Returns true if the queue is full.
    pub fn is_full(&self, add_tx_size: usize) -> bool {
        self.total_tx_size
            .load(std::sync::atomic::Ordering::SeqCst)
            .saturating_add(add_tx_size)
            >= DEFAULT_MAX_PRE_CHECK_QUEUE_TX_SIZE
    }

    /// Returns true if the given tx spends or references an output produced by
    /// a transaction currently in the pre-check queue.
    pub fn depends_on(&self, tx: &TransactionView) -> bool {
        let state = self.state.lock().expect("pre_check queue lock poisoned");
        state.flight.depends_on(tx)
    }

    /// Returns true if the queue contains a job for the given proposal id.
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        let state = self.state.lock().expect("pre_check queue lock poisoned");
        state.index.contains(id)
    }

    /// Returns the raw transaction for the given id, if it is waiting in the
    /// pre-check queue.
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        let state = self.state.lock().expect("pre_check queue lock poisoned");
        state
            .inner
            .iter()
            .find(|job| &job.tx.proposal_short_id() == id)
            .map(|job| job.tx.clone())
    }

    /// Remove a job from the queue by its short id.
    pub fn remove_by_id(&self, id: &ProposalShortId) -> Option<TransactionView> {
        let mut state = self.state.lock().expect("pre_check queue lock poisoned");
        let pos = state
            .inner
            .iter()
            .position(|job| &job.tx.proposal_short_id() == id)?;
        let job = state.inner.remove(pos).expect("position exists");
        state.index.remove(id);
        state.flight.remove(id);
        self.total_tx_size
            .fetch_sub(Self::tx_size(&job), std::sync::atomic::Ordering::SeqCst);
        Some(job.tx)
    }

    /// Remove all jobs submitted by the given peer.
    pub fn remove_by_peer(&self, peer: &PeerIndex) -> Vec<TransactionView> {
        let mut state = self.state.lock().expect("pre_check queue lock poisoned");
        let to_remove: Vec<usize> = state
            .inner
            .iter()
            .enumerate()
            .filter(|(_, job)| job.remote.as_ref().is_some_and(|(_, p)| p == peer))
            .map(|(idx, _)| idx)
            .collect();

        let mut removed = Vec::with_capacity(to_remove.len());
        for idx in to_remove.into_iter().rev() {
            let job = state.inner.remove(idx).expect("position exists");
            let id = job.tx.proposal_short_id();
            state.index.remove(&id);
            state.flight.remove(&id);
            self.total_tx_size
                .fetch_sub(Self::tx_size(&job), std::sync::atomic::Ordering::SeqCst);
            removed.push(job.tx);
        }
        removed
    }

    pub(crate) fn push(&self, job: PreCheckJob) -> Result<(), Reject> {
        let mut state = self.state.lock().expect("pre_check queue lock poisoned");
        let id = job.tx.proposal_short_id();
        if state.index.contains(&id) {
            return Ok(());
        }
        let tx_size = Self::tx_size(&job);
        let tx_hash = job.tx.hash();
        // The full check is performed while holding the lock so concurrent
        // pushes cannot both observe a non-full queue and exceed the limit.
        if self.is_full(tx_size) {
            return Err(Reject::Full(format!(
                "pre_check_queue total_tx_size exceeded, failed to add tx: {tx_hash:#x}"
            )));
        }
        state.index.insert(id.clone());
        state.flight.insert(id, &job.tx);
        state.inner.push_back(job);
        self.total_tx_size
            .fetch_add(tx_size, std::sync::atomic::Ordering::SeqCst);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    /// Drain all pending jobs without cancelling the queue.
    pub(crate) fn clear(&self) {
        let mut state = self.state.lock().expect("pre_check queue lock poisoned");
        state.inner.clear();
        state.index.clear();
        state.flight.clear();
        self.total_tx_size
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }

    /// Pop the next job, or return `None` if the queue has been cancelled.
    pub(crate) async fn pop(&self) -> Option<PreCheckJob> {
        loop {
            {
                let mut state = self.state.lock().expect("pre_check queue lock poisoned");
                if let Some(job) = state.inner.pop_front() {
                    let id = job.tx.proposal_short_id();
                    state.index.remove(&id);
                    state.flight.remove(&id);
                    let tx_size = Self::tx_size(&job);
                    self.total_tx_size
                        .fetch_sub(tx_size, std::sync::atomic::Ordering::SeqCst);
                    return Some(job);
                }
            }
            tokio::select! {
                _ = self.ready.notified() => {}
                _ = self.cancel.cancelled() => return None,
            }
        }
    }
}

#[cfg(all(test, feature = "pipeline"))]
mod pre_check_queue_tests {
    use super::*;
    use ckb_test_chain_utils::always_success_cell;
    use ckb_types::{
        bytes::Bytes,
        core::{Capacity, TransactionBuilder},
        packed::{CellInput, CellOutput, OutPoint},
        prelude::*,
    };

    fn dummy_tx(input: &OutPoint, output_capacity: usize) -> TransactionView {
        let (_, _, always_success_script) = always_success_cell();
        TransactionBuilder::default()
            .input(CellInput::new(input.clone(), 0))
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(output_capacity).unwrap())
                    .lock(always_success_script.clone())
                    .build(),
            )
            .output_data(Bytes::default().pack())
            .build()
    }

    #[test]
    fn remove_by_id_and_peer() {
        use ckb_types::{h256, prelude::Pack};
        let cancel = ckb_stop_handler::CancellationToken::new();
        let queue = PreCheckQueue::new(cancel);
        let input = OutPoint::new(
            h256!("0x0101010101010101010101010101010101010101010101010101010101010101").pack(),
            0,
        );

        let tx_a = dummy_tx(&input, 1_000);
        let tx_b = dummy_tx(&OutPoint::new(tx_a.hash(), 0), 500);
        let tx_c = dummy_tx(&OutPoint::new(tx_b.hash(), 0), 400);

        queue
            .push(PreCheckJob {
                tx: tx_a.clone(),
                is_proposal_tx: false,
                remote: Some((0, 1.into())),
            })
            .unwrap();
        queue
            .push(PreCheckJob {
                tx: tx_b.clone(),
                is_proposal_tx: false,
                remote: Some((0, 2.into())),
            })
            .unwrap();
        queue
            .push(PreCheckJob {
                tx: tx_c.clone(),
                is_proposal_tx: false,
                remote: Some((0, 1.into())),
            })
            .unwrap();

        assert!(queue.contains_key(&tx_b.proposal_short_id()));
        assert_eq!(
            queue.remove_by_id(&tx_b.proposal_short_id()),
            Some(tx_b.clone())
        );
        assert!(!queue.contains_key(&tx_b.proposal_short_id()));
        assert!(queue.contains_key(&tx_a.proposal_short_id()));
        assert!(queue.contains_key(&tx_c.proposal_short_id()));

        let removed = queue.remove_by_peer(&1.into());
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().any(|tx| tx.hash() == tx_a.hash()));
        assert!(removed.iter().any(|tx| tx.hash() == tx_c.hash()));
        assert!(queue.get_tx(&tx_a.proposal_short_id()).is_none());
        assert!(queue.get_tx(&tx_c.proposal_short_id()).is_none());
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

    pub(crate) async fn fetch_tx_verify_cache(&self, tx: &TransactionView) -> Option<CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        guard.peek(&tx.witness_hash()).cloned()
    }

    #[cfg(not(feature = "pipeline"))]
    async fn fetch_txs_verify_cache(
        &self,
        txs: impl Iterator<Item = &TransactionView>,
    ) -> HashMap<Byte32, CacheEntry> {
        let guard = self.txs_verify_cache.read().await;
        txs.filter_map(|tx| {
            let wtx_hash = tx.witness_hash();
            guard
                .peek(&wtx_hash)
                .cloned()
                .map(|value| (wtx_hash, value))
        })
        .collect()
    }

    pub(crate) async fn submit_entry(
        &self,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        mut status: TxStatus,
    ) -> (Result<(), Reject>, Arc<Snapshot>) {
        #[cfg(feature = "pipeline")]
        let (conflict_inputs, early_snapshot) =
            self.find_conflict_inputs(entry.transaction()).await;

        // If a higher-fee RBF candidate appeared while this tx was waiting in
        // the verify queue, abort before replacing anything.  This prevents a
        // lower-fee candidate from front-running a higher-fee one.
        #[cfg(feature = "pipeline")]
        {
            if !conflict_inputs.is_empty() {
                let id = entry.proposal_short_id();
                let fee = entry.fee;
                if self
                    .rbf_candidates
                    .read()
                    .await
                    .is_superseded(&id, fee, &conflict_inputs)
                {
                    self.rbf_candidates.write().await.remove(&id);
                    return (
                        Err(Reject::RBFRejected(
                            "superseded by higher-fee in-flight candidate".to_string(),
                        )),
                        early_snapshot,
                    );
                }
            }
        }

        #[cfg(feature = "pipeline")]
        let entry_id = entry.proposal_short_id();
        #[cfg(feature = "pipeline")]
        let entry_id_for_cleanup = entry_id.clone();

        // Separate the successful result from the collected reject events and
        // recovered txs. Reject callbacks must be dispatched and displaced txs
        // must be recovered even if the closure returns an error after
        // `process_rbf` has already removed old transactions (e.g. the
        // replacement fails the pool ancestor/size limits). Without this, a
        // remote peer can evict in-pool txs via a crafted RBF replacement that
        // is itself rejected, leaving the node with neither transaction.
        let ((result, recovered, reject_events), snapshot) = self
            .with_tx_pool_write_lock(move |tx_pool, snapshot| {
                let mut reject_events = Vec::new();
                let mut recovered = Vec::new();

                let mut removed_old_txs = Vec::new();
                let result = (|| -> Result<(), Reject> {
                    // check_rbf must be invoked in `write` lock to avoid concurrent issues.
                    let conflicts = if tx_pool.enable_rbf() {
                        tx_pool.check_rbf(&snapshot, &entry)?
                    } else {
                        // RBF is disabled but we found conflicts, return error here
                        // after_process will put this tx into conflicts_pool
                        let conflicted_outpoint =
                            tx_pool.pool_map.find_conflict_outpoint(entry.transaction());
                        if let Some(outpoint) = conflicted_outpoint {
                            return Err(Reject::Resolve(OutPointError::Dead(outpoint)));
                        }
                        HashSet::new()
                    };

                    // if snapshot changed by context switch we need redo time_relative verify
                    let tip_hash = snapshot.tip_hash();
                    if pre_resolve_tip != tip_hash {
                        debug!(
                            "submit_entry {} context changed. previous:{} now:{}",
                            entry.proposal_short_id(),
                            pre_resolve_tip,
                            tip_hash
                        );

                        // destructuring assignments are not currently supported
                        status = check_rtx(tx_pool, &snapshot, &entry.rtx)?;

                        let tip_header = snapshot.tip_header();
                        let tx_env = status.with_env(tip_header);
                        time_relative_verify(snapshot, Arc::clone(&entry.rtx), tx_env)?;
                    }

                    removed_old_txs =
                        self.process_rbf(tx_pool, &entry, &conflicts, &mut reject_events);

                    // Txs whose inputs are not consumed by the new tx can be
                    // recovered immediately, regardless of whether the new tx
                    // ultimately succeeds.
                    let mut available_inputs: HashSet<OutPoint> = HashSet::new();
                    available_inputs.extend(
                        removed_old_txs
                            .iter()
                            .flat_map(|removed| removed.transaction().input_pts_iter()),
                    );
                    for input in entry.transaction().input_pts_iter() {
                        available_inputs.remove(&input);
                    }
                    recovered.extend(
                        tx_pool.get_conflicted_txs_from_inputs(available_inputs.into_iter()),
                    );

                    // Parents must be recovered before children so that the
                    // ordered resolver can re-resolve and accept them in the
                    // correct order.
                    #[cfg(feature = "pipeline")]
                    Self::sort_txs_by_dependencies(&mut recovered);

                    let evicted = _submit_entry(tx_pool, status, &entry, &self.callbacks)?;

                    // in a corner case, a tx with lower fee rate may be rejected immediately
                    // after inserting into pool, return proper reject error here
                    for evict in evicted {
                        let reject = Reject::Invalidated(format!(
                            "invalidated by tx {}",
                            evict.transaction().hash()
                        ));
                        reject_events.push((evict, reject));
                    }

                    tx_pool.remove_conflict(&entry.proposal_short_id());
                    tx_pool
                        .limit_size(Some(&entry.proposal_short_id()), &mut reject_events)
                        .map_or(Ok(()), Err)?;

                    Ok(())
                })();

                // If the replacement was rejected after `process_rbf` removed the
                // old conflicting transactions, the new tx's inputs are free
                // again. Recover any txs stored in the conflict pool for those
                // inputs (in particular the old tx itself) so that a failed
                // RBF attempt cannot be used to evict in-pool transactions.
                //
                // IMPORTANT: exclude the entry transaction itself from recovery.
                // It was just rejected by RBF; re-enqueueing it would cause a
                // cycle where both the entry and the in-pool tx keep being
                // recovered and failing RBF against each other indefinitely.
                if result.is_err() {
                    let entry_id_clone = entry_id.clone();
                    recovered.extend(
                        tx_pool
                            .get_conflicted_txs_from_inputs(entry.transaction().input_pts_iter())
                            .into_iter()
                            .filter(|tx| tx.proposal_short_id() != entry_id_clone),
                    );
                    for tx in &recovered {
                        tx_pool.remove_conflict(&tx.proposal_short_id());
                    }
                    for old in removed_old_txs {
                        tx_pool.remove_conflict(&old.proposal_short_id());
                    }
                }

                (result, recovered, reject_events)
            })
            .await;

        // Dispatch reject callbacks outside the write lock, regardless of
        // whether the submission itself succeeded.
        for (entry, reject) in reject_events {
            self.callbacks.call_reject(&entry, reject);
        }

        // Send recovered txs to the deferred worker after the write lock is
        // released. Use .send().await rather than try_send so that recovery
        // txs are never silently dropped under high RBF frequency.
        // Recovery is attempted even if the replacement ultimately failed,
        // because the old conflicting txs have already been removed from the
        // pool and may now be valid again.
        if !recovered.is_empty()
            && let Err(e) = self
                .deferred_sender
                .send(crate::service::DeferredTask::RecoverTxs(recovered))
                .await
        {
            warn!("failed to enqueue recovered txs for re-processing: {}", e);
        }

        // The RBF candidate has either been accepted or definitively rejected;
        // remove it from the in-flight fee-ordering gate.
        #[cfg(feature = "pipeline")]
        self.rbf_candidates
            .write()
            .await
            .remove(&entry_id_for_cleanup);

        (result, snapshot)
    }

    pub(crate) async fn notify_block_assembler(&self, status: TxStatus) {
        if self.should_notify_block_assembler() {
            let message = match status {
                TxStatus::Fresh => Some(BlockAssemblerMessage::Pending),
                TxStatus::Proposed => Some(BlockAssemblerMessage::Proposed),
                _ => None,
            };

            if let Some(message) = message
                && self.block_assembler_sender.send(message).await.is_err()
            {
                error!("block_assembler receiver dropped");
            }
        }
    }

    // Remove conflicting transactions for RBF and record them in the conflicts
    // cache so they can be recovered if the replacement fails. Returns the set
    // of removed entries; the caller decides which ones to recover and when to
    // clean up the conflicts cache.
    fn process_rbf(
        &self,
        tx_pool: &mut TxPool,
        entry: &TxEntry,
        conflicts: &HashSet<ProposalShortId>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) -> Vec<TxEntry> {
        if conflicts.is_empty() {
            return Vec::new();
        }

        let all_removed: Vec<_> = conflicts
            .iter()
            .flat_map(|id| tx_pool.pool_map.remove_entry_and_descendants(id))
            .collect();

        for old in &all_removed {
            debug!(
                "remove conflict tx {} for RBF by new tx {}",
                old.transaction().hash(),
                entry.transaction().hash()
            );
            let reject =
                Reject::RBFRejected(format!("replaced by tx {}", entry.transaction().hash()));

            // collect reject events for dispatch outside write lock
            reject_events.push((old.clone(), reject));
        }

        // Record every removed entry (direct conflicts and their descendants)
        // in the conflicts cache so that they can all be recovered if the
        // replacement fails or if their inputs become available again.
        for old in &all_removed {
            tx_pool.record_conflict(old.transaction().clone());
        }

        all_removed
    }

    pub(crate) async fn verify_queue_contains(&self, tx: &TransactionView) -> bool {
        let queue = self.verify_queue.read().await;
        queue.contains_key(&tx.proposal_short_id())
    }

    pub(crate) async fn orphan_contains(&self, tx: &TransactionView) -> bool {
        let orphan = self.orphan.read().await;
        orphan.contains_key(&tx.proposal_short_id())
    }

    pub(crate) async fn with_tx_pool_read_lock<U, F: FnMut(&TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let tx_pool = self.tx_pool.read().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    /// Find the transaction inputs that are currently consumed by in-pool txs.
    /// These are the "conflict inputs" that matter for RBF ordering.
    #[cfg(feature = "pipeline")]
    pub(crate) async fn find_conflict_inputs(
        &self,
        tx: &TransactionView,
    ) -> (Vec<OutPoint>, Arc<Snapshot>) {
        self.with_tx_pool_read_lock(|tx_pool, _snapshot| {
            tx.input_pts_iter()
                .filter(|out_point| tx_pool.pool_map.edges.get_input_ref(out_point).is_some())
                .collect()
        })
        .await
    }

    pub(crate) async fn with_tx_pool_write_lock<U, F: FnMut(&mut TxPool, Arc<Snapshot>) -> U>(
        &self,
        mut f: F,
    ) -> (U, Arc<Snapshot>) {
        let mut tx_pool = self.tx_pool.write().await;
        let snapshot = tx_pool.cloned_snapshot();

        let ret = f(&mut tx_pool, Arc::clone(&snapshot));
        (ret, snapshot)
    }

    pub(crate) async fn pre_check(
        &self,
        tx: &TransactionView,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let tx_size = tx.data().serialized_size_in_block();

        // Fast path: for transactions whose inputs and cell deps all come from the
        // chain (not from any tx currently in the pool), we can resolve and compute
        // the fee without holding the tx_pool read lock.  We only take the lock
        // briefly to check for txid collisions.
        let (collision, snapshot) = self
            .with_tx_pool_read_lock(|tx_pool, _snapshot| check_txid_collision(tx_pool, tx).err())
            .await;
        if let Some(reject) = collision {
            return (Err(reject), snapshot);
        }

        let short_id = tx.proposal_short_id();
        let mut seen_inputs = HashSet::new();
        match resolve_transaction(
            tx.clone(),
            &mut seen_inputs,
            snapshot.as_ref(),
            snapshot.as_ref(),
        ) {
            Ok(rtx) => {
                let rtx = Arc::new(rtx);
                let fee = match check_tx_fee_with_min_fee_rate(
                    &snapshot,
                    &rtx,
                    tx_size,
                    self.tx_pool_config.min_fee_rate,
                ) {
                    Ok(fee) => fee,
                    Err(reject) => return (Err(reject), snapshot),
                };
                let status = get_tx_status(&snapshot, &short_id);
                (
                    Ok((snapshot.tip_hash(), rtx, status, fee, tx_size)),
                    snapshot,
                )
            }
            Err(OutPointError::Unknown(_)) => {
                // At least one input/cell dep is not in the chain snapshot.  It may
                // be an output of a tx currently in the pool, so fall back to the
                // locked path which can resolve through the pool.
                self.pre_check_with_pool_lock(tx, tx_size).await
            }
            Err(err) => (Err(Reject::Resolve(err)), snapshot),
        }
    }

    async fn pre_check_with_pool_lock(
        &self,
        tx: &TransactionView,
        tx_size: usize,
    ) -> (Result<PreCheckedTx, Reject>, Arc<Snapshot>) {
        let (ret, snapshot) = self
            .with_tx_pool_read_lock(|tx_pool, snapshot| {
                let tip_hash = snapshot.tip_hash();

                // Same txid means exactly the same transaction, including inputs, outputs, witnesses, etc.
                // It's also not possible for RBF, reject it directly
                check_txid_collision(tx_pool, tx)?;

                // Try normal path first, if double-spending check success we don't need RBF check
                // this make sure RBF won't introduce extra performance cost for hot path
                let res = resolve_tx(tx_pool, &snapshot, tx.clone(), false);
                match res {
                    Ok((rtx, status)) => {
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        Ok((tip_hash, rtx, status, fee, tx_size))
                    }
                    Err(Reject::Resolve(OutPointError::Dead(out))) => {
                        let (rtx, status) = resolve_tx(tx_pool, &snapshot, tx.clone(), true)?;
                        let fee = check_tx_fee(tx_pool, &snapshot, &rtx, tx_size)?;
                        let conflicts = tx_pool.pool_map.find_conflict_outpoint(tx);
                        if conflicts.is_none() {
                            // this mean one input's outpoint is dead, but there is no direct conflicted tx in tx_pool
                            // we should reject it directly and don't need to put it into conflicts pool
                            error!(
                                "{} is resolved as Dead, but there is no direct conflicted tx",
                                rtx.transaction.proposal_short_id()
                            );
                            return Err(Reject::Resolve(OutPointError::Dead(out)));
                        }
                        // we also return Ok here, so that the entry will be continue to be verified before submit
                        // we only want to put it into conflicts pool after the verification stage passed
                        // then we will double-check conflicts txs in `submit_entry`

                        Ok((tip_hash, rtx, status, fee, tx_size))
                    }
                    Err(err) => Err(err),
                }
            })
            .await;
        (ret, snapshot)
    }

    pub(crate) async fn non_contextual_verify(
        &self,
        tx: &TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<(), Reject> {
        if let Err(reject) = non_contextual_verify(&self.consensus, tx) {
            if reject.is_malformed_tx()
                && let Some(remote) = remote
            {
                self.ban_malformed(remote.1, format!("reject {reject}"))
                    .await;
            }
            return Err(reject);
        }
        Ok(())
    }

    pub(crate) async fn resumeble_process_tx(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        // non contextual verify first
        self.non_contextual_verify(&tx, remote).await?;

        if self.orphan_contains(&tx).await {
            debug!("reject tx {} already in orphan pool", tx.hash());
            return Err(Reject::Duplicated(tx.hash()));
        }

        if self.verify_queue_contains(&tx).await {
            return Err(Reject::Duplicated(tx.hash()));
        }

        #[cfg(feature = "pipeline")]
        {
            self.classify_and_enqueue_tx_spawn(tx, is_proposal_tx, remote)
                .await
        }

        #[cfg(not(feature = "pipeline"))]
        {
            // Synchronous fallback used for benchmarking the pre-pipeline baseline.
            let _ = is_proposal_tx;
            if let Some((ret, snapshot)) = self
                ._process_tx(tx.clone(), remote.map(|r| r.0), None)
                .await
            {
                self.after_process(tx, remote, &snapshot, &ret).await;
                ret.map(|_| true)
            } else {
                Ok(true)
            }
        }
    }

    async fn resumeble_process_tx_and_notify_full_reject(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        let tx_hash = tx.hash();
        let ret = self.resumeble_process_tx(tx, is_proposal_tx, remote).await;

        if matches!(ret, Err(Reject::Full(_))) {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash });
        }

        ret
    }

    pub(crate) async fn submit_remote_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<bool, Reject> {
        self.resumeble_process_tx_and_notify_full_reject(tx, false, Some((declared_cycles, peer)))
            .await
    }

    pub(crate) async fn notify_tx(&self, tx: TransactionView) -> Result<bool, Reject> {
        self.resumeble_process_tx_and_notify_full_reject(tx, true, None)
            .await
    }

    pub(crate) async fn test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        // non contextual verify first
        self.non_contextual_verify(&tx, None).await?;

        if self.verify_queue_contains(&tx).await {
            return Err(Reject::Duplicated(tx.hash()));
        }

        if self.orphan_contains(&tx).await {
            debug!("reject tx {} already in orphan pool", tx.hash());
            return Err(Reject::Duplicated(tx.hash()));
        }
        self._test_accept_tx(tx.clone()).await
    }

    pub(crate) async fn process_tx(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<Completed, Reject> {
        // non contextual verify first
        self.non_contextual_verify(&tx, remote).await?;

        if self.verify_queue_contains(&tx).await || self.orphan_contains(&tx).await {
            return Err(Reject::Duplicated(tx.hash()));
        }

        if let Some((ret, snapshot)) = self
            ._process_tx(tx.clone(), remote.map(|r| r.0), None)
            .await
        {
            self.after_process(tx, remote, &snapshot, &ret).await;
            ret
        } else {
            // currently, the returned cycles is not been used, mock 0 if delay
            Ok(Completed {
                cycles: 0,
                fee: Capacity::zero(),
            })
        }
    }

    pub(crate) fn put_recent_reject(&self, tx_hash: &Byte32, reject: &Reject) {
        if let Some(ref recent_reject) = self.recent_reject
            && let Err(e) = recent_reject.put(tx_hash, reject.clone())
        {
            error!(
                "Failed to record recent_reject {} {} {}",
                tx_hash, reject, e
            );
        }
    }

    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        #[cfg(feature = "pipeline")]
        {
            if self.pre_check_queue.remove_by_id(&id).is_some() {
                return true;
            }
        }
        {
            let mut queue = self.ordered_resolve_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                return true;
            }
        }
        {
            let mut queue = self.verify_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                // Release verify_queue write lock before acquiring other locks
                // to respect the documented lock ordering convention.
                drop(queue);
                // The removed tx may have had descendants waiting in the
                // ordered resolve queue. Wake the resolver so they can be
                // retried (and rejected if the parent is gone) promptly.
                #[cfg(feature = "pipeline")]
                self.rbf_candidates.write().await.remove(&id);
                let ordered = self.ordered_resolve_queue.read().await;
                if !ordered.is_empty() {
                    ordered.subscribe().notify_one();
                }
                return true;
            }
        }
        {
            let mut orphan = self.orphan.write().await;
            if orphan.remove_orphan_tx(&id).is_some() {
                return true;
            }
        }
        let removed = {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.remove_tx(&id)
        };
        if removed {
            let ordered = self.ordered_resolve_queue.read().await;
            if !ordered.is_empty() {
                ordered.subscribe().notify_one();
            }
        }
        removed
    }

    pub(crate) async fn after_process(
        &self,
        tx: TransactionView,
        remote: Option<(Cycle, PeerIndex)>,
        _snapshot: &Snapshot,
        ret: &Result<Completed, Reject>,
    ) {
        let tx_hash = tx.hash();

        // log tx verification result for monitor node
        if log_enabled_target!("ckb_tx_monitor", Trace)
            && let Ok(c) = ret
        {
            trace_target!(
                "ckb_tx_monitor",
                r#"{{"tx_hash":"{:#x}","cycles":{}}}"#,
                tx_hash,
                c.cycles
            );
        }

        if matches!(
            ret,
            Err(Reject::RBFRejected(..) | Reject::Resolve(OutPointError::Dead(_)))
        ) {
            let mut tx_pool = self.tx_pool.write().await;
            if tx_pool.pool_map.find_conflict_outpoint(&tx).is_some() {
                tx_pool.record_conflict(tx.clone());
            }
        }

        match remote {
            Some((declared_cycle, peer)) => match ret {
                Ok(_) => {
                    debug!(
                        "after_process remote send_result_to_relayer {} {}",
                        tx_hash, peer
                    );
                    self.handle_verify_success(&tx, Some(peer)).await;
                }
                Err(reject) => {
                    debug!(
                        "after_process {} {} remote reject: {} ",
                        tx_hash, peer, reject
                    );
                    if is_missing_input(reject) {
                        let parents = tx.unique_parents();
                        self.handle_missing_input_orphan(tx, peer, declared_cycle, parents)
                            .await;
                    } else {
                        self.handle_remote_reject(&tx_hash, reject, peer).await;
                    }
                }
            },
            None => {
                match ret {
                    Ok(_) | Err(Reject::Duplicated(_)) => {
                        if matches!(ret, Err(Reject::Duplicated(_))) {
                            debug!("after_process {} duplicated", tx_hash);
                        } else {
                            debug!("after_process local send_result_to_relayer {}", tx_hash);
                        }
                        // Re-broadcast tx when it's duplicated and submitted
                        // through local rpc, or notify on fresh success.
                        self.handle_verify_success(&tx, None).await;
                    }
                    Err(reject) => {
                        debug!("after_process {} reject: {} ", tx_hash, reject);
                        if reject.should_recorded() {
                            self.put_recent_reject(&tx_hash, reject);
                        }
                    }
                }
            }
        }
    }

    /// Common success handler: relay the result and trigger orphan processing.
    ///
    /// Box::pin is required because after_process and process_orphan_tx are
    /// mutually recursive async fns; without boxing the compiler cannot prove
    /// the resulting future has a finite size.
    async fn handle_verify_success(&self, tx: &TransactionView, original_peer: Option<PeerIndex>) {
        self.send_result_to_relayer(TxVerificationResult::Ok {
            original_peer,
            tx_hash: tx.hash(),
        });
        Box::pin(self.process_orphan_tx(tx)).await;
    }

    /// Post-processing for a rejected remote transaction: ban the peer if the
    /// tx is malformed, relay the rejection if allowed, and record it in the
    /// recent-reject database if applicable.
    ///
    /// This is the single source of truth for the "remote error triple" used
    /// by both [`Self::after_process`] and [`Self::process_orphan_tx`].
    pub(crate) async fn handle_remote_reject(
        &self,
        tx_hash: &Byte32,
        reject: &Reject,
        peer: PeerIndex,
    ) {
        if reject.is_malformed_tx() {
            self.ban_malformed(peer, format!("reject {reject}")).await;
        }
        if reject.is_allowed_relay() {
            self.send_result_to_relayer(TxVerificationResult::Reject {
                tx_hash: tx_hash.clone(),
            });
        }
        if reject.should_recorded() {
            self.put_recent_reject(tx_hash, reject);
        }
    }

    /// Route a transaction with missing inputs to the orphan pool and notify
    /// the relayer about the missing parents.
    ///
    /// Used by both [`Self::after_process`] (which computes parents from the
    /// tx) and the ordered resolver (which receives parents from the resolve
    /// stage result).
    pub(crate) async fn handle_missing_input_orphan(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        declared_cycle: Cycle,
        parents: HashSet<Byte32>,
    ) {
        // Only notify the relayer after the tx has actually been accepted into
        // the orphan pool. This avoids telling peers about missing parents for
        // a tx that we end up dropping (e.g. duplicate orphan or pool full).
        if self.add_orphan(tx, peer, declared_cycle).await {
            self.send_result_to_relayer(TxVerificationResult::UnknownParents { peer, parents });
        }
    }

    pub(crate) async fn add_orphan(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        declared_cycle: Cycle,
    ) -> bool {
        let (added, evicted_txs) =
            self.orphan
                .write()
                .await
                .add_orphan_tx(tx, peer, declared_cycle);
        // for any evicted orphan tx, we should send reject to relayer
        // so that we mark it as `unknown` in filter
        for tx_hash in evicted_txs {
            self.send_result_to_relayer(TxVerificationResult::Reject { tx_hash });
        }
        added
    }

    pub(crate) async fn find_orphan_by_previous(&self, tx: &TransactionView) -> Vec<OrphanEntry> {
        let orphan = self.orphan.read().await;
        orphan
            .find_by_previous(tx)
            .iter()
            .filter_map(|id| orphan.get(id).cloned())
            .collect::<Vec<_>>()
    }

    pub(crate) async fn remove_orphan_tx(&self, id: &ProposalShortId) {
        self.orphan.write().await.remove_orphan_tx(id);
    }

    /// Remove all orphans which are resolved by the given transaction.
    ///
    /// The search is breadth-first: each orphan is routed through the same
    /// pipeline entry point as other remote transactions. When an orphan is
    /// eventually verified and submitted, `after_process` will recursively
    /// process its own descendants in the orphan pool.
    pub(crate) async fn process_orphan_tx(&self, tx: &TransactionView) {
        let mut orphan_queue: VecDeque<TransactionView> = VecDeque::new();
        orphan_queue.push_back(tx.clone());

        while let Some(previous) = orphan_queue.pop_front() {
            let orphans = self.find_orphan_by_previous(&previous).await;
            for orphan in orphans.into_iter() {
                let orphan_id = orphan.tx.proposal_short_id();

                #[cfg(feature = "pipeline")]
                {
                    match self
                        .classify_and_enqueue_tx(
                            orphan.tx.clone(),
                            false,
                            Some((orphan.cycle, orphan.peer)),
                        )
                        .await
                    {
                        Ok(_) => {
                            self.remove_orphan_tx(&orphan_id).await;
                            // The orphan is now in the pipeline. Its own children
                            // will be processed once it successfully submits via
                            // the normal `after_process` -> `handle_verify_success`
                            // path, so we do not need to push it back here.
                        }
                        Err(reject) => {
                            // Keep the orphan if the only problem is that its
                            // parents are not yet available or the pipeline queues
                            // are temporarily full.  For any other reject reason
                            // (malformed, low fee, etc.) remove it and notify the
                            // peer.
                            if crate::util::is_missing_input(&reject)
                                || matches!(reject, Reject::Full(_))
                            {
                                warn!(
                                    "process_orphan {} not ready ({reject}); keeping orphan from {}",
                                    orphan.tx.hash(),
                                    tx.hash(),
                                );
                            } else {
                                self.remove_orphan_tx(&orphan_id).await;
                                self.handle_remote_reject(&orphan.tx.hash(), &reject, orphan.peer)
                                    .await;
                            }
                        }
                    }
                }

                #[cfg(not(feature = "pipeline"))]
                {
                    if let Some((ret, snapshot)) = self
                        ._process_tx(orphan.tx.clone(), Some(orphan.cycle), None)
                        .await
                    {
                        let remote = Some((orphan.cycle, orphan.peer));
                        let keep = matches!(&ret, Err(reject) if crate::util::is_missing_input(reject) || matches!(reject, Reject::Full(_)));
                        if !keep {
                            self.remove_orphan_tx(&orphan_id).await;
                        }
                        // after_process handles remote reject notifications
                        // internally; do NOT call handle_remote_reject here to
                        // avoid double ban/relay/recent_reject.
                        self.after_process(orphan.tx, remote, &snapshot, &ret).await;
                    }
                }
            }
        }
    }

    pub(crate) fn send_result_to_relayer(&self, result: TxVerificationResult) {
        if let Err(e) = self.tx_relay_sender.send(result) {
            error!("tx-pool tx_relay_sender internal error {}", e);
        }
    }

    async fn ban_malformed(&self, peer: PeerIndex, reason: String) {
        const DEFAULT_BAN_TIME: Duration = Duration::from_secs(3600 * 24 * 3);

        #[cfg(feature = "with_sentry")]
        use sentry::{Level, capture_message, with_scope};

        #[cfg(feature = "with_sentry")]
        with_scope(
            |scope| scope.set_fingerprint(Some(&["ckb-tx-pool", "receive-invalid-remote-tx"])),
            || {
                capture_message(
                    &format!(
                        "Ban peer {} for {} seconds, reason: \
                        {}",
                        peer,
                        DEFAULT_BAN_TIME.as_secs(),
                        reason
                    ),
                    Level::Info,
                )
            },
        );
        self.network.ban_peer(peer, DEFAULT_BAN_TIME, reason);
        self.ordered_resolve_queue
            .write()
            .await
            .remove_txs_by_peer(&peer);
        let removed_ids = self.verify_queue.write().await.remove_txs_by_peer(&peer);
        // Remove orphan txs from the banned peer so they are not re-processed
        // after the ban.
        {
            let mut orphan = self.orphan.write().await;
            let orphan_ids: Vec<_> = orphan
                .entries
                .iter()
                .filter(|(_, entry)| entry.peer == peer)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &orphan_ids {
                orphan.remove_orphan_tx(id);
            }
        }
        #[cfg(feature = "pipeline")]
        {
            self.pre_check_queue.remove_by_peer(&peer);
            let mut rbf = self.rbf_candidates.write().await;
            for id in removed_ids {
                rbf.remove(&id);
            }
        }
        #[cfg(not(feature = "pipeline"))]
        {
            let _ = removed_ids;
        }
    }

    pub(crate) async fn _process_tx(
        &self,
        tx: TransactionView,
        declared_cycles: Option<Cycle>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let (ret, snapshot) = self.pre_check(&tx).await;

        let (pre_resolve_tip, rtx, status, fee, tx_size) =
            try_or_return_with_snapshot!(ret, snapshot);

        self.verify_and_submit_core(
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            declared_cycles,
            command_rx,
        )
        .await
    }

    /// Verify and submit a transaction whose inputs have already been resolved.
    ///
    /// This is the second stage of the tx-pool pipeline: the resolver has
    /// already produced a [`ResolvedTx`], and this function runs the CPU-heavy
    /// contextual verification and the final write-locked submit.
    pub(crate) async fn _verify_and_submit_tx(
        &self,
        resolved: crate::resolved_tx::ResolvedTx,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let crate::resolved_tx::ResolvedTx {
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            remote,
            is_proposal_tx: _,
        } = resolved;

        let declared_cycles = remote.map(|(cycles, _)| cycles);

        self.verify_and_submit_core(
            tx,
            rtx,
            status,
            fee,
            tx_size,
            pre_resolve_tip,
            snapshot,
            declared_cycles,
            command_rx,
        )
        .await
    }

    /// Shared core: verify a resolved transaction and submit it to the pool.
    ///
    /// Both `_process_tx` (sync path) and `_verify_and_submit_tx` (pipeline
    /// path) converge here after the resolve step.
    #[allow(clippy::too_many_arguments)]
    async fn verify_and_submit_core(
        &self,
        tx: TransactionView,
        rtx: Arc<ResolvedTransaction>,
        status: TxStatus,
        fee: Capacity,
        tx_size: usize,
        pre_resolve_tip: Byte32,
        snapshot: Arc<Snapshot>,
        declared_cycles: Option<Cycle>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let wtx_hash = tx.witness_hash();
        let instant = Instant::now();
        let is_sync_process = command_rx.is_none();

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = declared_cycles.unwrap_or_else(|| self.consensus.max_block_cycles());
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status.with_env(tip_header));

        let verified_ret = verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            command_rx,
        )
        .await;

        let verified = try_or_return_with_snapshot!(verified_ret, snapshot);

        if let Some(declared) = declared_cycles
            && declared != verified.cycles
        {
            info!(
                "declared cycles not match verified cycles, declared: {}, verified: {}, tx_hash: {}",
                declared,
                verified.cycles,
                tx.hash()
            );
            return Some((
                Err(Reject::DeclaredWrongCycles(declared, verified.cycles)),
                snapshot,
            ));
        }

        let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);

        let (ret, submit_snapshot) = self.submit_entry(pre_resolve_tip, entry, status).await;
        try_or_return_with_snapshot!(ret, submit_snapshot);

        self.notify_block_assembler(status).await;

        // A newly submitted transaction may resolve dependent transactions that
        // are waiting in the ordered resolve queue (e.g. children of a parent
        // that was just re-added after a reorg). Wake the ordered resolver so
        // those children can be retried promptly.
        let queue = self.ordered_resolve_queue.read().await;
        if !queue.is_empty() {
            queue.subscribe().notify_one();
        }

        if verify_cache.is_none() {
            // Defer cache update to the background worker instead of
            // spawning a fire-and-forget task.
            if let Err(e) =
                self.deferred_sender
                    .try_send(crate::service::DeferredTask::CacheUpdate {
                        wtx_hash: wtx_hash.clone(),
                        verified,
                    })
            {
                warn!(
                    "failed to enqueue verify cache update for {}: {}",
                    wtx_hash, e
                );
            }
        }

        if let Some(metrics) = ckb_metrics::handle() {
            let elapsed = instant.elapsed().as_secs_f64();
            if is_sync_process {
                metrics.ckb_tx_pool_sync_process.observe(elapsed);
            } else {
                metrics.ckb_tx_pool_async_process.observe(elapsed);
            }
        }

        Some((Ok(verified), submit_snapshot))
    }

    pub(crate) async fn _test_accept_tx(&self, tx: TransactionView) -> Result<Completed, Reject> {
        let (pre_check_ret, snapshot) = self.pre_check(&tx).await;

        let (_tip_hash, rtx, status, _fee, _tx_size) = pre_check_ret?;

        // skip check the delay window

        let verify_cache = self.fetch_tx_verify_cache(&tx).await;
        let max_cycles = self.consensus.max_block_cycles();
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(status.with_env(tip_header));

        verify_rtx(
            Arc::clone(&snapshot),
            Arc::clone(&rtx),
            tx_env,
            &verify_cache,
            max_cycles,
            None,
        )
        .await
    }

    /// Topologically sort transactions so that parents are placed before their
    /// children. This is required when re-adding detached transactions into the
    /// pipeline: a child must not be classified before its parent has had a
    /// chance to enter the in-flight pipeline, otherwise it will be treated as a
    /// local orphan and have to wait for a retry.
    #[cfg(feature = "pipeline")]
    pub(crate) fn sort_txs_by_dependencies(txs: &mut Vec<TransactionView>) {
        if txs.len() <= 1 {
            return;
        }

        let mut output_to_index: HashMap<OutPoint, usize> =
            HashMap::with_capacity(txs.len().saturating_mul(2));
        for (i, tx) in txs.iter().enumerate() {
            let tx_hash = tx.hash();
            for idx in 0..tx.outputs().len() {
                output_to_index.insert(OutPoint::new(tx_hash.clone(), idx as u32), i);
            }
        }

        let mut in_degree = vec![0usize; txs.len()];
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); txs.len()];
        for (i, tx) in txs.iter().enumerate() {
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

        let mut ready: VecDeque<usize> = (0..txs.len()).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted = Vec::with_capacity(txs.len());
        while let Some(i) = ready.pop_front() {
            sorted.push(i);
            for &child in &children[i] {
                in_degree[child] -= 1;
                if in_degree[child] == 0 {
                    ready.push_back(child);
                }
            }
        }

        if sorted.len() != txs.len() {
            // A cycle should never happen in valid detached blocks, but if it
            // does we keep the original order rather than losing transactions.
            return;
        }

        let mut remaining: Vec<Option<TransactionView>> = txs.drain(..).map(Some).collect();
        txs.extend(
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
            self.fee_estimator.commit_block(&blk);
            attached.extend(blk.transactions().into_iter().skip(1));
        }
        let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        // In non-pipeline mode, pre-fetch verify cache for batch re-verification.
        // In pipeline mode, caching is handled per-tx by verify_and_submit_core
        // using the correct wtx_hash key.
        #[cfg(not(feature = "pipeline"))]
        let fetched_cache = self.fetch_txs_verify_cache(retain.iter()).await;

        let reject_events;
        {
            // This closure is used to limit the lifetime of mutable tx_pool.
            let mut tx_pool = self.tx_pool.write().await;

            reject_events = _update_tx_pool_for_reorg(
                &mut tx_pool,
                &attached,
                &detached_headers,
                detached_proposal_id,
                snapshot,
                &self.callbacks,
                mine_mode,
            );

            // Non-pipeline path: re-verify detached txs inline (write lock held).
            #[cfg(not(feature = "pipeline"))]
            self.readd_detached_tx(&mut tx_pool, retain, fetched_cache)
                .await;
        }

        // Dispatch reject callbacks outside the write lock
        for (entry, reject) in reject_events {
            self.callbacks.call_reject(&entry, reject);
        }

        self.remove_orphan_txs_by_attach(&attached).await;
        {
            let mut queue = self.verify_queue.write().await;
            queue.remove_txs(attached.iter().map(|tx| tx.proposal_short_id()));
        }

        // Pipeline path: recover detached transactions through the synchronous
        // per-tx entry point.  Dependent transactions must be processed after
        // their parents have already been submitted to the pool; a topological
        // sort guarantees this ordering.  Using `_process_tx` here instead of
        // the general pipeline keeps the recovery logic simple and correct while
        // still releasing the tx-pool write lock between transactions.
        #[cfg(feature = "pipeline")]
        {
            let mut retain = retain;
            Self::sort_txs_by_dependencies(&mut retain);
            let mut chunk_rx = self.chunk_rx.clone();
            for tx in retain {
                if let Some((ret, snapshot)) = self
                    ._process_tx(tx.clone(), None, Some(&mut chunk_rx))
                    .await
                    && let Err(ref reject) = ret
                {
                    debug!("reorg re-add failed: {}", reject);
                    self.after_process(tx, None, &snapshot, &ret).await;
                } else {
                    // The detached tx is now back in the pool. Wake up any
                    // orphans that depend on it (including via cell dep).
                    self.process_orphan_tx(&tx).await;
                }
            }
        }
    }

    /// Check if a transaction depends on any in-flight pipeline transaction.
    /// If so, route it to the ordered resolve queue.
    ///
    /// Returns:
    /// - `Ok(Some(true))` — tx was dependent and successfully enqueued
    /// - `Ok(Some(false))` — tx was dependent but is a duplicate
    /// - `Ok(None)` — tx is independent, caller should proceed with pre_check
    #[cfg(feature = "pipeline")]
    async fn check_and_route_dependent(
        &self,
        tx: &TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<Option<bool>, Reject> {
        let id = tx.proposal_short_id();
        let depends_on_pipeline = {
            let ordered = self.ordered_resolve_queue.read().await;
            if ordered.depends_on(tx) {
                true
            } else {
                drop(ordered);
                let verify_queue = self.verify_queue.read().await;
                if verify_queue.depends_on(tx) {
                    true
                } else {
                    self.pre_check_queue.depends_on(tx)
                }
            }
        };

        if depends_on_pipeline {
            let mut ordered = self.ordered_resolve_queue.write().await;
            if ordered.contains_key(&id) {
                return Ok(Some(false));
            }
            return ordered
                .add_tx(crate::resolved_tx::ResolveJob {
                    tx: tx.clone(),
                    remote,
                    is_proposal_tx,
                    attempts: 0,
                })
                .map(Some);
        }

        Ok(None)
    }

    /// Classify a transaction and enqueue it for verification or ordered resolve.
    ///
    /// This is the core entry-point classifier.  It checks whether the tx
    /// depends on an in-flight pipeline tx, runs `pre_check`, and routes the
    /// result to the appropriate queue.
    #[cfg(feature = "pipeline")]
    pub(crate) async fn classify_and_enqueue_tx(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        let id = tx.proposal_short_id();

        if let Some(routed) = self
            .check_and_route_dependent(&tx, is_proposal_tx, remote)
            .await?
        {
            return Ok(routed);
        }

        // Run pre_check once at the entry point.
        let (pre_check_ret, snapshot) = self.pre_check(&tx).await;

        match pre_check_ret {
            Ok((pre_resolve_tip, rtx, status, fee, tx_size)) => {
                // For RBF replacements, register the candidate before it enters
                // the verify queue so lower-fee candidates can be rejected while
                // a higher-fee candidate is already in flight.
                let conflict_inputs = if remote.is_some() {
                    self.find_conflict_inputs(&tx).await.0
                } else {
                    Vec::new()
                };
                let rbf_registered = !conflict_inputs.is_empty();
                if rbf_registered {
                    let mut rbf = self.rbf_candidates.write().await;
                    match rbf.register(id.clone(), fee, &conflict_inputs) {
                        Ok(displaced_ids) => {
                            // Higher-fee candidate(s) displaced lower-fee one(s)
                            // still waiting in the verify queue.  Drop all
                            // displaced candidates so only the highest-fee tx
                            // reaches submit_entry.
                            if !displaced_ids.is_empty() {
                                drop(rbf);
                                let mut verify_queue = self.verify_queue.write().await;
                                for displaced_id in &displaced_ids {
                                    verify_queue.remove_tx(displaced_id);
                                }
                            }
                        }
                        Err(reason) => {
                            drop(rbf);
                            let reject = Reject::RBFRejected(reason);
                            self.after_process(tx, remote, &snapshot, &Err(reject.clone()))
                                .await;
                            return Err(reject);
                        }
                    }
                }

                let resolved = crate::resolved_tx::ResolvedTx {
                    tx: tx.clone(),
                    rtx,
                    status,
                    fee,
                    tx_size,
                    pre_resolve_tip,
                    snapshot: Arc::clone(&snapshot),
                    remote,
                    is_proposal_tx,
                };
                let reject = {
                    let mut verify_queue = self.verify_queue.write().await;
                    match verify_queue.add_tx(resolved) {
                        Ok(added) => return Ok(added),
                        Err(reject) => reject,
                    }
                };
                // The verify queue rejected the tx (e.g., it is full). Clean up
                // the RBF registration so the input is not blocked forever.
                if rbf_registered {
                    self.rbf_candidates.write().await.remove(&id);
                }
                self.after_process(tx, remote, &snapshot, &Err(reject.clone()))
                    .await;
                Err(reject)
            }
            Err(reject) if crate::util::is_missing_input(&reject) => {
                let mut ordered = self.ordered_resolve_queue.write().await;
                if ordered.contains_key(&id) {
                    return Ok(false);
                }
                ordered.add_tx(crate::resolved_tx::ResolveJob {
                    tx,
                    remote,
                    is_proposal_tx,
                    attempts: 0,
                })
            }
            Err(reject) => {
                self.after_process(tx, remote, &snapshot, &Err(reject.clone()))
                    .await;
                Err(reject)
            }
        }
    }

    /// Entry-point classifier used by remote/local submission.
    ///
    /// Dependent transactions (those that spend an output currently in flight)
    /// are handled synchronously so they land in the ordered resolve queue in
    /// arrival order and errors propagate to the caller.  Independent
    /// transactions are sent to a fixed-size worker pool so that the expensive
    /// `pre_check` work does not serialize inside the service actor.
    #[cfg(feature = "pipeline")]
    async fn classify_and_enqueue_tx_spawn(
        &self,
        tx: TransactionView,
        is_proposal_tx: bool,
        remote: Option<(Cycle, PeerIndex)>,
    ) -> Result<bool, Reject> {
        if let Some(routed) = self
            .check_and_route_dependent(&tx, is_proposal_tx, remote)
            .await?
        {
            return Ok(routed);
        }

        let job = PreCheckJob {
            tx,
            is_proposal_tx,
            remote,
        };
        self.pre_check_queue.push(job)?;

        // Returning Ok(true) only means the tx was accepted into the pipeline;
        // actual classification/verification happens in the worker pool.
        Ok(true)
    }

    async fn remove_orphan_txs_by_attach(&self, txs: &LinkedHashSet<TransactionView>) {
        // CRITICAL: this must run after `_update_tx_pool_for_reorg` has replaced
        // `tx_pool.snapshot` with the post-attachment snapshot. Because the snapshot
        // already reflects the attached blocks, an orphan whose input was consumed by
        // one of those blocks resolves to `CellStatus::Dead` and is rejected here,
        // instead of being accepted back into the pipeline.
        for tx in txs.iter() {
            self.process_orphan_tx(tx).await;
        }
        let mut orphan = self.orphan.write().await;
        orphan.remove_orphan_txs(txs.iter().map(|tx| tx.proposal_short_id()));
    }

    #[cfg(not(feature = "pipeline"))]
    async fn readd_detached_tx(
        &self,
        tx_pool: &mut TxPool,
        txs: Vec<TransactionView>,
        fetched_cache: HashMap<Byte32, CacheEntry>,
    ) {
        let max_cycles = self.tx_pool_config.max_tx_verify_cycles;
        for tx in txs {
            let tx_size = tx.data().serialized_size_in_block();
            let tx_hash = tx.hash();
            if let Ok((rtx, status)) = resolve_tx(tx_pool, tx_pool.snapshot(), tx, false)
                && let Ok(fee) = check_tx_fee(tx_pool, tx_pool.snapshot(), &rtx, tx_size)
            {
                let verify_cache = fetched_cache.get(&tx_hash).cloned();
                let snapshot = tx_pool.cloned_snapshot();
                let tip_header = snapshot.tip_header();
                let tx_env = Arc::new(status.with_env(tip_header));
                if let Ok(verified) = verify_rtx(
                    snapshot,
                    Arc::clone(&rtx),
                    tx_env,
                    &verify_cache,
                    max_cycles,
                    None,
                )
                .await
                {
                    let entry = TxEntry::new(rtx, verified.cycles, fee, tx_size);
                    if let Err(e) = _submit_entry(tx_pool, status, &entry, &self.callbacks) {
                        error!("readd_detached_tx submit_entry {} error {}", tx_hash, e);
                    } else {
                        debug!("readd_detached_tx submit_entry {}", tx_hash);
                    }
                }
            }
        }
    }

    pub(crate) async fn clear_pool(&mut self, new_snapshot: Arc<Snapshot>) {
        {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.clear(Arc::clone(&new_snapshot));
        }
        {
            let mut queue = self.ordered_resolve_queue.write().await;
            queue.clear();
        }
        {
            let mut queue = self.verify_queue.write().await;
            queue.clear();
        }
        {
            let mut orphan = self.orphan.write().await;
            orphan.clear();
        }
        #[cfg(feature = "pipeline")]
        {
            self.pre_check_queue.clear();
            self.rbf_candidates.write().await.clear();
        }
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
        self.fee_estimator.update_ibd_state(in_ibd);
    }

    pub(crate) async fn estimate_fee_rate(
        &self,
        estimate_mode: EstimateMode,
        enable_fallback: bool,
    ) -> Result<FeeRate, AnyError> {
        let all_entry_info = self.tx_pool.read().await.get_all_entry_info();
        match self
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

type PreCheckedTx = (
    Byte32,                   // tip_hash
    Arc<ResolvedTransaction>, // rtx
    TxStatus,                 // status
    Capacity,                 // tx fee
    usize,                    // tx size
);

type ResolveResult = Result<(Arc<ResolvedTransaction>, TxStatus), Reject>;

fn get_tx_status(snapshot: &Snapshot, short_id: &ProposalShortId) -> TxStatus {
    if snapshot.proposals().contains_proposed(short_id) {
        TxStatus::Proposed
    } else if snapshot.proposals().contains_gap(short_id) {
        TxStatus::Gap
    } else {
        TxStatus::Fresh
    }
}

fn check_rtx(
    tx_pool: &TxPool,
    snapshot: &Snapshot,
    rtx: &ResolvedTransaction,
) -> Result<TxStatus, Reject> {
    let short_id = rtx.transaction.proposal_short_id();
    let tx_status = get_tx_status(snapshot, &short_id);
    tx_pool.check_rtx_from_pool(rtx).map(|_| tx_status)
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

fn _submit_entry(
    tx_pool: &mut TxPool,
    status: TxStatus,
    entry: &TxEntry,
    callbacks: &Callbacks,
) -> Result<HashSet<TxEntry>, Reject> {
    let tx_hash = entry.transaction().hash();
    debug!("submit_entry {:?} {}", status, tx_hash);
    let (succ, evicts) = match status {
        TxStatus::Fresh => tx_pool.add_pending(entry.clone())?,
        TxStatus::Gap => tx_pool.add_gap(entry.clone())?,
        TxStatus::Proposed => tx_pool.add_proposed(entry.clone())?,
    };
    if succ {
        match status {
            TxStatus::Fresh => callbacks.call_pending(entry),
            TxStatus::Gap => callbacks.call_pending(entry),
            TxStatus::Proposed => callbacks.call_proposed(entry),
        }
    }
    Ok(evicts)
}

fn _update_tx_pool_for_reorg(
    tx_pool: &mut TxPool,
    attached: &LinkedHashSet<TransactionView>,
    detached_headers: &HashSet<Byte32>,
    detached_proposal_id: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    callbacks: &Callbacks,
    mine_mode: bool,
) -> Vec<(TxEntry, Reject)> {
    let mut reject_events = Vec::new();

    tx_pool.snapshot = Arc::clone(&snapshot);

    // NOTE: `remove_by_detached_proposal` will try to re-put the given expired/detached proposals into
    // pending-pool if they can be found within txpool. As for a transaction
    // which is both expired and committed at the one time(commit at its end of commit-window),
    // we should treat it as a committed and not re-put into pending-pool. So we should ensure
    // that involves `remove_committed_txs` before `remove_expired`.
    tx_pool.remove_committed_txs(attached.iter(), detached_headers, &mut reject_events);
    tx_pool.remove_by_detached_proposal(detached_proposal_id.iter(), callbacks, &mut reject_events);

    // mine mode:
    // pending ---> gap ----> proposed
    // try move gap to proposed
    if mine_mode {
        let mut proposals = Vec::new();
        let mut gaps = Vec::new();

        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Gap) {
            let short_id = entry.inner.proposal_short_id();
            if snapshot.proposals().contains_proposed(&short_id) {
                proposals.push((short_id, entry.inner.clone()));
            }
        }

        for entry in tx_pool.pool_map.entries.get_by_status(&Status::Pending) {
            let short_id = entry.inner.proposal_short_id();
            let elem = (short_id.clone(), entry.inner.clone());
            if snapshot.proposals().contains_proposed(&short_id) {
                proposals.push(elem);
            } else if snapshot.proposals().contains_gap(&short_id) {
                gaps.push(elem);
            }
        }

        for (id, entry) in proposals {
            debug!("begin to proposed: {:x}", id);
            if let Err(e) = tx_pool.proposed_rtx(&id) {
                debug!(
                    "Failed to add proposed tx {}, reason: {}",
                    entry.transaction().hash(),
                    e
                );
                reject_events.push((entry, e));
            } else {
                callbacks.call_proposed(&entry)
            }
        }

        for (id, entry) in gaps {
            debug!("begin to gap: {:x}", id);
            if let Err(e) = tx_pool.gap_rtx(&id) {
                debug!(
                    "Failed to add tx to gap {}, reason: {}",
                    entry.transaction().hash(),
                    e
                );
                reject_events.push((entry, e));
            }
        }
    }

    // Remove expired transaction from pending
    tx_pool.remove_expired(&mut reject_events);

    // Remove transactions from the pool until its size <= size_limit.
    let _ = tx_pool.limit_size(None, &mut reject_events);

    reject_events
}
