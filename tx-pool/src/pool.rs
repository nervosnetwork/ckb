//! Top-level Pool type, methods, and tests
use super::component::{TxEntry, tx_selector::TxSelector};
use crate::component::conflict_cache::ConflictCache;
use crate::component::pool_map::{PoolEntry, PoolMap, PoolMapAddOutcome, RemovedPoolEntry, Status};
use crate::constants::{MAX_ESTIMATE_TARGET, MIN_ESTIMATE_TARGET};
use crate::error::Reject;
use crate::pool_cell::PoolCell;
use crate::tx_source::TxSource;
use ckb_app_config::TxPoolConfig;
use ckb_fee_estimator::Error as FeeEstimatorError;
use ckb_logger::{debug, warn};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::{BlockNumber, FeeRate};
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        Capacity, Cycle, TransactionView,
        cell::{
            CellChecker, OverlayCellChecker, OverlayCellProvider, ResolvedTransaction,
            resolve_transaction,
        },
        tx_pool::{PoolTxDetailInfo, TxPoolEntryInfo, TxPoolIds},
    },
    packed::{Byte32, ProposalShortId},
};
use lru::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

pub(crate) mod rbf;

/// Cache size for recently committed transaction hashes.
///
/// Used to short-circuit re-submission of transactions that were already committed in a
/// recent block. 100k entries is enough to cover several hours of main-chain traffic
/// under normal load without consuming excessive memory.
const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;

fn reject_full_for_evicted(entry: &TxEntry) -> Reject {
    Reject::Full(format!(
        "the fee_rate for this transaction is: {}",
        entry.fee_rate()
    ))
}

/// Read-only executable precondition for a historical cache handoff.
///
/// The conflict cache is an owner, not a speculative resolver queue. Moving a
/// trusted entry out of it while any referenced cell is still unavailable
/// would make the coordinator reject the transaction terminally and erase the
/// only future wake edge. Use the same overlay liveness semantics as normal
/// resolution, without materializing another `ResolvedTransaction` under the
/// pool write lock.
#[cfg(test)]
#[path = "tests/pool_seam.rs"]
mod test_seam;

fn conflict_recovery_ready(
    pool_map: &PoolMap,
    snapshot: &Snapshot,
    tx: &TransactionView,
    recovery_outpoints: &[OutPoint],
) -> bool {
    if pool_map.find_conflict_outpoint(tx).is_some() {
        return false;
    }
    let pool_cell = PoolCell::new(pool_map, false);
    let checker = OverlayCellChecker::new(&pool_cell, snapshot);
    recovery_outpoints
        .iter()
        .all(|out_point| checker.is_live(out_point) == Some(true))
}

/// Tx-pool implementation
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    pub(crate) pool_map: PoolMap,
    /// cache for committed transactions hash
    pub(crate) committed_txs_hash_cache: LruCache<ProposalShortId, Byte32>,
    /// storage snapshot reference
    pub(crate) snapshot: Arc<Snapshot>,
    // expiration milliseconds,
    pub(crate) expiry: u64,
    /// Historical, non-executable cache of verified transactions blocked by
    /// currently accepted pool inputs. Re-entry always goes through the
    /// coordinator.
    pub(crate) conflict_cache: ConflictCache,
    /// Whether the post-startup reconcile (`remove_onchain_entries`) has run.
    /// It exists for exactly one window — reorg notifications skipped during
    /// the startup reload — so it runs once on the first reorg and is then
    /// skipped: a full scan with a store lookup per entry is too expensive
    /// to repeat on every block.
    pub(crate) onchain_reconcile_done: bool,
    /// One-shot fault injection for the reorg status-transition boundary.
    /// Production builds have neither the field nor the branch.
    #[cfg(test)]
    pub(crate) fail_next_status_transition: bool,
    /// One-shot panic injection at the final pool commit boundary. Production
    /// has no corresponding branch; this proves the service-wide fail-close
    /// guard rather than a coordinator-only unit transition.
    #[cfg(test)]
    pub(crate) fail_next_pool_commit_panic: bool,
}

