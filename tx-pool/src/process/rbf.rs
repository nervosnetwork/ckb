use crate::component::entry::TxEntry;
use crate::component::pipeline_queue::PipelineQueue;
use crate::error::Reject;
use crate::pool::TxPool;
use crate::resolved_tx::ResolvedTx;
use crate::tx_source::TxSource;
use ckb_types::core::{Capacity, FeeRate, TransactionView};
use ckb_types::packed::{OutPoint, ProposalShortId};
use std::collections::HashSet;

impl super::TxPoolService {
    /// Remove a transaction from the in-flight RBF fee-ordering gate.
    ///
    /// Called on every exit path from the verify → submit pipeline (verify
    /// failure, cycles mismatch, successful submission) to prevent ghost
    /// entries that are never cleaned up.
    pub(crate) async fn remove_rbf_candidate(&self, id: &ProposalShortId) {
        self.queues.rbf_candidates.write().await.remove(id);
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
        self.read_tx_pool(|tx_pool| {
            tx.input_pts_iter()
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
            let pool = self.tx_pool.read().await;
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
            self.queues
                .rbf_candidates
                .write()
                .await
                .remove_by_conflict_outpoints(&outpoints);
        }
    }

    // Remove conflicting transactions for RBF and record them in the conflicts
    // cache so they can be recovered if the replacement fails. Returns the set
    // of removed entries; the caller decides which ones to recover and when to
    // clean up the conflicts cache.
    pub(crate) fn process_rbf(
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
            ckb_logger::debug!(
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
        //
        // The original pipeline source is not retained once a transaction has
        // entered the pool, so recovered entries fall back to `TxSource::Local`.
        for old in &all_removed {
            tx_pool.record_conflict(old.transaction().clone(), TxSource::Local);
        }

        all_removed
    }

    /// Register a remote transaction as an RBF candidate and enqueue it for
    /// verification.
    ///
    /// This helper encapsulates the lock-order sensitive dance between
    /// `rbf_candidates` and `verify_queue`:
    ///   1. Validate the candidate and compute the displacement set while
    ///      holding `rbf_candidates.write()`.
    ///   2. Insert into the verify queue.
    ///   3. Commit the registration and remove displaced candidates atomically.
    ///
    /// This guarantees that lower-fee-rate displaced candidates are only removed
    /// from the pipeline once the higher-fee-rate candidate is successfully
    /// queued (P0-2 fix), and maintains the global lock order
    /// `rbf_candidates -> verify_queue` (P0-1 fix).
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

        // Fast-path duplicate check: a remote tx already in the verify queue
        // should not register as an RBF candidate and displace lower-fee-rate
        // candidates.
        {
            let verify_queue = self.queues.verify_queue.read().await;
            if verify_queue.contains_key(&id) {
                return Ok(false);
            }
        }

        let conflict_inputs = self.find_conflict_inputs(&tx).await;
        if conflict_inputs.is_empty() {
            return Ok(false);
        }

        let fee_rate = self.compute_size_based_fee_rate(fee, tx_size);
        let mut rbf_guard = self.queues.rbf_candidates.write().await;
        match rbf_guard.register(id.clone(), fee_rate, &conflict_inputs) {
            Ok(registration) => {
                let mut verify_queue = self.queues.verify_queue.write().await;
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

                        // Success: commit the registration and remove displaced
                        // candidates from the verify queue while holding both
                        // locks.
                        let displaced = registration
                            .displaced
                            .iter()
                            .filter_map(|(id, _, _)| verify_queue.remove_tx(id))
                            .collect::<Vec<_>>();
                        rbf_guard.commit(registration);
                        drop(verify_queue);
                        drop(rbf_guard);

                        if !displaced.is_empty() {
                            let ret = Err(Reject::RBFRejected(
                                "superseded by higher-fee-rate in-flight candidate".to_string(),
                            ));
                            for resolved in displaced {
                                self.after_process(resolved.tx, resolved.source, &ret).await;
                            }
                        }
                        Ok(true)
                    }
                    Err(reject) => {
                        // Verify queue rejected the tx (e.g. Full). Drop the
                        // pending registration so the inputs are not blocked.
                        drop(verify_queue);
                        drop(registration);
                        drop(rbf_guard);
                        self.after_process(tx, source, &Err(reject.clone())).await;
                        Err(reject)
                    }
                }
            }
            Err(reason) => {
                drop(rbf_guard);
                let reject = Reject::RBFRejected(reason);
                self.after_process(tx, source, &Err(reject.clone())).await;
                Err(reject)
            }
        }
    }
}
