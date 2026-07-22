use crate::component::entry::TxEntry;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::rbf_candidates::displace_and_commit;
use crate::error::Reject;
use crate::resolved_tx::ResolvedTx;
use crate::service::TxVerificationResult;
use crate::tx_source::TxSource;
use ckb_types::core::{Capacity, FeeRate, TransactionView};
use ckb_types::packed::{OutPoint, ProposalShortId};
use std::collections::HashSet;

impl super::TxPoolService {
    /// Remove the in-flight registration of a candidate that has been
    /// committed to the pool, and definitively reject the candidates it
    /// displaced: their rejection is now real (the winner committed), so
    /// it is relayed through the usual `after_process` path and recorded
    /// in recent_reject like any terminal RBF rejection.
    pub(crate) async fn finalize_rbf_candidate(&self, id: &ProposalShortId) {
        let held = self.settle_rbf_candidate(id, true).await;
        self.publish_finalized_rbf_candidates(held).await;
    }

    /// Atomically release one RBF registration and every candidate it owns.
    ///
    /// This is the authoritative pre-publication ownership transition used by
    /// the submit transaction. A committed winner returns its held candidates
    /// for later terminal publication; an aborted winner restores them before
    /// returning. In either case no bounded side-effect channel is awaited
    /// while the registration still owns `RaceLost` entries.
    pub(crate) async fn settle_rbf_candidate(
        &self,
        id: &ProposalShortId,
        committed: bool,
    ) -> Vec<ResolvedTx> {
        self.settle_rbf_candidate_inner(id, committed, None).await
    }

    pub(crate) async fn settle_rbf_candidate_at(
        &self,
        id: &ProposalShortId,
        committed: bool,
        epoch: u64,
    ) -> Vec<ResolvedTx> {
        self.settle_rbf_candidate_inner(id, committed, Some(epoch))
            .await
    }

    async fn settle_rbf_candidate_inner(
        &self,
        id: &ProposalShortId,
        committed: bool,
        epoch: Option<u64>,
    ) -> Vec<ResolvedTx> {
        // Registration removal and held-entry transfer are one ownership
        // transition. Holding both locks in the global order prevents expiry
        // routing from observing "winner gone" while its losers are still
        // parked under that winner.
        let held = {
            let mut rbf = self.pipeline.queues.rbf_candidates.write().await;
            let mut room = self.pipeline.waiting_room.write().await;
            if epoch.is_some_and(|epoch| !self.is_pipeline_epoch_current(epoch)) {
                return Vec::new();
            }
            rbf.remove(id);
            room.wake_by_winner(id)
        };
        if committed {
            held
        } else {
            self.restore_held_rbf_candidates(held).await;
            Vec::new()
        }
    }

    /// Publish the real terminal rejection of candidates released by a
    /// committed RBF winner. The ownership transition has already completed,
    /// so callback/relay/recent-reject work cannot expose a half-settled gate.
    pub(crate) async fn publish_finalized_rbf_candidates(&self, held: Vec<ResolvedTx>) {
        if held.is_empty() {
            return;
        }
        let ret = Err(Reject::RBFRejected(
            super::TxPoolService::SUPERSEDED_BY_HIGHER_FEE_CANDIDATE.to_string(),
        ));
        for resolved in held {
            self.after_process_at(resolved.tx, resolved.source, &ret, resolved.epoch)
                .await;
        }
    }

    /// Remove the in-flight registration of a candidate that left the
    /// pipeline without committing (verification failure, declared-cycles
    /// mismatch, submit failure, removal by RPC or peer ban), and restore
    /// the candidates it displaced: the displacement was speculative, so
    /// they simply resume verification.
    #[cfg(test)]
    pub(crate) async fn abort_rbf_candidate(&self, id: &ProposalShortId) {
        self.settle_rbf_candidate(id, false).await;
    }

    pub(crate) async fn abort_rbf_candidate_at(&self, id: &ProposalShortId, epoch: u64) {
        self.settle_rbf_candidate_at(id, false, epoch).await;
    }

