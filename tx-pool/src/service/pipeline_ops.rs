//! Cross-structure pipeline operations.
//!
//! The pipeline's structure list — pre-check queue, ordered-resolve queue,
//! verify queue, RBF-held registrations, orphan pool, and the main pool —
//! is enumerated in exactly one place: this module. Every cross-structure
//! operation (duplicate/visibility checks, removal, lookup, clearing)
//! lives here instead of being hand-enumerated at each call site, so
//! adding a structure (e.g. a new queue or a new held set) touches one
//! file instead of ten.
//!
//! Lock acquisition inside a helper follows the documented hierarchy
//! (`ordered_resolve_queue → rbf_candidates → verify_queue → orphan →
//! tx_pool`), and guards are always acquired and released sequentially —
//! never nested — so these helpers cannot deadlock.

use crate::component::pipeline_queue::PipelineQueue;
use crate::service::{PipelineTxLocation, RemoveTxOutcome, TxPoolService};
use ckb_store::ChainStore;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, ProposalShortId};
use std::collections::HashSet;

impl TxPoolService {
    /// Returns true if the id is known to any of the three pipeline queues,
    /// counting worker-active (popped but unfinished) jobs as present.
    ///
    /// This is the visibility set used by the local-orphan flight check:
    /// a parent transaction counts as "in flight" for the whole time it is
    /// inside the pipeline, including the window between pop and finish.
    pub(crate) async fn queues_contain_or_active(&self, id: &ProposalShortId) -> bool {
        if self.pipeline.queues.pre_check_queue.contains_or_active(id) {
            return true;
        }
        {
            let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
            if ordered.contains_or_active(id) {
                return true;
            }
        }
        {
            let verify_queue = self.pipeline.queues.verify_queue.read().await;
            if verify_queue.contains_or_active(id) {
                return true;
            }
        }
        false
    }

    /// Returns true if the id is anywhere in the pipeline (queues and their
    /// active jobs, RBF-held registrations, orphan pool; not the main
    /// pool).
    pub(crate) async fn pipeline_contains(&self, id: &ProposalShortId) -> bool {
        if self.queues_contain_or_active(id).await {
            return true;
        }
        {
            // A displaced candidate parked as `RaceLost` in the waiting
            // room is not in any queue but is still inside the pipeline
            // lifecycle; `contains_key` covers every parked reason
            // (`RaceLost` included).
            let room = self.pipeline.waiting_room.read().await;
            if room.contains_key(id) {
                return true;
            }
        }
        false
    }

    /// Returns the ids of parents that are neither on-chain nor already in
    /// the pool — i.e. the transaction cannot be resolved *right now*.
    ///
    /// Parents that are merely in flight (queued or being processed) count
    /// as unavailable here: resolution would still fail. Queue visibility
    /// for the orphan-retry heuristic lives in
    /// [`Self::all_missing_parents_in_flight`].
    pub(crate) async fn unavailable_parent_ids(
        &self,
        parents: &HashSet<Byte32>,
    ) -> Vec<ProposalShortId> {
        // Collect parents that are neither on-chain nor already in the pool,
        // while holding a single read guard. This avoids re-acquiring the
        // tx_pool lock for every parent.
        let pool = self.pool.tx_pool.read().await;
        let snapshot = pool.cloned_snapshot();
        parents
            .iter()
            .filter(|h| !snapshot.transaction_exists(h))
            .map(ProposalShortId::from_tx_hash)
            .filter(|id| !pool.contains_proposal_id(id))
            .collect()
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
        // Collect parents that are neither on-chain nor already in the pool.
        let missing_ids = self.unavailable_parent_ids(parents).await;
        if missing_ids.is_empty() {
            return true;
        }

        // Parents parked in the waiting room count as in flight too:
        // orphans are re-driven once their own blocker resolves, and
        // RBF-held candidates resume when their winner leaves the pipeline.
        // Treating them as "not in flight" would burn the child's small
        // immediate-retry budget while its parent is merely parked.
        let missing_ids = {
            let room = self.pipeline.waiting_room.read().await;
            missing_ids
                .into_iter()
                .filter(|id| !room.contains_key(id))
                .collect::<Vec<_>>()
        };

        // `queues_contain_or_active` is used (not plain `contains_key`): a
        // parent that has been popped by a worker and is being processed
        // right now — pre-check classification, ordered resolution, or the
        // potentially seconds-long script verification — is still in
        // flight, and the orphan must not burn its small non-in-flight
        // retry budget while its parent is mid-flight.
        for parent_id in missing_ids {
            if !self.queues_contain_or_active(&parent_id).await {
                return false;
            }
        }
        true
    }

