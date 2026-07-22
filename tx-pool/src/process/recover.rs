//! Event-driven re-entry of transactions into the pipeline.
//!
//! This module owns the "who goes where, when" of transaction recovery:
//! transactions that could not proceed the first time are routed back into
//! the pipeline from here:
//! - **conflict recovery** (`enqueue_recover_txs`): txs whose conflicting
//!   inputs have been freed (failed RBF replacements, evictions) are
//!   re-enqueued into the ordered resolve queue;
//! - **orphan orchestration** (`handle_missing_input_orphan`,
//!   `process_orphan_tx`, `remove_orphan_txs_by_attach`): txs with missing
//!   parents are parked and re-routed once their parents land;
//! - **RBF-held restore** (`restore_held_rbf_candidates`): candidates
//!   displaced by a lost in-flight race resume verification once their
//!   winner leaves the pipeline.
//!
//! Storage stays with its owner: the pipeline-side `WaitingRoom` in
//! `PipelineState` holds `ParentsMissing` (orphans) and `RaceLost` (RBF
//! in-flight losers) entries, the pool-side `WaitingRoom` in `TxPool`
//! holds `InputsBlocked` (conflict recovery), and RBF registrations stay in
//! `component::rbf_candidates`. Only the orchestration lives here.
//!
//! Lock order notes (kept consistent with the global hierarchy
//! `ordered_resolve_queue → rbf_candidates → verify_queue → waiting_room →
//! tx_pool`): `handle_missing_input_orphan` holds `waiting_room.write()`
//! while taking `tx_pool.read()`; `requeue_and_reregister` holds
//! `rbf_candidates.write()` then `verify_queue.write()` then
//! `waiting_room.write()`; the ordered-queue fallback is only taken after
//! all three are released.