    /// Hand a superseded-at-submit candidate to the waiting room as the
    /// winner's `RaceLost` instead of rejecting it.
    ///
    /// The superseding decision was made against an *unverified* winner, so
    /// it is speculative and the candidate's fate must be too: if the winner
    /// is committed to the pool, the candidate is really rejected
    /// (`finalize_rbf_candidate`); if the winner leaves the pipeline first,
    /// the candidate is restored to the verify queue
    /// (`restore_held_rbf_candidates`). If the winner has already
    /// disappeared (committed or aborted between the superseded check and
    /// this call), the candidate simply resumes: it is restored immediately.
    ///
    /// `conflict_inputs` is computed by the caller (`submit_entry` already
    /// holds it), so this does not cost a second `tx_pool` read.
    pub(crate) async fn hold_superseded_candidate(
        &self,
        resolved: ResolvedTx,
        conflict_inputs: Vec<OutPoint>,
    ) -> bool {
        let id = resolved.tx.proposal_short_id();
        let epoch = resolved.epoch;
        let own_fee_rate = self.compute_size_based_fee_rate(resolved.fee, resolved.tx_size);
        let (own_woken, evicted, restore_self) = {
            let mut rbf_guard = self.pipeline.queues.rbf_candidates.write().await;
            let mut verify_queue = self.pipeline.queues.verify_queue.write().await;
            let mut room = self.pipeline.waiting_room.write().await;
            if !self.is_pipeline_epoch_current(epoch) {
                return false;
            }
            // The worker's active lease hands ownership to RaceLost (or to
            // the immediate restore path) here. Its trailing finish call is
            // token-conditional and cannot erase a later restored lease.
            verify_queue.release_active_for_transition(&id, epoch);
            // The candidate's own registration (if any) is removed — it lost
            // the race. Anything it held is restored right away: their
            // displacer is no longer a live registration.
            let own_woken = {
                rbf_guard.remove(&id);
                room.wake_by_winner(&id)
            };
            // Re-check the winner's strength: the superseding decision was
            // made under a read guard a moment ago, and the original
            // superseder may have left the pipeline since. Only park under
            // a registration that is *actually* stronger — otherwise a
            // weaker leftover registration would finalize-reject a
            // candidate that should have replaced it.
            match rbf_guard.find_winner(&conflict_inputs, &id) {
                Some((winner, winner_rate)) if winner_rate > own_fee_rate => {
                    let (retained, evicted) = room.wait_resolved(
                        resolved,
                        crate::component::waiting_room::WaitReason::RaceLost { winner },
                    );
                    // RaceLost is exempt from the waiting-room-local budget:
                    // its ResolvedTx lifecycle permit remains charged, so it
                    // cannot be evicted here.
                    debug_assert!(
                        retained,
                        "a freshly parked RaceLost entry cannot be evicted"
                    );
                    (own_woken, evicted, None)
                }
                // The winner already left the pipeline (or what remains is
                // not stronger): the candidate simply resumes.
                _ => (own_woken, Vec::new(), Some(resolved)),
            }
        };
        let mut restore = own_woken;
        restore.extend(restore_self);
        self.restore_held_rbf_candidates(restore).await;
        self.route_waiting_evictions(evicted).await;
        true
    }

    /// Compute the size-based fee rate used for RBF in-flight candidate
    /// ordering.
    ///
    /// The gate must not trust peer-declared cycles: a malicious peer could
    /// declare artificially low cycles to inflate a weight-based fee rate and
    /// displace honest candidates (the lie is only caught later, at
    /// `DeclaredWrongCycles`, after the honest candidate has already been
    /// evicted from the pipeline). Using the same size-based fee rate as
    /// `min_fee_rate` and [`TxPool::calculate_min_replace_fee`] removes the
    /// manipulable input and keeps the ordering consistent with the pool's
    /// RBF replacement fee floor.
    pub(crate) fn compute_size_based_fee_rate(&self, fee: Capacity, tx_size: usize) -> FeeRate {
        FeeRate::calculate(fee, tx_size as u64)
    }

    /// Find the transaction inputs that are currently consumed by in-pool txs.
    /// These are the "conflict inputs" that matter for RBF ordering.
    pub(crate) async fn find_conflict_inputs(&self, tx: &TransactionView) -> Vec<OutPoint> {
        self.consumed_inputs(std::iter::once(tx))
            .await
            .into_iter()
            .collect()
    }