    /// Check if a transaction depends on any in-flight pipeline transaction
    /// (i.e. spends or references an output produced by a queued tx).
    pub(crate) async fn depends_on_pipeline(&self, tx: &TransactionView) -> bool {
        let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
        if ordered.depends_on(tx) {
            return true;
        }
        drop(ordered);
        let verify_queue = self.pipeline.queues.verify_queue.read().await;
        if verify_queue.depends_on(tx) {
            return true;
        }
        drop(verify_queue);
        self.pipeline.queues.pre_check_queue.depends_on(tx)
    }

    /// Remove a transaction by hash from every pipeline structure it may
    /// occupy (pre-check, ordered resolve, verify, RBF registrations,
    /// orphan and the main pool).
    pub(crate) async fn remove_tx(&self, tx_hash: Byte32) -> RemoveTxOutcome {
        let id = ProposalShortId::from_tx_hash(&tx_hash);
        if self
            .pipeline
            .queues
            .pre_check_queue
            .remove_by_id(&id)
            .is_some()
        {
            return RemoveTxOutcome::Removed;
        }
        {
            let mut queue = self.pipeline.queues.ordered_resolve_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                return RemoveTxOutcome::Removed;
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
            let mut rbf = self.pipeline.queues.rbf_candidates.write().await;
            let mut queue = self.pipeline.queues.verify_queue.write().await;
            if queue.remove_tx(&id).is_some() {
                rbf.remove(&id);
                let held = {
                    let mut room = self.pipeline.waiting_room.write().await;
                    room.wake_by_winner(&id)
                };
                drop(queue);
                drop(rbf);
                // Candidates held by the removed tx's registration are
                // restored: their displacer just left the pipeline.
                self.restore_held_rbf_candidates(held).await;
                // The removed tx may have had descendants waiting in the
                // ordered resolve queue. Wake the resolver so they can be
                // retried (and rejected if the parent is gone) promptly.
                self.wake_ordered_resolver_if_needed().await;
                return RemoveTxOutcome::Removed;
            }
        }
        // Do not return early on the waiting-room hit: the tx may be
        // double-parked (a `RaceLost` entry pipeline-side *and* an
        // `InputsBlocked` copy pool-side), so keep removing from the
        // remaining structures and report at the end.
        let mut found = false;
        {
            let mut orphan = self.pipeline.waiting_room.write().await;
            found |= orphan.remove(&id).is_some();
        }
        let (removed_entries, conflict_removed) = {
            let mut tx_pool = self.pool.tx_pool.write().await;
            // A copy parked pool-side (InputsBlocked conflict recovery)
            // must leave too, otherwise it lingers until budget eviction.
            let conflict_removed = tx_pool.remove_conflict(&id);
            (tx_pool.remove_tx(&id), conflict_removed)
        };
        if !removed_entries.is_empty() {
            // The removed pool entries have released their inputs. Clean up
            // any in-flight RBF candidates targeting those inputs so they do
            // not block future replacements.
            self.cleanup_rbf_for_removed_entries(removed_entries.iter())
                .await;
            self.wake_ordered_resolver_if_needed().await;
        }
        if found || conflict_removed || !removed_entries.is_empty() {
            return RemoveTxOutcome::Removed;
        }
        // Not found in any removable location. A worker may still be
        // processing it mid-flight (popped from a queue, not yet terminal):
        // report that honestly instead of "not found" — the caller would
        // otherwise watch the "missing" transaction enter the pool moments
        // later.
        if self
            .pipeline
            .queues
            .pre_check_queue
            .get_active_tx(&id)
            .is_some()
        {
            return RemoveTxOutcome::InProgress;
        }
        {
            let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
            if ordered.get_active_tx(&id).is_some() {
                return RemoveTxOutcome::InProgress;
            }
        }
        {
            let verify_queue = self.pipeline.queues.verify_queue.read().await;
            if verify_queue.get_active_tx(&id).is_some() {
                return RemoveTxOutcome::InProgress;
            }
        }
        RemoveTxOutcome::NotFound
    }