impl TxPool {
    /// Create new TxPool
    pub fn new(config: TxPoolConfig, snapshot: Arc<Snapshot>) -> TxPool {
        let expiry = config.expiry_hours as u64 * 60 * 60 * 1000;
        TxPool {
            pool_map: PoolMap::new(config.max_ancestors_count),
            committed_txs_hash_cache: LruCache::new(COMMITTED_HASH_CACHE_SIZE),
            config,
            snapshot,
            expiry,
            conflict_cache: ConflictCache::new(),
            onchain_reconcile_done: false,
            #[cfg(test)]
            fail_next_status_transition: false,
            #[cfg(test)]
            fail_next_pool_commit_panic: false,
        }
    }

    /// Tx-pool owned snapshot, it may not consistent with chain cause tx-pool update snapshot asynchronously
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Makes a clone of the `Arc<Snapshot>`
    pub(crate) fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Check whether tx-pool enable RBF
    pub fn enable_rbf(&self) -> bool {
        self.config.min_rbf_rate > self.config.min_fee_rate
    }

    /// The least required fee for a replacement transaction to be accepted.
    ///
    /// The incremental RBF fee is computed against the replacement transaction's
    /// serialized `size`, not its weight. This matches how `min_fee_rate` is
    /// enforced at submission time (see [`check_tx_fee_with_min_fee_rate`]),
    /// because cycles are not reliably available when a tx is first submitted.
    /// Using size here keeps the replacement threshold consistent with the pool's
    /// normal fee policy.
    pub fn min_replace_fee(&self, tx: &TxEntry) -> Option<Capacity> {
        if !self.enable_rbf() {
            return None;
        }

        let entry = self.get_pool_entry(&tx.proposal_short_id())?;
        let mut conflicts = vec![entry];
        let descendants = self.pool_map.calc_descendants(&tx.proposal_short_id());
        let descendants = descendants
            .iter()
            .filter_map(|id| self.get_pool_entry(id))
            .collect::<Vec<_>>();
        conflicts.extend(descendants);
        self.calculate_min_replace_fee(&conflicts, tx.size as u64)
    }

    /// Add tx with pending status
    /// If did have this value present, false is returned.
    pub(crate) fn add_pending(&mut self, entry: TxEntry) -> Result<PoolMapAddOutcome, Reject> {
        self.pool_map.add_entry(entry, Status::Pending)
    }

    /// Add tx which proposed but still uncommittable to gap
    pub(crate) fn add_gap(&mut self, entry: TxEntry) -> Result<PoolMapAddOutcome, Reject> {
        self.pool_map.add_entry(entry, Status::Gap)
    }

    /// Add tx with proposed status
    pub(crate) fn add_proposed(&mut self, entry: TxEntry) -> Result<PoolMapAddOutcome, Reject> {
        self.pool_map.add_entry(entry, Status::Proposed)
    }