    /// Inputs of the given transactions that are currently consumed by
    /// in-pool txs, computed with a single `tx_pool` read.
    ///
    /// Used by the restore path so a whole worklist is priced with one lock
    /// acquisition instead of one per transaction.
    pub(crate) async fn consumed_inputs<'a>(
        &self,
        txs: impl IntoIterator<Item = &'a TransactionView> + Send,
    ) -> HashSet<OutPoint> {
        let inputs: Vec<OutPoint> = txs.into_iter().flat_map(|tx| tx.input_pts_iter()).collect();
        self.read_tx_pool(|tx_pool| {
            inputs
                .into_iter()
                .filter(|out_point| {
                    tx_pool
                        .pool_map
                        .out_point_index
                        .get_input_ref(out_point)
                        .is_some()
                })
                .collect()
        })
        .await
    }

    /// Clean up in-flight RBF candidates whose conflict inputs are no longer
    /// consumed by any in-pool transaction.
    ///
    /// This is a best-effort liveness helper: when a pool entry is evicted,
    /// replaced, or committed, some of the inputs it was consuming may become
    /// free. A concurrent RBF candidate may still hold those inputs in
    /// `rbf_candidates`; removing the stale registrations prevents future
    /// replacements from being blocked by a candidate that is no longer racing
    /// against anyone.
    ///
    /// Inputs that are still consumed by an in-pool transaction are *not*
    /// included, because a candidate targeting them is still a valid competitor
    /// for the current owner of those inputs.
    ///
    /// # Lock ordering
    ///
    /// The `tx_pool` read guard is dropped before `rbf_candidates` is locked.
    /// This is intentional: `submit_entry` holds `rbf_candidates.read()` while
    /// taking `tx_pool.write()`, so the two locks must never be acquired in
    /// opposite order. Keep this invariant if you refactor this helper.
    pub(crate) async fn cleanup_rbf_for_removed_entries<'a>(
        &self,
        removed_entries: impl IntoIterator<Item = &'a TxEntry> + Send,
    ) {
        let outpoints: HashSet<OutPoint> = {
            let pool = self.pool.tx_pool.read().await;
            removed_entries
                .into_iter()
                .flat_map(|entry| entry.transaction().input_pts_iter())
                .filter(|outpoint| {
                    pool.pool_map
                        .out_point_index
                        .get_input_ref(outpoint)
                        .is_none()
                })
                .collect()
        };
        if !outpoints.is_empty() {
            let held = {
                let mut rbf = self.pipeline.queues.rbf_candidates.write().await;
                let mut room = self.pipeline.waiting_room.write().await;
                let mut held = Vec::new();
                for id in rbf.remove_by_conflict_outpoints(&outpoints) {
                    held.extend(room.wake_by_winner(&id));
                }
                held
            };
            // Candidates held by the removed registrations are restored:
            // their displacer just left the pipeline without committing.
            self.restore_held_rbf_candidates(held).await;
        }
    }

    /// Register a remote transaction as an RBF candidate and enqueue it for
    /// verification.
    ///
    /// This helper encapsulates the lock-order sensitive dance between
    /// `rbf_candidates` and `verify_queue`:
    ///   1. Validate the candidate and compute the displacement set while
    ///      holding `rbf_candidates.write()`.
    ///   2. Insert into the verify queue.
    ///   3. Commit the registration and remove displaced candidates from the
    ///      verify queue atomically, holding them in the new registration.
    ///
    /// This guarantees that lower-fee-rate displaced candidates are only removed
    /// from the pipeline once the higher-fee-rate candidate is successfully
    /// queued (P0-2 fix), and maintains the global lock order
    /// `rbf_candidates -> verify_queue` (P0-1 fix). Displaced candidates are
    /// held, not rejected — see the `rbf_candidates` module docs for the
    /// hold-and-restore contract.
    ///
    /// Returns `Ok(true)` if the tx was newly queued, `Ok(false)` if it was a
    /// duplicate, and `Err` if registration or queuing failed.
    pub(crate) async fn register_rbf_candidate(
        &self,
        tx: TransactionView,
        source: TxSource,
        resolved: &ResolvedTx,
        fee: Capacity,
        tx_size: usize,
    ) -> Result<bool, Reject> {
        let id = tx.proposal_short_id();
        let epoch = resolved.epoch;

        if !self.is_pipeline_epoch_current(epoch) {
            return Err(Reject::Internal(
                "tx-pool pipeline generation invalidated by clear".to_string(),
            ));
        }

        // Fast-path duplicate check: a remote tx already in the verify queue
        // should not register as an RBF candidate and displace lower-fee-rate
        // candidates.
        {
            let verify_queue = self.pipeline.queues.verify_queue.read().await;
            if verify_queue.contains_key(&id) {
                return Ok(false);
            }
        }

        let conflict_inputs = self.find_conflict_inputs(&tx).await;
        if conflict_inputs.is_empty() {
            return Ok(false);
        }

        let fee_rate = self.compute_size_based_fee_rate(fee, tx_size);
        let mut rbf_guard = self.pipeline.queues.rbf_candidates.write().await;
        if !self.is_pipeline_epoch_current(epoch) {
            return Err(Reject::Internal(
                "tx-pool pipeline generation invalidated by clear".to_string(),
            ));
        }
        match rbf_guard.register(id.clone(), fee_rate, &conflict_inputs) {
            Ok(registration) => {
                let mut verify_queue = self.pipeline.queues.verify_queue.write().await;
                if !self.is_pipeline_epoch_current(epoch) {
                    drop(verify_queue);
                    drop(registration);
                    return Err(Reject::Internal(
                        "tx-pool pipeline generation invalidated by clear".to_string(),
                    ));
                }
                match verify_queue.add_tx(resolved.clone()) {
                    Ok(added) => {
                        if !added {
                            // Duplicate: the same tx was inserted concurrently.
                            // Drop the pending registration without displacing
                            // anyone.
                            drop(verify_queue);
                            drop(registration);
                            return Ok(false);
                        }

                        // Success: commit the registration. Displaced
                        // candidates leave the verify queue but are parked
                        // in the waiting room as this winner's `RaceLost`
                        // instead of being rejected: their rejection only
                        // becomes real once this candidate is committed to
                        // the pool (`finalize_rbf_candidate`); if this
                        // candidate leaves the pipeline first, they are
                        // restored (`abort_rbf_candidate`). No recent-reject
                        // entry is recorded for a speculative displacement —
                        // that would let an unverified high-fee candidate
                        // censor an honest in-flight one at zero cost.
                        let mut room = self.pipeline.waiting_room.write().await;
                        if !self.is_pipeline_epoch_current(epoch) {
                            // `clear_pipeline` advances before acquiring any
                            // of these locks. If it already won, do not commit
                            // a registration behind its clearing pass.
                            drop(room);
                            verify_queue.remove_tx(&id);
                            drop(verify_queue);
                            drop(registration);
                            return Err(Reject::Internal(
                                "tx-pool pipeline generation invalidated by clear".to_string(),
                            ));
                        }
                        let crate::component::rbf_candidates::DisplaceOutcome {
                            to_restore,
                            evicted,
                        } = displace_and_commit(
                            &mut rbf_guard,
                            &mut verify_queue,
                            &mut room,
                            registration,
                        );
                        drop(room);
                        drop(verify_queue);
                        drop(rbf_guard);

                        // Candidates whose displacer just left the pipeline
                        // (including whatever the displaced registrations
                        // held) resume verification.
                        self.restore_held_rbf_candidates(to_restore).await;
                        self.route_waiting_evictions(evicted).await;
                        Ok(true)
                    }
                    Err(reject) => {
                        // Verify queue rejected the tx (e.g. Full). Drop the
                        // pending registration so the inputs are not blocked.
                        drop(verify_queue);
                        drop(registration);
                        drop(rbf_guard);
                        self.after_process_at(tx, source, &Err(reject.clone()), epoch)
                            .await;
                        Err(reject)
                    }
                }
            }
            Err(reason) => {
                // A valid candidate that loses only to an *unverified*
                // stronger registration is speculative, not terminal. Park
                // it under that winner while both indexes are locked, so a
                // failed winner wakes it even though the original in-pool
                // input owner never changed. Recording it as InputsBlocked
                // here strands it forever: winner abort does not free the
                // pool input and therefore cannot trigger conflict recovery.
                if let Some((winner, winner_rate)) = rbf_guard.find_winner(&conflict_inputs, &id)
                    && winner_rate >= fee_rate
                {
                    let evicted = {
                        let mut room = self.pipeline.waiting_room.write().await;
                        if !self.is_pipeline_epoch_current(epoch) {
                            return Err(Reject::Internal(
                                "tx-pool pipeline generation invalidated by clear".to_string(),
                            ));
                        }
                        let (retained, evicted) = room.wait_resolved(
                            resolved.clone(),
                            crate::component::waiting_room::WaitReason::RaceLost { winner },
                        );
                        debug_assert!(
                            retained,
                            "a freshly parked RaceLost entry cannot be evicted"
                        );
                        evicted
                    };
                    drop(rbf_guard);
                    self.route_waiting_evictions(evicted).await;
                    return Ok(true);
                }
                drop(rbf_guard);
                let reject = Reject::RBFRejected(reason);
                // Non-supersession failures (for example the displacement
                // fan-out cap) have no winner that can restore this
                // candidate. Keep the normal conflict-recovery trace, but do
                // not poison recent_reject.
                {
                    let mut tx_pool = self.pool.tx_pool.write().await;
                    if tx_pool.pool_map.find_conflict_outpoint(&tx).is_some() {
                        tx_pool.record_conflict(tx.clone(), source);
                    }
                }
                if source.peer().is_some() {
                    self.send_result_to_relayer(TxVerificationResult::Reject {
                        tx_hash: tx.hash(),
                    });
                }
                Err(reject)
            }
        }
    }
}
