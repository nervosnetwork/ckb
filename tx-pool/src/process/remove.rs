//! Transaction removal from the pipeline structures and the pool, plus the
//! liveness heuristics that support it.
//!
//! [`TxPoolService::remove_tx`] removes a transaction by hash from every
//! structure it may occupy (pre-check, ordered resolve, verify, orphan and
//! the main pool), and [`TxPoolService::all_missing_parents_in_flight`] is
//! the read-only counterpart used by the ordered resolver to decide whether
//! an orphan's parents are still on their way.

use crate::component::pipeline_queue::PipelineQueue;
use ckb_store::ChainStore;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;

impl super::TxPoolService {
    /// Notify the ordered resolver if there are jobs waiting.
    ///
    /// Must be called after a transaction is removed from the verify queue or
    /// the in-pool set: the removed tx may have had descendants waiting in the
    /// ordered resolve queue, and waking the resolver lets them be retried
    /// (and rejected if the parent is gone) promptly.
    pub(super) async fn wake_ordered_resolver_if_needed(&self) {
        let ordered = self.queues.ordered_resolve_queue.read().await;
        if !ordered.is_empty() {
            ordered.subscribe().notify_one();
        }
    }

    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> bool {
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        if self.queues.pre_check_queue.remove_by_id(&id).is_some() {
            return true;
        }
        {
            let mut queue = self.queues.ordered_resolve_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                return true;
            }
        }
        {
            // Lock hierarchy: rbf_candidates must be acquired before
            // verify_queue. Holding rbf_candidates while checking verify_queue
            // prevents a deadlock with register_rbf_candidate / update_reorg,
            // which take rbf_candidates.write() before verify_queue.write().
            // Orphan and tx_pool are checked after verify_queue so that the
            // global order remains consistent across remove_tx and
            // ban_malformed: ordered -> rbf -> verify -> orphan -> tx_pool.
            let mut rbf = self.queues.rbf_candidates.write().await;
            let mut queue = self.queues.verify_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                drop(queue);
                rbf.remove(&id);
                // The removed tx may have had descendants waiting in the
                // ordered resolve queue. Wake the resolver so they can be
                // retried (and rejected if the parent is gone) promptly.
                self.wake_ordered_resolver_if_needed().await;
                return true;
            }
        }
        {
            let mut orphan = self.orphan.write().await;
            if orphan.remove_orphan_tx(&id).is_some() {
                return true;
            }
        }
        let removed_entries = {
            let mut tx_pool = self.tx_pool.write().await;
            tx_pool.remove_tx(&id)
        };
        if !removed_entries.is_empty() {
            // The removed pool entries have released their inputs. Clean up
            // any in-flight RBF candidates targeting those inputs so they do
            // not block future replacements.
            self.cleanup_rbf_for_removed_entries(removed_entries.iter())
                .await;
            self.wake_ordered_resolver_if_needed().await;
        }
        !removed_entries.is_empty()
    }

    /// Returns true if every parent of `tx` that is not already on-chain is
    /// currently in the tx-pool or one of the pipeline queues.
    ///
    /// This is used by the ordered resolver to decide whether a local orphan
    /// with missing inputs should retry without burning an attempt. We only
    /// skip the attempt counter when all missing parents are actually in flight;
    /// if any parent is permanently missing, the attempt counter must advance so
    /// that the orphan is eventually rejected.
    pub(crate) async fn all_missing_parents_in_flight(&self, parents: &HashSet<Byte32>) -> bool {
        // Collect parents that are neither on-chain nor already in the pool, while
        // holding a single read guard. This avoids re-acquiring the tx_pool lock
        // for every missing parent.
        let missing_ids: Vec<ProposalShortId> = {
            let pool = self.tx_pool.read().await;
            let snapshot = pool.cloned_snapshot();
            parents
                .iter()
                .filter(|h| !snapshot.transaction_exists(h))
                .map(ProposalShortId::from_tx_hash)
                .filter(|id| !pool.contains_proposal_id(id))
                .collect()
        };
        if missing_ids.is_empty() {
            return true;
        }

        // Read-only scan: take each async lock once outside the loop instead
        // of re-acquiring both per parent. Acquisition follows the documented
        // lock order (ordered_resolve_queue -> verify_queue); pre_check_queue
        // is a std Mutex with short critical sections and is locked per call.
        let ordered = self.queues.ordered_resolve_queue.read().await;
        let verify = self.queues.verify_queue.read().await;
        for parent_id in missing_ids {
            if ordered.contains_key(&parent_id) {
                continue;
            }
            if verify.contains_key(&parent_id) {
                continue;
            }
            if self.queues.pre_check_queue.contains_key(&parent_id) {
                continue;
            }
            return false;
        }
        true
    }
}