    /// Remove newly committed (attached) transactions from every pipeline
    /// structure, running the full terminal sequence for each: queue
    /// removal, registration removal, and — because the holder committed
    /// on-chain — *finalize* semantics for the candidates it held (their
    /// race is lost for real: relayed, but not recorded).
    pub(crate) async fn remove_attached_from_pipeline(&self, attached: &[ProposalShortId]) {
        for id in attached {
            self.pipeline.queues.pre_check_queue.remove_by_id(id);
            {
                let mut ordered = self.pipeline.queues.ordered_resolve_queue.write().await;
                ordered.remove_tx(id);
            }
            {
                // Maintain the global lock order rbf_candidates -> verify
                // (same as `remove_tx`).
                let _rbf = self.pipeline.queues.rbf_candidates.write().await;
                let mut queue = self.pipeline.queues.verify_queue.write().await;
                queue.remove_tx(id);
                drop(queue);
            }
            // The attached tx may hold an in-flight registration even when
            // it is no longer queued (e.g. popped by a verify worker):
            // committing on-chain makes its displacement real, so the held
            // candidates are really rejected (finalize), not restored.
            // This is a no-op for transactions without a registration.
            self.finalize_rbf_candidate(id).await;
            {
                let mut room = self.pipeline.waiting_room.write().await;
                room.remove(id);
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
        self.pipeline
            .queues
            .ordered_resolve_queue
            .write()
            .await
            .clear();
        self.pipeline.queues.rbf_candidates.write().await.clear();
        self.pipeline.queues.verify_queue.write().await.clear();
        self.pipeline.waiting_room.write().await.clear();
        // `pre_check_queue` uses a std::sync::Mutex, independent of the async
        // lock hierarchy; keep it last so it can never be held across an
        // `.await`.
        self.pipeline.queues.pre_check_queue.clear();
    }

    /// Search the pipeline queues for a transaction by short id.
    ///
    /// Transactions popped by a worker (active) and transactions held by an
    /// in-flight RBF registration count as in the pipeline too: from the
    /// caller's perspective they are still inside that stage's lifecycle.
    pub(crate) async fn find_tx_in_pipeline(
        &self,
        id: &ProposalShortId,
    ) -> Option<PipelineTxLocation> {
        if let Some(tx) = self
            .pipeline
            .queues
            .pre_check_queue
            .get_tx(id)
            .or_else(|| self.pipeline.queues.pre_check_queue.get_active_tx(id))
        {
            return Some(PipelineTxLocation::PreChecking { tx });
        }
        {
            let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
            if let Some(tx) = ordered
                .get_tx(id)
                .cloned()
                .or_else(|| ordered.get_active_tx(id))
            {
                return Some(PipelineTxLocation::Ordered { tx });
            }
        }
        {
            let verify_queue = self.pipeline.queues.verify_queue.read().await;
            if let Some(resolved) = verify_queue
                .get_tx_by_id(id)
                .or_else(|| verify_queue.get_active_tx(id))
            {
                return Some(PipelineTxLocation::Verifying {
                    tx: resolved.tx.clone(),
                    fee: resolved.fee,
                    status: resolved.status,
                });
            }
        }
        {
            // A displaced candidate parked in the waiting room as some
            // winner's `RaceLost` is not in any queue but is still inside
            // the verify-stage lifecycle (it may be restored at any moment).
            let room = self.pipeline.waiting_room.read().await;
            if let Some(entry) = room.find_held(id)
                && let Some(resolved) = entry.resolved.as_deref()
            {
                return Some(PipelineTxLocation::Verifying {
                    tx: resolved.tx.clone(),
                    fee: resolved.fee,
                    status: resolved.status,
                });
            }
        }
        {
            let orphan = self.pipeline.waiting_room.read().await;
            if let Some(entry) = orphan.get(id) {
                return Some(PipelineTxLocation::Orphan {
                    tx: entry.tx.clone(),
                    cycle: entry.source.cycles().unwrap_or(0),
                });
            }
        }
        None
    }

    /// Filter proposals down to those that are **completely new** to this
    /// node: not in any pipeline queue (or active), not RBF-held, not in
    /// the orphan pool, and not in the main pool.
    ///
    /// These locations are exactly the same stages searched by
    /// [`Self::get_tx_for_compact_block`], so filtering them out here is safe:
    /// a proposal marked as "known" can always be retrieved later for compact
    /// block reconstruction.
    pub async fn exclude_existing_proposal(
        &self,
        mut proposals: Vec<ProposalShortId>,
    ) -> Vec<ProposalShortId> {
        // Snapshot the held set once instead of scanning it per id
        // (O(proposals × held) otherwise).
        let held = {
            let room = self.pipeline.waiting_room.read().await;
            room.held_ids()
        };
        proposals.retain(|id| !held.contains(id));

        let mut unknown = Vec::with_capacity(proposals.len());
        for id in std::mem::take(&mut proposals) {
            if !self.pipeline_contains(&id).await {
                unknown.push(id);
            }
        }
        let mut proposals = unknown;
        {
            let tx_pool = self.pool.tx_pool.read().await;
            proposals.retain(|id| !tx_pool.contains_proposal_id(id));
        }
        proposals
    }

    /// Retrieves transactions required for compact block reconstruction.
    ///
    /// During compact block relay, a node may receive a block that contains transactions
    /// still being verified and not yet present in the main mempool. This method searches
    /// all locations where a transaction can reside when its short ID is known:
    ///
    /// 1. `ordered_resolve_queue` – transactions waiting for parent resolution
    /// 2. `pre_check_queue` – transactions awaiting pre-check by workers
    /// 3. `verify_queue` – transactions currently undergoing background validation
    /// 4. `rbf_candidates` – displaced transactions held by in-flight registrations
    /// 5. `orphan_pool` – orphan transactions waiting for missing parents
    /// 6. `pool_map` – the main mempool (already accepted transactions)
    ///
    /// # Returns
    /// A map containing only the transactions that were found, keyed by their short ID.
    /// Missing entries are simply omitted (caller should treat absence as "need to request")
    /// Returning a `HashMap` allows the caller (compact block reconstructor) to:
    /// - Immediately obtain all locally-available transactions in a single call
    /// - Quickly identify which short IDs are missing
    pub async fn get_tx_for_compact_block(
        &self,
        short_ids: HashSet<ProposalShortId>,
    ) -> std::collections::HashMap<ProposalShortId, TransactionView> {
        let mut txs = std::collections::HashMap::with_capacity(short_ids.len());
        {
            let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                ordered
                    .get_tx(short_id)
                    .cloned()
                    .or_else(|| ordered.get_active_tx(short_id))
                    .map(|tx| (short_id.to_owned(), tx))
            }));
        }
        txs.extend(short_ids.iter().filter_map(|short_id| {
            self.pipeline
                .queues
                .pre_check_queue
                .get_tx(short_id)
                .or_else(|| self.pipeline.queues.pre_check_queue.get_active_tx(short_id))
                .map(|tx| (short_id.to_owned(), tx))
        }));
        {
            let verify_queue = self.pipeline.queues.verify_queue.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                verify_queue
                    .get_tx_by_id(short_id)
                    .or_else(|| verify_queue.get_active_tx(short_id))
                    .map(|resolved| (short_id.to_owned(), resolved.tx.to_owned()))
            }));
        }
        {
            // Displaced candidates parked as `RaceLost` in the waiting room
            // are locally available even though they sit in no queue.
            let room = self.pipeline.waiting_room.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                room.find_held(short_id)
                    .and_then(|entry| entry.resolved.as_deref())
                    .map(|resolved| (short_id.to_owned(), resolved.tx.to_owned()))
            }));
        }
        {
            let orphan = self.pipeline.waiting_room.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                orphan
                    .get(short_id)
                    .map(|entry| (short_id.to_owned(), entry.tx.to_owned()))
            }));
        }
        {
            let tx_pool = self.pool.tx_pool.read().await;
            txs.extend(short_ids.iter().filter_map(|short_id| {
                tx_pool
                    .get_tx_from_pool_or_store(short_id)
                    .map(|tx| (short_id.to_owned(), tx))
            }));
        }
        txs
    }
}