    /// Returns true if the tx-pool contains a tx with specified id.
    pub(crate) fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pool_map.get_by_id(id).is_some()
    }

    pub(crate) fn record_conflict(&mut self, tx: TransactionView, source: TxSource) {
        self.record_conflict_with_release(tx, source, None);
    }

    pub(crate) fn record_conflict_entry(&mut self, entry: &TxEntry, source: TxSource) {
        self.record_conflict_entry_with_release(entry, source, None);
    }

    pub(crate) fn record_conflict_entry_for_release(
        &mut self,
        entry: &TxEntry,
        source: TxSource,
        release_event: Arc<crate::component::conflict_cache::ConflictReleaseEvent>,
    ) {
        self.record_conflict_entry_with_release(entry, source, Some(release_event));
    }

    fn record_conflict_entry_with_release(
        &mut self,
        entry: &TxEntry,
        source: TxSource,
        release_event: Option<Arc<crate::component::conflict_cache::ConflictReleaseEvent>>,
    ) {
        let tx = entry.transaction().clone();
        let recovery_outpoints = tx
            .input_pts_iter()
            .chain(entry.related_dep_out_points().cloned())
            .collect::<Vec<_>>();
        self.record_conflict_with_release_and_outpoints(
            tx,
            source,
            release_event,
            Some(recovery_outpoints),
        );
    }

    fn record_conflict_with_release(
        &mut self,
        tx: TransactionView,
        source: TxSource,
        release_event: Option<Arc<crate::component::conflict_cache::ConflictReleaseEvent>>,
    ) {
        self.record_conflict_with_release_and_outpoints(tx, source, release_event, None);
    }

    fn record_conflict_with_release_and_outpoints(
        &mut self,
        tx: TransactionView,
        source: TxSource,
        release_event: Option<Arc<crate::component::conflict_cache::ConflictReleaseEvent>>,
        recovery_outpoints: Option<Vec<OutPoint>>,
    ) {
        let short_id = tx.proposal_short_id();
        let (_added, evicted) = match recovery_outpoints {
            Some(recovery_outpoints) => self.conflict_cache.insert_with_outpoints_for_release(
                tx,
                source,
                recovery_outpoints,
                release_event,
            ),
            None => match release_event {
                Some(release_event) => {
                    self.conflict_cache
                        .insert_for_release(tx, source, Some(release_event))
                }
                None => self.conflict_cache.insert(tx, source),
            },
        };
        if !evicted.is_empty() {
            // Budget-pressure evictions are otherwise silent: a recoverable
            // transaction disappearing without a trace makes recovery
            // accounting impossible to debug.
            warn!(
                "conflict cache evicted {} entries under budget pressure while recording {}",
                evicted.len(),
                short_id
            );
        }
        debug!(
            "record_conflict {:?} now room size: {}",
            short_id,
            self.conflict_cache.len()
        );
    }

    pub(crate) fn remove_conflict_hash(&mut self, hash: &Byte32) -> bool {
        let removed = self.conflict_cache.remove_hash(hash);
        debug!(
            "remove_conflict_hash {:?} now room size: {}",
            hash,
            self.conflict_cache.len()
        );
        removed
    }

    /// Register freed inputs for bounded discovery. The pool mutation never
    /// walks the conflict-cache fan-out; maintenance probes candidates and
    /// performs the later cache→coordinator transfer in separate slices.
    pub(crate) fn schedule_conflicted_txs_from_inputs(
        &mut self,
        inputs: impl Iterator<Item = OutPoint>,
    ) -> usize {
        self.conflict_cache
            .schedule_discovery_by_inputs(inputs, None)
    }

    /// Project physical pool removals onto the active chain's semantic
    /// availability delta. Removing a stale overlay for a transaction that is
    /// already committed does not free any of that transaction's inputs: the
    /// chain still consumes them. Keeping this projection beside the pool
    /// snapshot prevents each removal path (RBF, reorg and administration)
    /// from independently getting that distinction wrong.
    pub(crate) fn released_inputs_from_removed_entries<'a>(
        &self,
        removed: impl IntoIterator<Item = &'a TxEntry>,
    ) -> Vec<OutPoint> {
        removed
            .into_iter()
            .filter(|entry| {
                !self
                    .snapshot
                    .transaction_exists(&entry.transaction().hash())
            })
            .flat_map(|entry| entry.transaction().input_pts_iter())
            .collect()
    }

    pub(crate) fn schedule_conflicted_txs_from_inputs_for_release(
        &mut self,
        inputs: impl Iterator<Item = OutPoint>,
        release_event: Arc<crate::component::conflict_cache::ConflictReleaseEvent>,
    ) -> usize {
        self.conflict_cache
            .schedule_discovery_by_inputs(inputs, Some(release_event))
    }

    pub(crate) fn discover_conflicted_txs(
        &mut self,
        limit: usize,
    ) -> crate::component::conflict_cache::ConflictDiscoveryProgress {
        let pool_map = &self.pool_map;
        let snapshot = &self.snapshot;
        self.conflict_cache
            .discover_recoverable(limit, |tx, recovery_outpoints| {
                conflict_recovery_ready(pool_map, snapshot, tx, recovery_outpoints)
            })
    }

    pub(crate) fn pop_conflict_recovery(
        &mut self,
    ) -> Option<crate::component::conflict_cache::ConflictRecoveryCandidate> {
        self.conflict_cache.pop_recovery_candidate()
    }

    pub(crate) fn conflict_recovery_ready(
        &self,
        tx: &TransactionView,
        recovery_outpoints: &[OutPoint],
    ) -> bool {
        conflict_recovery_ready(&self.pool_map, self.snapshot(), tx, recovery_outpoints)
    }

    pub(crate) fn reschedule_conflict_recovery(&mut self, hash: &Byte32) -> bool {
        self.conflict_cache.reschedule_recovery(hash)
    }

    pub(crate) fn conflict_recovery_len(&self) -> usize {
        self.conflict_cache.recovery_len()
    }

    pub(crate) fn conflict_discovery_len(&self) -> usize {
        self.conflict_cache.discovery_len()
    }

    /// Level-triggered predicate for cache-owned maintenance. Registration
    /// can coalesce into an already-live discovery cursor and therefore add
    /// zero new tickets; callers must still issue a wake whenever work is
    /// present rather than interpreting the registration count as authority.
    pub(crate) fn conflict_maintenance_pending(&self) -> bool {
        self.conflict_recovery_len() != 0 || self.conflict_discovery_len() != 0
    }

    pub(crate) fn clear_conflict_recovery_schedule(&mut self) {
        self.conflict_cache.clear_recovery_schedule();
    }

    /// Returns tx with cycles corresponding to the id.
    pub(crate) fn get_tx_with_cycles(
        &self,
        id: &ProposalShortId,
    ) -> Option<(TransactionView, Cycle)> {
        self.pool_map
            .get_by_id(id)
            .map(|entry| (entry.inner.transaction().clone(), entry.inner.cycles))
    }

    pub(crate) fn get_pool_entry(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.pool_map.get_by_id(id)
    }

    pub(crate) fn get_tx_from_pool(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.pool_map
            .get_by_id(id)
            .map(|entry| entry.inner.transaction())
    }

    /// Return an accepted transaction only when the complete raw hash
    /// matches. Proposal short IDs are lookup accelerators, never identity.
    pub(crate) fn get_tx_from_pool_by_hash(&self, hash: &Byte32) -> Option<&TransactionView> {
        self.pool_map
            .get_by_hash(hash)
            .map(|entry| entry.inner.transaction())
    }

    pub(crate) fn remove_committed_txs<'a>(
        &mut self,
        txs: impl Iterator<Item = &'a TransactionView>,
        detached_headers: &HashSet<Byte32>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        for tx in txs {
            // Attached-block accessors are views into the whole block. This
            // LRU outlives reorg processing, so materialize both cached values
            // before a 10/32-byte key retains an entire block allocation.
            let tx_hash = crate::util::compact_packed(&tx.hash());
            let short_id = crate::util::compact_packed(&tx.proposal_short_id());
            debug!("try remove_committed_tx {}", tx_hash);
            self.remove_committed_tx(tx, reject_events);

            self.committed_txs_hash_cache.put(short_id, tx_hash);
        }

        if !detached_headers.is_empty() {
            self.resolve_conflict_header_dep(detached_headers, reject_events)
        }
    }

    fn resolve_conflict_header_dep(
        &mut self,
        detached_headers: &HashSet<Byte32>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        for (entry, reject) in self.pool_map.resolve_conflict_header_dep(detached_headers) {
            reject_events.push((entry, reject));
        }
    }

    fn remove_committed_tx(
        &mut self,
        tx: &TransactionView,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        let short_id = tx.proposal_short_id();
        let exact_resident = self.pool_map.get_by_hash(&tx.hash()).is_some();
        if exact_resident && self.pool_map.remove_entry(&short_id).is_some() {
            debug!("remove_committed_tx for {}", tx.hash());
        }
        {
            for (entry, reject) in self.pool_map.resolve_conflict(tx) {
                debug!(
                    "removed {} for committed: {}",
                    entry.transaction().hash(),
                    tx.hash()
                );
                reject_events.push((entry, reject));
            }
        }
    }

    // Expire all transaction (and their dependencies) in the pool.
    pub(crate) fn remove_expired(&mut self, reject_events: &mut Vec<(TxEntry, Reject)>) {
        let now_ms = ckb_systemtime::unix_time_as_millis();

        let expired: Vec<_> = self
            .pool_map
            .iter()
            .filter(|&entry| self.expiry + entry.inner.timestamp < now_ms)
            .map(|entry| entry.id.clone())
            .collect();

        for id in expired {
            // Cascade to descendants: an expired parent's children can
            // never resolve (its outputs die with it), so leaving them
            // would make them zombies until their own expiry — matching
            // `limit_size` and this function's own documentation. Entries
            // already taken down by an earlier cascade in this same pass
            // simply yield an empty removal here.
            let removed = self.pool_map.remove_entry_and_descendants(&id);
            for entry in removed {
                let tx_hash = entry.transaction().hash();
                debug!("remove_expired {} timestamp({})", tx_hash, entry.timestamp);
                let reject = Reject::Expiry(entry.timestamp);
                reject_events.push((entry, reject));
            }
        }
    }

    /// One-shot post-startup reconcile: drop pool entries that can no
    /// longer be valid, returning the removed entries so the caller can run
    /// the related cleanups (in-flight RBF registrations whose conflict
    /// inputs were freed by these removals must not linger as ghosts).
    ///
    /// Reorg notifications are skipped while the node is still in its
    /// startup reload (`service_started == false`), so transactions
    /// committed during a long reload window would otherwise linger in the
    /// pool until expiry. Called once from the reorg write-lock section,
    /// which runs against a *fresh* snapshot — the first reorg after
    /// startup doubles as the reconcile for that window (see
    /// `onchain_reconcile_done` for why it does not run on every reorg).
    ///
    /// Two passes:
    ///   1. entries whose transaction is already committed on-chain
    ///      (children are kept: their inputs resolve on-chain);
    ///   2. zombies whose inputs/cell deps are neither produced in-pool nor
    ///      live on-chain, or whose header deps are no longer on the active
    ///      main chain (their dependencies changed in the skipped window).
    ///      Zombie removal cascades to descendants, which cannot resolve
    ///      either.
    pub(crate) fn remove_onchain_entries(&mut self) -> Vec<TxEntry> {
        let committed: Vec<ProposalShortId> = self
            .pool_map
            .iter()
            .filter(|entry| {
                self.snapshot
                    .transaction_exists(&entry.inner.transaction().hash())
            })
            .map(|entry| entry.id.clone())
            .collect();
        let mut removed = Vec::new();
        for id in committed {
            if let Some(entry) = self.pool_map.remove_entry(&id) {
                removed.push(entry);
            }
        }

        let zombies: Vec<ProposalShortId> = self
            .pool_map
            .iter()
            .filter(|entry| {
                let tx = entry.inner.transaction();
                let inputs: HashSet<OutPoint> = tx.input_pts_iter().collect();
                // An input is dead when it is neither produced in-pool nor
                // live on-chain. The pool-side check goes through `entries`
                // (the authoritative set), not `links`.
                let input_dead = inputs.iter().any(|out_point| {
                    let parent_in_pool = self.pool_map.get_by_hash(&out_point.tx_hash()).is_some();
                    !parent_in_pool && self.snapshot.get_cell(out_point).is_none()
                });
                let header_dead = tx
                    .header_deps_iter()
                    .any(|header| !self.snapshot.is_main_chain(&header));
                input_dead
                    || entry.inner.related_dep_out_points().any(|dep| {
                        // A dep that is also an input of this same tx is
                        // consumed by the tx itself — exempt, mirroring
                        // `pre_validate_entry_deps`.
                        if inputs.contains(dep) {
                            return false;
                        }
                        let producer_in_pool = self.pool_map.get_by_hash(&dep.tx_hash()).is_some();
                        !producer_in_pool && self.snapshot.get_cell(dep).is_none()
                    })
                    || header_dead
            })
            .map(|entry| entry.id.clone())
            .collect();
        for id in zombies {
            removed.extend(self.pool_map.remove_entry_and_descendants(&id));
        }
        removed
    }

    // Remove transactions until both serialized and resolved-residency
    // budgets are satisfied.
    // Return a `Reject` for current inserting entry if it's removed
    pub(crate) fn limit_size(
        &mut self,
        current_entry_id: Option<&ProposalShortId>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) -> Option<Reject> {
        self.limit_size_inner(current_entry_id, reject_events, None)
    }

    /// Transactional form of [`Self::limit_size`]. Every physical removal is
    /// exported with its original status so a caller that rejects the current
    /// insertion can restore the exact pre-commit pool before releasing the
    /// write guard.
    pub(crate) fn limit_size_with_journal(
        &mut self,
        current_entry_id: Option<&ProposalShortId>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
        removal_journal: &mut Vec<RemovedPoolEntry>,
    ) -> Option<Reject> {
        self.limit_size_inner(current_entry_id, reject_events, Some(removal_journal))
    }

    fn limit_size_inner(
        &mut self,
        current_entry_id: Option<&ProposalShortId>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
        mut removal_journal: Option<&mut Vec<RemovedPoolEntry>>,
    ) -> Option<Reject> {
        let mut ret = None;
        let resident_limit = self.config.tx_pool_resident_size_budget();
        let mut repaired_counters = false;
        while self.pool_map.stats.total_tx_size > self.config.max_tx_pool_size
            || self.pool_map.stats.total_tx_resident_size > resident_limit
        {
            let next_evict_entry = || {
                self.pool_map
                    .next_evict_entry(Status::Pending)
                    .or_else(|| self.pool_map.next_evict_entry(Status::Gap))
                    .or_else(|| self.pool_map.next_evict_entry(Status::Proposed))
            };

            if let Some(id) = next_evict_entry() {
                let removed = self.pool_map.remove_entry_and_descendants_with_status(&id);
                for removed in removed {
                    // The ordinary path keeps the old move-only behavior.
                    // Clone a `TxEntry` only when a transactional caller must
                    // retain the exact undo record as well as its reject event.
                    let entry = match removal_journal.as_deref_mut() {
                        Some(journal) => {
                            let entry = removed.entry.clone();
                            journal.push(removed);
                            entry
                        }
                        None => removed.entry,
                    };
                    let tx_hash = entry.transaction().hash();
                    debug!(
                        "Removed by size limit {} timestamp({})",
                        tx_hash, entry.timestamp
                    );
                    let reject = reject_full_for_evicted(&entry);
                    if let Some(short_id) = current_entry_id
                        && entry.proposal_short_id() == *short_id
                    {
                        ret = Some(reject.clone());
                    }
                    reject_events.push((entry, reject));
                }
            } else {
                // The status index covers every entry, so an empty eviction
                // scan can only mean cached totals drifted (typically high)
                // or the multi-index itself is corrupt. Rebuild once from the
                // authoritative entries. Silently returning over budget would
                // turn an invariant failure into an attacker-retainable state.
                assert!(
                    !repaired_counters,
                    "tx-pool remains over capacity after authoritative counter repair"
                );
                self.pool_map
                    .repair_total_statistics("limit_size_without_candidate");
                repaired_counters = true;
            }
        }
        ret
    }

    // remove transaction with detached proposal from gap and proposed
    // try re-put to pending
    pub(crate) fn remove_by_detached_proposal<'a>(
        &mut self,
        ids: impl Iterator<Item = &'a ProposalShortId>,
        notify_events: &mut Vec<(TxEntry, Status)>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        // Remove the complete union before re-adding anything. Re-adding
        // after each root lets overlapping detached roots process the same
        // descendant repeatedly (a chain of N detached proposal IDs can
        // otherwise generate O(N^2) pool mutations and notifications).
        let mut entries = Vec::new();
        for id in ids {
            if let Some(e) = self.pool_map.get_by_id(id) {
                let status = e.status;
                if status == Status::Pending {
                    continue;
                }
                entries.extend(self.pool_map.remove_entry_and_descendants(id));
            }
        }

        // The captured ancestor counts describe the pre-removal DAG. Parent-
        // first replay reconstructs it while every entry is handled exactly
        // once, independent of detached-ID iteration order.
        entries.sort_unstable_by_key(|entry| entry.ancestors_count);
        for mut entry in entries {
            let tx_hash = entry.transaction().hash();
            entry.reset_statistic_state();
            match self.add_pending(entry.clone()) {
                Ok(PoolMapAddOutcome {
                    inserted: true,
                    evicted,
                }) => {
                    // Re-pending notifications are collected and dispatched
                    // by the caller outside the write lock (user callbacks
                    // must not run in-lock).
                    notify_events.push((entry.clone(), Status::Pending));
                    for removed in evicted {
                        let evict = removed.entry;
                        let reject = reject_full_for_evicted(&evict);
                        reject_events.push((evict, reject));
                    }
                }
                Ok(PoolMapAddOutcome {
                    inserted: false, ..
                }) => {
                    panic!(
                        "detached-proposal replay found an impossible duplicate short-id owner for {tx_hash}"
                    )
                }
                Err(reject) => {
                    // PoolMap restores its exact local escape-hatch cohort
                    // before returning this error, so this caller only owns
                    // the replay entry's rejection.
                    reject_events.push((entry.clone(), reject.clone()));
                }
            }
            debug!("remove_by_detached_proposal {} replayed", tx_hash);
        }
    }

    pub(crate) fn remove_tx(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        self.pool_map.remove_entry_and_descendants(id)
    }

    pub(crate) fn check_rtx_from_pool(&self, rtx: &ResolvedTransaction) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let pool_cell = PoolCell::new(&self.pool_map, false);
        let checker = OverlayCellChecker::new(&pool_cell, snapshot);
        let mut seen_inputs = HashSet::new();
        rtx.check(&mut seen_inputs, &checker, snapshot)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn resolve_tx_from_pool(
        &self,
        tx: TransactionView,
        rbf: bool,
    ) -> Result<Arc<ResolvedTransaction>, Reject> {
        let snapshot = self.snapshot();
        let pool_cell = PoolCell::new(&self.pool_map, rbf);
        let provider = OverlayCellProvider::new(&pool_cell, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(tx, &mut seen_inputs, &provider, snapshot)
            .map(Arc::new)
            .map_err(Reject::Resolve)
    }

    /// Run the only fallible reorg status hook before snapshot or membership
    /// mutation. Production status plans are derived and applied under one
    /// write lock, so a later failure would be an internal invariant rather
    /// than a retryable transaction outcome.
    pub(crate) fn preflight_reorg_status_transitions(&mut self) -> Result<(), Reject> {
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_status_transition) {
            return Err(Reject::Malformed(
                "injected status transition failure".to_string(),
                "before reorg mutation".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn transition_to_status_required(
        &mut self,
        short_id: &ProposalShortId,
        target: Status,
    ) {
        let entry = self
            .get_pool_entry(short_id)
            .expect("reorg status plan references a live pool entry");
        let tx_hash = entry.inner.transaction().hash();
        assert_ne!(
            entry.status, target,
            "reorg status plan contains a redundant transition for {tx_hash}"
        );
        debug!("transition_to_status: {:?} => {:?}", tx_hash, target);
        self.pool_map.set_entry(short_id, target);
    }

    /// Get to-be-proposal transactions that may be included in the next block.
    pub(crate) fn get_ids(&self) -> TxPoolIds {
        let pending = self
            .pool_map
            .pending_gap_entries()
            .map(|entry| entry.transaction().hash())
            .collect();

        let proposed = self
            .pool_map
            .proposed_entries()
            .map(|entry| entry.transaction().hash())
            .collect();

        TxPoolIds { pending, proposed }
    }

    pub(crate) fn get_all_entry_info(&self) -> TxPoolEntryInfo {
        let pending = self
            .pool_map
            .pending_gap_entries()
            .map(|entry| (entry.transaction().hash(), entry.to_info()))
            .collect();

        let proposed = self
            .pool_map
            .proposed_entries()
            .map(|entry| (entry.transaction().hash(), entry.to_info()))
            .collect();

        let conflicted = self
            .conflict_cache
            .entries()
            .map(|entry| entry.tx.hash())
            .collect();
        TxPoolEntryInfo {
            pending,
            proposed,
            conflicted,
        }
    }

    /// Collect all accepted transactions in authoritative dependency order.
    ///
    /// `PoolMap`'s graph includes verified dep-group expansion while a raw
    /// transaction-only sort cannot. Parent entries always have a strictly
    /// smaller `ancestors_count` than their children, so this cold-path order
    /// is sufficient for serial persistence replay and independent chains use
    /// full hash as a deterministic tie-breaker.
    pub(crate) fn get_all_txs(&self) -> Vec<TransactionView> {
        let mut entries = self.pool_map.iter().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| {
            left.inner
                .ancestors_count
                .cmp(&right.inner.ancestors_count)
                .then_with(|| {
                    left.inner
                        .transaction()
                        .hash()
                        .cmp(&right.inner.transaction().hash())
                })
        });
        entries
            .into_iter()
            .map(|entry| entry.inner.transaction().clone())
            .collect()
    }

    pub(crate) fn clear(&mut self, snapshot: Arc<Snapshot>) {
        self.pool_map.clear();
        self.snapshot = snapshot;
        self.committed_txs_hash_cache = LruCache::new(COMMITTED_HASH_CACHE_SIZE);
        self.conflict_cache.clear();
    }

    pub(crate) fn package_proposals(&self, proposals_limit: u64) -> Vec<ProposalShortId> {
        // Select proposals independently of the template's optional uncles.
        // The block assembler atomically filters conflicting uncles after this
        // selection. Excluding here can strand a recovered Pending tx forever
        // when a miner validly omits the returned uncle: the candidate remains
        // eligible, so every subsequent template excludes the same short id.
        self.pool_map
            .score_sorted_iter_by_status(Status::Pending)
            .map(|entry| entry.proposal_short_id())
            .take(proposals_limit as usize)
            .collect()
    }

    pub(crate) fn package_txs(
        &self,
        max_block_cycles: Cycle,
        txs_size_limit: usize,
    ) -> (Vec<TxEntry>, usize, Cycle) {
        let (entries, size, cycles) =
            TxSelector::new(&self.pool_map).txs_to_commit(txs_size_limit, max_block_cycles);

        if !entries.is_empty() {
            ckb_logger::info!(
                "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                entries.len(),
                size,
                txs_size_limit,
                cycles,
                max_block_cycles
            );
        }
        (entries, size, cycles)
    }

    pub(crate) fn estimate_fee_rate(
        &self,
        target_to_be_committed: BlockNumber,
    ) -> Result<FeeRate, FeeEstimatorError> {
        if !(MIN_ESTIMATE_TARGET..=MAX_ESTIMATE_TARGET).contains(&target_to_be_committed) {
            return Err(FeeEstimatorError::NoProperFeeRate);
        }
        let closest = self.snapshot.consensus().tx_proposal_window().closest();
        let target_blocks = target_to_be_committed.saturating_sub(closest).max(1) as usize;
        let fee_rate = self.pool_map.estimate_fee_rate(
            target_blocks,
            self.snapshot.consensus().max_block_bytes() as usize,
            self.snapshot.consensus().max_block_cycles(),
            self.config.min_fee_rate,
        );
        Ok(fee_rate)
    }

    /// query the details of a transaction in the pool, only for trouble shooting
    pub(crate) fn get_tx_detail(&self, id: &ProposalShortId) -> Option<PoolTxDetailInfo> {
        if let Some(entry) = self.pool_map.get_by_id(id) {
            // Rank and counts come from the sorted index and pool stats
            // directly; building full id vectors via `get_ids()` would
            // allocate ~2x pool size for a single query.
            let rank_in_pending = if entry.status == Status::Proposed {
                0
            } else {
                self.pool_map
                    .pending_gap_entries()
                    .position(|pending| pending.proposal_short_id() == *id)
                    .unwrap_or_default()
                    + 1
            };
            let res = PoolTxDetailInfo {
                timestamp: entry.inner.timestamp,
                entry_status: entry.status.to_string(),
                pending_count: self.pool_map.pending_size(),
                rank_in_pending,
                proposed_count: self.pool_map.proposed_size(),
                descendants_count: self.pool_map.calc_descendants(id).len(),
                ancestors_count: self.pool_map.calc_ancestors(id).len(),
                score_sortkey: entry.inner.as_score_key().into(),
            };
            Some(res)
        } else {
            None
        }
    }
}