use crate::component::pipeline_queue::PipelineQueue;
use crate::component::rbf_candidates::displace_and_commit;
use crate::error::Reject;
use crate::resolved_tx::ResolvedTx;
use crate::service::{TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use ckb_logger::{debug, warn};
use ckb_store::ChainStore;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_util::LinkedHashSet;
use std::collections::HashSet;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Maximum number of attempts to enqueue one recovered transaction before
/// giving up. The ordered resolve queue is drained continuously by the
/// single ordered resolver, so `Reject::Full` is transient backpressure, not
/// a terminal condition; 40 attempts at `LOCAL_ORPHAN_RETRY_DELAY` (~2 s per
/// tx) is generous headroom over ordinary bursts, while still bounding how
/// long shutdown or a permanently stuck resolver can block the deferred
/// worker.
const RECOVER_ENQUEUE_MAX_ATTEMPTS: usize = 40;

/// How many worklist entries one lock slice of `requeue_and_reregister`
/// processes before releasing the three write guards, so a large restore
/// cannot stall the whole pipeline behind it.
const REQUEUE_LOCK_BATCH: usize = 32;

/// Re-enqueue recovered transactions into the ordered resolve queue,
/// retrying on transient `Full` backpressure.
///
/// The recovered txs' conflict-cache entries have already been removed by
/// the submit path, so dropping one here would lose a valid transaction
/// entirely. The queue lock is released between attempts so the resolver
/// can keep draining, and shutdown is observed between attempts so a full
/// queue cannot hold the deferred worker hostage. A transaction whose
/// bounded retries are exhausted still gets a *terminal* outcome (see
/// [`recover_terminal`]) — never a silent drop.
pub(crate) async fn enqueue_recover_txs(
    queues: Arc<crate::component::pipeline_queues::PipelineQueues>,
    txs: Vec<(TransactionView, TxSource)>,
    cancel: &CancellationToken,
    relay: &crate::service::RelayState,
) {
    for (tx, source) in txs {
        debug!("recover back: {:?}", tx.proposal_short_id());
        let job = crate::resolved_tx::ResolveJob::new(tx.clone(), source);
        for attempt in 1..=RECOVER_ENQUEUE_MAX_ATTEMPTS {
            let result = {
                let mut queue = queues.ordered_resolve_queue.write().await;
                queue.add_tx(job.clone())
            };
            match result {
                Ok(_) => break,
                Err(crate::error::Reject::Full(_)) if attempt < RECOVER_ENQUEUE_MAX_ATTEMPTS => {
                    tokio::select! {
                        _ = tokio::time::sleep(crate::resolve_mgr::LOCAL_ORPHAN_RETRY_DELAY) => {}
                        _ = cancel.cancelled() => {
                            warn!("recover task aborted by shutdown");
                            return;
                        }
                    }
                }
                Err(reject) => {
                    warn!(
                        "failed to recover tx back to ordered resolve queue after {} attempts: {}",
                        attempt, reject
                    );
                    recover_terminal(relay, &tx, source, reject);
                    break;
                }
            }
        }
    }
}

/// Terminal routing for a recovered transaction the queue could not take
/// even after the bounded retries: its earlier reject events were
/// suppressed in anticipation of this recovery, so the loop must be closed
/// now — a reject callback for subscribers and a relayer notification for
/// remote peers (their filter entry must not wait forever). Nothing is
/// recorded in recent_reject (`Full` is exempt from recording): this is
/// backpressure, not invalidity.
fn recover_terminal(
    relay: &crate::service::RelayState,
    tx: &TransactionView,
    source: TxSource,
    reject: Reject,
) {
    let entry = crate::component::entry::TxEntry::dummy_resolve(
        tx.clone(),
        0,
        ckb_types::core::Capacity::zero(),
        tx.data().serialized_size_in_block(),
    );
    relay.callbacks.call_reject(&entry, reject);
    if source.peer().is_some()
        && let Err(e) = relay
            .tx_relay_sender
            .send(TxVerificationResult::Reject { tx_hash: tx.hash() })
    {
        warn!("recover terminal relayer notify failed: {}", e);
    }
}

impl TxPoolService {
    pub(crate) async fn waiting_room_contains(&self, tx: &TransactionView) -> bool {
        let room = self.pipeline.waiting_room.read().await;
        room.contains_key(&tx.proposal_short_id())
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
        source: TxSource,
        parents: HashSet<Byte32>,
    ) {
        // Recheck parent availability while holding the orphan write lock.
        // The tx failed resolution a moment ago, but its parents may have
        // landed since: a parent committing in that window runs its
        // `process_orphan_tx` *before* this insert could complete, and
        // nothing would re-trigger for this orphan afterwards — it would be
        // stuck until expiry (~80 minutes). Lock order orphan -> tx_pool
        // follows the documented hierarchy (see `remove_tx`); a parent
        // committing concurrently runs its `process_orphan_tx` only after
        // we release this lock, and then finds the inserted orphan.
        let mut orphan = self.pipeline.waiting_room.write().await;
        if self.unavailable_parent_ids(&parents).await.is_empty() {
            drop(orphan);
            // Everything is resolvable now — route the tx through the
            // pipeline instead of parking it in the orphan pool.
            //
            // Box::pin is required because after_process and
            // handle_missing_input_orphan -> classify are mutually recursive
            // async fns (same pattern as process_orphan_tx).
            match Box::pin(self.classify_and_enqueue_tx(tx.clone(), source)).await {
                Ok(_) => return,
                // Only transient backpressure justifies a retry: any other
                // reject has already been traced by `after_process` inside
                // classify, so let it drop rather than churn the orphan
                // pool until expiry.
                Err(Reject::Full(_)) => {
                    // Parking as `ParentsMissing` would strand the tx until
                    // expiry: its parents are all available, so no parent
                    // event will ever fire to wake it. Retry through the
                    // ordered queue's delayed section instead, and only
                    // give up (terminally) if that is full too.
                    let mut ordered = self.pipeline.queues.ordered_resolve_queue.write().await;
                    let add_result = ordered.add_tx_delayed(
                        crate::resolved_tx::ResolveJob::new(tx.clone(), source),
                        crate::resolve_mgr::LOCAL_ORPHAN_RETRY_DELAY,
                    );
                    drop(ordered);
                    match add_result {
                        Ok(_) => return,
                        Err(reject) => {
                            self.terminal_reject(tx, source, reject).await;
                            return;
                        }
                    }
                }
                Err(_) => return,
            }
        }
        // Only notify the relayer after the tx has actually been accepted into
        // the orphan pool. This avoids telling peers about missing parents for
        // a tx that we end up dropping (e.g. duplicate orphan or pool full).
        let (added, evicted) = {
            let reason = crate::component::waiting_room::WaitReason::ParentsMissing {
                parents: tx.unique_parents(),
            };
            orphan.wait(tx, source, reason)
        };
        drop(orphan);
        // Route evicted entries by reason: orphans are rejected, race-lost
        // candidates are restored.
        self.route_waiting_evictions(evicted).await;
        if added && let Some(peer) = source.peer() {
            self.send_result_to_relayer(TxVerificationResult::UnknownParents { peer, parents });
        }
    }

    /// Test-only convenience wrapper for planting an orphan directly.
    #[cfg(test)]
    pub(crate) async fn add_orphan(&self, tx: TransactionView, source: TxSource) -> bool {
        let reason = crate::component::waiting_room::WaitReason::ParentsMissing {
            parents: tx.unique_parents(),
        };
        let (added, evicted) = self
            .pipeline
            .waiting_room
            .write()
            .await
            .wait(tx, source, reason);
        // for any evicted orphan tx, we should send reject to relayer
        // so that we mark it as `unknown` in filter
        for entry in evicted {
            self.send_result_to_relayer(TxVerificationResult::Reject {
                tx_hash: entry.tx.hash(),
            });
        }
        added
    }

    /// Remove all orphans which are directly resolved by the given transaction.
    ///
    /// Only the direct children of `tx` are re-routed here, through the same
    /// pipeline entry point as other remote transactions. When such an orphan
    /// is eventually verified and submitted, `after_process` recursively
    /// processes *its* children in the orphan pool — the recursion happens
    /// through `after_process`, not in this function.
    ///
    /// Removals are batched into a single write lock: an orphan's success or
    /// failure in the pipeline does not depend on its siblings being removed
    /// first, so there is no need to pay the cost of a write lock per orphan.
    pub(crate) async fn process_orphan_tx(&self, tx: &TransactionView) {
        self.process_orphan_tx_inner(tx, None).await;
    }

    /// `skip`: orphan ids that must not be routed in this pass (e.g.
    /// transactions that are themselves attached in the current reorg —
    /// routing them would resolve Dead against the new snapshot and poison
    /// recent_reject for a committed transaction).
    async fn process_orphan_tx_inner(
        &self,
        tx: &TransactionView,
        skip: Option<&HashSet<ProposalShortId>>,
    ) {
        // Collect the orphan entries under a single read lock, then process
        // them outside the lock. This keeps the critical section short and
        // avoids cloning transactions while holding the write lock.
        let orphans: Vec<_> = {
            let orphan = self.pipeline.waiting_room.read().await;
            orphan
                .find_by_parent(tx)
                .into_iter()
                .cloned()
                .filter_map(|id| orphan.get(&id).cloned().map(|entry| (id, entry)))
                .collect()
        };

        // Batch the parent-availability check: one read guard and one
        // snapshot for *all* orphans, instead of one guard plus per-parent
        // store lookups per orphan (reorgs route many orphans through here).
        let unavailable_hashes: HashSet<Byte32> = {
            let all_parents: HashSet<Byte32> = orphans
                .iter()
                .flat_map(|(_, entry)| entry.tx.unique_parents())
                .collect();
            let pool = self.pool.tx_pool.read().await;
            let snapshot = pool.cloned_snapshot();
            all_parents
                .into_iter()
                .filter(|h| !snapshot.transaction_exists(h))
                .filter(|h| !pool.contains_proposal_id(&ProposalShortId::from_tx_hash(h)))
                .collect()
        };

        let mut to_remove = Vec::new();
        for (orphan_id, orphan) in orphans.into_iter() {
            if skip.is_some_and(|skip| skip.contains(&orphan_id)) {
                continue;
            }
            let orphan_hash = orphan.tx.hash();

            // Only route orphans whose parents are *all* available now:
            // reclassifying one that still has missing parents just bounces
            // it back into the orphan pool (refreshing its expiry and
            // re-notifying the relayer on every round trip).
            if orphan
                .tx
                .unique_parents()
                .iter()
                .any(|parent| unavailable_hashes.contains(parent))
            {
                continue;
            }

            match self.classify_and_enqueue_tx(orphan.tx, orphan.source).await {
                Ok(_) => {
                    to_remove.push(orphan_id);
                    // The orphan is now in the pipeline. Its own children
                    // will be processed once it successfully submits via
                    // the normal `after_process` -> `handle_verify_success`
                    // path, so we do not need to push it back here.
                }
                Err(reject) => {
                    // Keep the orphan if the pipeline queues are temporarily
                    // full. For any other reject reason (malformed, low fee,
                    // etc.) remove it.
                    //
                    // `classify_and_enqueue_tx` already ran `after_process`
                    // for this reject (relayer notification, ban, recent
                    // reject), so we must not run `handle_remote_reject`
                    // again here.
                    if matches!(reject, Reject::Full(_)) {
                        warn!(
                            "process_orphan {} not ready ({reject}); keeping orphan from {}",
                            orphan_hash,
                            tx.hash(),
                        );
                    } else {
                        to_remove.push(orphan_id);
                    }
                }
            }
        }

        if !to_remove.is_empty() {
            let mut room = self.pipeline.waiting_room.write().await;
            for id in to_remove {
                // Only remove entries that are still parked as orphans: a
                // tx reclassified into the verify queue may have been
                // displaced by a stronger candidate while this loop ran,
                // re-parking it as `RaceLost` — removing that entry would
                // silently lose the candidate.
                let is_orphan = room.get(&id).is_some_and(|entry| {
                    matches!(
                        entry.reason,
                        crate::component::waiting_room::WaitReason::ParentsMissing { .. }
                    )
                });
                if is_orphan {
                    room.remove(&id);
                }
            }
        }
    }

    pub(crate) async fn remove_orphan_txs_by_attach(&self, txs: &LinkedHashSet<TransactionView>) {
        // CRITICAL: this must run after `update_tx_pool_for_reorg` has replaced
        // `tx_pool.snapshot` with the post-attachment snapshot. Because the snapshot
        // already reflects the attached blocks, an orphan whose input was consumed by
        // one of those blocks resolves to `CellStatus::Dead` and is rejected here,
        // instead of being accepted back into the pipeline.
        //
        // Orphans that are *themselves* attached must be skipped while
        // routing: their inputs are consumed by their own block, so routing
        // them would resolve Dead and poison recent_reject (and the relayer
        // filter) for a transaction that just committed on-chain. They are
        // removed from the room by `remove_many` below.
        let attached_ids: HashSet<ProposalShortId> =
            txs.iter().map(|tx| tx.proposal_short_id()).collect();
        for tx in txs.iter() {
            self.process_orphan_tx_inner(tx, Some(&attached_ids)).await;
        }
        let mut orphan = self.pipeline.waiting_room.write().await;
        orphan.remove_many(attached_ids.into_iter());
    }

    /// Restore held (displaced) candidates back into the verify queue and
    /// resume their in-flight registrations.
    ///
    /// They bypass the entry path entirely — in particular they are *not*
    /// run through `after_process`, so no recent-reject entry is recorded
    /// for a rejection that never became real. Duplicate re-insertion is
    /// harmless (`add_tx` dedups by id, covering a resubmission that
    /// arrived while the tx was held).
    ///
    /// The registration is restored together with the queue slot: a
    /// restored candidate with only its slot back would be unprotected and
    /// could be rejected at submit time by a *speculative* competitor,
    /// re-opening the censorship vector the hold-and-restore design
    /// removes. Restored candidates are processed highest-fee-rate first,
    /// so siblings conflicting with each other displace fairly; a restored
    /// candidate may itself displace a registration that appeared meanwhile
    /// (the displaced txs are held by its registration), and whatever the
    /// displaced registrations held is appended to the work list — their
    /// displacer is gone.
    ///
    /// If the verify queue rejects with `Full`, the tx falls back to the
    /// ordered resolve queue (it will be re-resolved there); if that also
    /// fails the tx is dropped with a warning — bounded queues force a
    /// terminal somewhere.
    pub(crate) async fn restore_held_rbf_candidates(&self, held: Vec<ResolvedTx>) {
        if held.is_empty() {
            return;
        }
        let (worklist, consumed) = self.sort_and_prefetch(held).await;
        let (fallbacks, evicted) = self.requeue_and_reregister(worklist, consumed).await;
        self.fallback_terminal(fallbacks).await;
        // Box::pin is required: route → terminal_reject → handle_remote_reject
        // → ban_malformed → restore is an unboxed recursion cycle otherwise.
        Box::pin(self.route_waiting_evictions(evicted)).await;
    }

    /// Route waiting-room evictions by reason: orphans get a Reject
    /// notification (their wait expired or the room was full). An expired
    /// `RaceLost` revokes the stalled winner's *speculative* registration
    /// and restores every loser it held. Merely surviving in a verify backlog
    /// is not proof that the winner is valid and must never terminally reject
    /// an already-admitted transaction. Revocation also prevents an
    /// expired -> restored -> re-held verification loop; the winner may keep
    /// verifying, but it must compete under the real pool RBF rules.
    pub(crate) async fn route_waiting_evictions(
        &self,
        evicted: Vec<crate::component::waiting_room::WaitingEntry>,
    ) {
        let mut restore = Vec::new();
        let mut stale_winners = HashSet::new();
        for entry in evicted {
            match entry.reason {
                crate::component::waiting_room::WaitReason::ParentsMissing { .. } => {
                    self.send_result_to_relayer(TxVerificationResult::Reject {
                        tx_hash: entry.tx.hash(),
                    });
                }
                crate::component::waiting_room::WaitReason::RaceLost { winner } => {
                    if let Some(resolved) = entry.resolved {
                        // Join the winner's ownership transition below. If
                        // finalize/abort already won both locks this is an
                        // idempotent no-op; otherwise expiry revokes the stale
                        // speculative registration and wakes all its losers.
                        stale_winners.insert(winner);
                        restore.push(*resolved);
                    } else {
                        // RaceLost entries are always parked with their
                        // resolved form; reaching here means a future
                        // re-park path forgot to carry it.
                        warn!(
                            "RaceLost waiting entry without resolved form: {}",
                            entry.tx.hash()
                        );
                    }
                }
                crate::component::waiting_room::WaitReason::InputsBlocked { .. } => {
                    // Conflicts-cache migration (S3): recovered conflicts
                    // resume through the ordered resolve queue.
                }
            }
        }
        if !stale_winners.is_empty() {
            // Lock order: rbf_candidates -> waiting_room. Removing the
            // registration first makes every subsequent submit observe that
            // the timeout revoked the speculative preference.
            let mut rbf = self.pipeline.queues.rbf_candidates.write().await;
            let mut room = self.pipeline.waiting_room.write().await;
            for winner in stale_winners {
                rbf.remove(&winner);
                restore.extend(room.wake_by_winner(&winner));
            }
        }
        // Box::pin is required: restore → route → restore is recursive
        // (expired race-lost entries resume through this same path).
        Box::pin(self.restore_held_rbf_candidates(restore)).await;
    }

    /// Sort held candidates strongest-first (sibling conflicts between
    /// restored candidates resolve in fee-rate order) and pre-compute their
    /// conflict inputs with a single `tx_pool` read, so the critical
    /// section in `requeue_and_reregister` contains only in-memory
    /// operations and cannot stall the pipeline on per-item lock
    /// acquisition. The result may be slightly stale by the time
    /// registrations commit: a freed input merely keeps a conservative
    /// registration (cleaned up later), and an input consumed meanwhile
    /// just skips registration and lets the pool-level RBF rules decide.
    async fn sort_and_prefetch(
        &self,
        mut held: Vec<ResolvedTx>,
    ) -> (std::collections::VecDeque<ResolvedTx>, HashSet<OutPoint>) {
        held.sort_by_key(|resolved| {
            std::cmp::Reverse(self.compute_size_based_fee_rate(resolved.fee, resolved.tx_size))
        });
        let worklist: std::collections::VecDeque<ResolvedTx> = held.into();
        let consumed = self
            .consumed_inputs(worklist.iter().map(|resolved| &resolved.tx))
            .await;
        (worklist, consumed)
    }

    /// Re-queue every worklist entry into the verify queue and resume its
    /// in-flight registration. The three write guards (`rbf_candidates`,
    /// `verify_queue`, `waiting_room`) are taken per *slice* of
    /// [`REQUEUE_LOCK_BATCH`] entries and released between slices, so a
    /// large restore cannot stall the whole pipeline behind one pass. A
    /// restored candidate may itself displace a registration that
    /// appeared meanwhile (the displaced txs are parked as its `RaceLost`),
    /// and whatever the displaced registrations held is appended to the
    /// work list — their displacer is gone. Returns the jobs that the
    /// verify queue rejected with `Full` (for `fallback_terminal`) and the
    /// entries evicted from the waiting room during the pass (for
    /// `route_waiting_evictions`).
    async fn requeue_and_reregister(
        &self,
        mut worklist: std::collections::VecDeque<ResolvedTx>,
        consumed: HashSet<OutPoint>,
    ) -> (
        Vec<crate::resolved_tx::ResolveJob>,
        Vec<crate::component::waiting_room::WaitingEntry>,
    ) {
        let mut fallbacks = Vec::new();
        let mut evicted = Vec::new();
        while !worklist.is_empty() {
            let mut batch = 0;
            {
                let mut rbf_guard = self.pipeline.queues.rbf_candidates.write().await;
                let mut verify_queue = self.pipeline.queues.verify_queue.write().await;
                let mut room = self.pipeline.waiting_room.write().await;
                while let Some(resolved) = worklist.pop_front() {
                    let tx = resolved.tx.clone();
                    let source = resolved.source;
                    let id = tx.proposal_short_id();
                    let fee_rate = self.compute_size_based_fee_rate(resolved.fee, resolved.tx_size);
                    match verify_queue.add_tx(resolved) {
                        Ok(true) => {
                            let conflict_inputs: Vec<OutPoint> = tx
                                .input_pts_iter()
                                .filter(|out_point| consumed.contains(out_point))
                                .collect();
                            if conflict_inputs.is_empty() {
                                batch += 1;
                                if batch >= REQUEUE_LOCK_BATCH {
                                    break;
                                }
                                continue;
                            }
                            if let Ok(registration) =
                                rbf_guard.register(id, fee_rate, &conflict_inputs)
                            {
                                let crate::component::rbf_candidates::DisplaceOutcome {
                                    to_restore,
                                    evicted: ev,
                                } = displace_and_commit(
                                    &mut rbf_guard,
                                    &mut verify_queue,
                                    &mut room,
                                    registration,
                                );
                                worklist.extend(to_restore);
                                evicted.extend(ev);
                            }
                            // A registration error means an equal-or-stronger
                            // sibling already holds an input: keep the restored
                            // tx queued; the fee ordering is decided at submit.
                        }
                        Ok(false) => {
                            // Duplicate: resubmitted while held; its own entry
                            // path already handles registration.
                        }
                        Err(_) => fallbacks.push(crate::resolved_tx::ResolveJob::new(tx, source)),
                    }
                    batch += 1;
                    if batch >= REQUEUE_LOCK_BATCH {
                        break;
                    }
                }
            }
            // Let the pipeline breathe between slices: submissions, the
            // ordered resolver and verify workers all need these locks.
            tokio::task::yield_now().await;
        }
        (fallbacks, evicted)
    }

    /// Chain-end for jobs the verify queue could not take: fall back to the
    /// ordered resolve queue (re-resolution there), and if that is also
    /// full, leave a trace through the normal after_process path
    /// (recent_reject + relay) instead of silently dropping the displaced
    /// candidate.
    async fn fallback_terminal(&self, fallbacks: Vec<crate::resolved_tx::ResolveJob>) {
        if fallbacks.is_empty() {
            return;
        }
        let mut terminal = Vec::new();
        {
            let mut ordered = self.pipeline.queues.ordered_resolve_queue.write().await;
            for job in fallbacks {
                // `add_tx` consumes the job on error; keep the raw parts
                // for the terminal trace.
                let tx = job.tx.clone();
                let source = job.source;
                if let Err(reject) = ordered.add_tx(job) {
                    terminal.push((tx, source, reject));
                }
            }
        }
        // Box::pin is required because after_process → orphan →
        // classify → register_rbf_candidate can lead back here, making
        // the recursion indirect (same pattern as process_orphan_tx).
        for (tx, source, reject) in terminal {
            Box::pin(self.after_process(tx, source, &Err(reject))).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::verify_queue::VerifyQueue;
    use ckb_types::core::TransactionBuilder;
    use std::time::Duration;
    use tokio::sync::RwLock;

    /// A recovered transaction must not be dropped when the ordered resolve
    /// queue is momentarily full: the worker retries (releasing the queue
    /// lock between attempts) until the resolver drains room for it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recover_txs_retries_until_ordered_queue_has_room() {
        let queues = Arc::new(crate::component::pipeline_queues::PipelineQueues {
            ordered_resolve_queue: RwLock::new(
                crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
            ),
            verify_queue: RwLock::new(VerifyQueue::new(
                70_000_000,
                ckb_app_config::VerifyOrdering::ArrivalTime,
                usize::MAX,
            )),
            pre_check_queue: crate::component::pre_check_queue::PreCheckQueue::new(
                CancellationToken::new(),
            ),
            rbf_candidates: RwLock::new(crate::component::rbf_candidates::RbfCandidates::new()),
        });

        // Fill the ordered queue to the brim so the first enqueue attempts
        // fail with `Reject::Full`.
        {
            let mut queue = queues.ordered_resolve_queue.write().await;
            queue.set_total_tx_size_for_test(crate::constants::MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE);
        }

        let tx = TransactionBuilder::default().build();
        let id = tx.proposal_short_id();
        let cancel = CancellationToken::new();
        let (tx_relay_sender, _relay_rx) = ckb_channel::bounded(16);
        let (block_assembler_sender, _ba_rx) = tokio::sync::mpsc::channel(1);
        let relay = crate::service::RelayState {
            network: Arc::new(crate::network::DummyTxPoolNetwork),
            tx_relay_sender,
            block_assembler_sender,
            callbacks: Arc::new(crate::callback::Callbacks::new()),
            banned_peers: Default::default(),
        };

        // Free room after 120ms, the way the resolver would by draining a job.
        {
            let queues = Arc::clone(&queues);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(120)).await;
                let mut queue = queues.ordered_resolve_queue.write().await;
                queue.set_total_tx_size_for_test(0);
            });
        }

        tokio::time::timeout(
            Duration::from_secs(10),
            enqueue_recover_txs(
                Arc::clone(&queues),
                vec![(tx, TxSource::Local)],
                &cancel,
                &relay,
            ),
        )
        .await
        .expect("recover worker must finish once room is freed");
        let queue = queues.ordered_resolve_queue.read().await;
        assert!(
            queue.contains_key(&id),
            "recovered tx must eventually be enqueued, not dropped"
        );
    }
}
