//! A unified waiting room: transactions that cannot proceed yet, indexed by
//! the event they are waiting for.
//!
//! Today it subsumes the orphan pool (`WaitReason::ParentsMissing`). RBF
//! in-flight losers (`RaceLost`) and conflict-cache entries
//! (`InputsBlocked`) are migrated onto the same structure in the later
//! stages of the waiting-room consolidation.

use crate::resolved_tx::ResolvedTx;
use crate::tx_source::TxSource;
use ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL;
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, OutPoint, ProposalShortId};
use ckb_util::shrink_to_fit;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::constants::SHRINK_THRESHOLD;

/// Expiration time for waiting transactions, expressed as a multiple of the
/// maximum block interval.
///
/// Entries whose parents never land are evicted after this window so that
/// out-of-order block/transaction propagation does not cause them to be
/// dropped prematurely. 100 block intervals is 4800 seconds (~80 minutes)
/// on main-net parameters.
pub(crate) const WAITING_ENTRY_EXPIRE_TIME: u64 = 100 * MAX_BLOCK_INTERVAL;

/// Entry-count and byte limits for `ParentsMissing` entries (the orphan
/// pool equivalent).
pub(crate) const MAX_PARENTS_MISSING_ENTRIES: usize = 100;
/// See [`MAX_PARENTS_MISSING_ENTRIES`].
pub(crate) const MAX_PARENTS_MISSING_TX_SIZE: usize = 20_000_000;

/// Entry-count and byte limits for `InputsBlocked` entries (same scale as
/// the retired conflicts-cache LRU).
pub(crate) const MAX_INPUTS_BLOCKED_ENTRIES: usize = 10_000;
/// See [`MAX_INPUTS_BLOCKED_ENTRIES`]: 10k average-size transactions.
pub(crate) const MAX_INPUTS_BLOCKED_TX_SIZE: usize = 50_000_000;

/// Why a waiting transaction cannot proceed yet.
#[derive(Debug, Clone)]
pub(crate) enum WaitReason {
    /// Some parents (inputs or cell-deps) are neither on-chain nor in the
    /// pool. Woken by: a parent entering the pool.
    ParentsMissing { parents: HashSet<Byte32> },
    /// Inputs are consumed by in-pool transactions (RBF victims, escape-hatch
    /// or size-limit evictions). Woken by: the consumer leaving the pool.
    InputsBlocked { inputs: HashSet<OutPoint> },
    /// Lost an in-flight RBF race to the named registration. Woken by: the
    /// winner's registration reaching a terminal state (commit → really
    /// rejected; abort → restored).
    RaceLost { winner: ProposalShortId },
}

/// A transaction parked in the waiting room.
#[derive(Debug, Clone)]
pub(crate) struct WaitingEntry {
    /// The transaction itself.
    pub tx: TransactionView,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// What it is waiting for.
    pub(crate) reason: WaitReason,
    /// The fully resolved form, carried by `RaceLost` entries so a restore
    /// can resume verification without re-resolving. `None` for the other
    /// reasons.
    pub(crate) resolved: Option<Box<ResolvedTx>>,
    /// Expire timestamp
    pub(crate) expires_at: u64,
}

impl WaitingEntry {
    /// Returns the peer index if this entry came from a remote source.
    pub fn peer(&self) -> Option<PeerIndex> {
        self.source.peer()
    }
}

/// Unified waiting room.
///
/// `by_id` owns the entries; `by_parent` indexes `ParentsMissing` entries
/// by each missing parent hash so a parent entering the pool can wake its
/// children directly (the orphan pool's per-outpoint index was coarser
/// than needed: every consumer unions over all of the parent's outputs
/// anyway); `by_winner` indexes `RaceLost` entries by the registration
/// they lost to; `by_outpoint` indexes `InputsBlocked` entries by every
/// blocked input so a freed outpoint finds its candidates directly.
///
/// `ParentsMissing` and `InputsBlocked` use reason-local budgets. `RaceLost`
/// owns an already-budgeted `ResolvedTx`: its lifecycle permit remains charged
/// across verify queue, active worker and waiting-room moves, so it must not be
/// charged or evicted a second time here.
///
/// Expiry applies only to `ParentsMissing` (orphan churn) and `RaceLost`
/// (a stalled winner must not hold the loser forever); `InputsBlocked`
/// entries have no expiry — like the retired conflicts LRU, they leave
/// only through recovery or budget eviction. The expiry scan itself is
/// gated by `expiry_watermark` (the smallest `expires_at` in the room), so
/// a `wait()` on the hot pool-side path does not pay a full-table scan
/// while nothing can be expired.
#[derive(Debug)]
pub(crate) struct WaitingRoom {
    by_id: HashMap<ProposalShortId, WaitingEntry>,
    by_parent: HashMap<Byte32, HashSet<ProposalShortId>>,
    by_winner: HashMap<ProposalShortId, HashSet<ProposalShortId>>,
    by_outpoint: HashMap<OutPoint, HashSet<ProposalShortId>>,
    /// Per-reason accounting (entry count + total serialized size).
    parents_missing: GroupStat,
    /// See `parents_missing`.
    inputs_blocked: GroupStat,
    /// The smallest `expires_at` currently in the room; the expiry scan in
    /// `limit_size` is skipped entirely while `now < expiry_watermark`.
    expiry_watermark: u64,
    /// Insertion-order eviction candidates per budgeted reason group.
    /// Budget eviction pops from the front instead of scanning `by_id`
    /// for an arbitrary entry (O(1) per eviction instead of O(n) per
    /// entry, O(k·n) per pass). Ids removed through other paths are left
    /// behind and skipped lazily when they reach the front.
    parents_missing_order: VecDeque<ProposalShortId>,
    /// See `parents_missing_order`.
    inputs_blocked_order: VecDeque<ProposalShortId>,
}

impl Default for WaitingRoom {
    fn default() -> Self {
        Self {
            by_id: HashMap::new(),
            by_parent: HashMap::new(),
            by_winner: HashMap::new(),
            by_outpoint: HashMap::new(),
            parents_missing: GroupStat::default(),
            inputs_blocked: GroupStat::default(),
            expiry_watermark: u64::MAX,
            parents_missing_order: VecDeque::new(),
            inputs_blocked_order: VecDeque::new(),
        }
    }
}

/// Per-reason size accounting for the budget checks.
#[derive(Default, Debug)]
struct GroupStat {
    len: usize,
    tx_size: usize,
}

impl WaitingRoom {
    /// Create an empty waiting room.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Number of `ParentsMissing` (orphan) entries — the `orphan_size`
    /// reported by `get_tx_pool_info`, kept distinct from the total so
    /// `RaceLost` entries do not inflate it.
    pub(crate) fn parents_missing_len(&self) -> usize {
        self.parents_missing.len
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.by_id.contains_key(id)
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&WaitingEntry> {
        self.by_id.get(id)
    }

    /// Returns the reason-local accounting group, or `None` for `RaceLost`
    /// entries already charged by their resolved-lifecycle permit.
    fn stat_for(&mut self, reason: &WaitReason) -> Option<&mut GroupStat> {
        match reason {
            WaitReason::ParentsMissing { .. } => Some(&mut self.parents_missing),
            WaitReason::InputsBlocked { .. } => Some(&mut self.inputs_blocked),
            WaitReason::RaceLost { .. } => None,
        }
    }

    /// Park a transaction.
    ///
    /// Returns `(true, evicted)` if the transaction was newly inserted
    /// and is still present after eviction, or `(false, evicted)` if it
    /// was already present or was evicted by the size limits.
    pub(crate) fn wait(
        &mut self,
        tx: TransactionView,
        source: TxSource,
        reason: WaitReason,
    ) -> (bool, Vec<WaitingEntry>) {
        let id = tx.proposal_short_id();
        if self.by_id.contains_key(&id) {
            return (false, vec![]);
        }

        debug!("waiting room add {}", tx.hash());
        if let Some(stat) = self.stat_for(&reason) {
            stat.len += 1;
            stat.tx_size = stat
                .tx_size
                .saturating_add(tx.data().serialized_size_in_block());
        }
        match &reason {
            WaitReason::ParentsMissing { parents } => {
                for parent in parents {
                    self.by_parent
                        .entry(parent.clone())
                        .or_default()
                        .insert(id.clone());
                }
                self.parents_missing_order.push_back(id.clone());
            }
            WaitReason::InputsBlocked { inputs } => {
                for input in inputs {
                    self.by_outpoint
                        .entry(input.clone())
                        .or_default()
                        .insert(id.clone());
                }
                self.inputs_blocked_order.push_back(id.clone());
            }
            WaitReason::RaceLost { winner } => {
                self.by_winner
                    .entry(winner.clone())
                    .or_default()
                    .insert(id.clone());
            }
        }
        // `InputsBlocked` entries never expire (like the retired conflicts
        // LRU); the other reasons get the standard expiry window.
        let expires_at = if matches!(reason, WaitReason::InputsBlocked { .. }) {
            u64::MAX
        } else {
            ckb_systemtime::unix_time().as_secs() + WAITING_ENTRY_EXPIRE_TIME
        };
        self.by_id.insert(
            id.clone(),
            WaitingEntry {
                tx,
                source,
                reason,
                resolved: None,
                expires_at,
            },
        );
        self.expiry_watermark = self.expiry_watermark.min(expires_at);

        // DoS prevention: the room must not grow unbounded.
        let evicted_txs = self.limit_size();
        let retained = self.by_id.contains_key(&id);
        (retained, evicted_txs)
    }

    /// Park a fully resolved transaction (a `RaceLost` candidate). Same
    /// contract as [`Self::wait`], additionally carrying the resolved form
    /// so a restore can resume verification without re-resolving.
    pub(crate) fn wait_resolved(
        &mut self,
        resolved: ResolvedTx,
        reason: WaitReason,
    ) -> (bool, Vec<WaitingEntry>) {
        let tx = resolved.tx.clone();
        let id = tx.proposal_short_id();
        // On duplicate, the newer wait reason wins: an entry parked under a
        // different reason (e.g. an orphan reclassified into the verify
        // queue and then displaced by a stronger candidate) must be
        // re-parked with the current reason instead of silently dropping
        // the candidate.
        let old = if self.by_id.contains_key(&id) {
            self.remove(&id)
        } else {
            None
        };
        // A re-park under the same RaceLost reason keeps the original
        // expiry: a stalled winner must not hold the loser forever, and
        // refreshing the expiry on every restore/re-hold cycle would
        // defeat it (expired → restored → re-held loops are bounded by the
        // *original* deadline).
        let preserve_expiry = old.as_ref().is_some_and(|entry| {
            matches!(entry.reason, WaitReason::RaceLost { .. })
                && matches!(reason, WaitReason::RaceLost { .. })
        });
        let old_expires_at = old.map(|entry| entry.expires_at);
        let (retained, evicted) = self.wait(tx, resolved.source, reason);
        if retained && let Some(entry) = self.by_id.get_mut(&id) {
            if preserve_expiry && let Some(expires_at) = old_expires_at {
                entry.expires_at = expires_at;
                self.expiry_watermark = self.expiry_watermark.min(expires_at);
            }
            entry.resolved = Some(Box::new(resolved));
        }
        (retained, evicted)
    }

    /// Remove an entry by id, un-indexing it from its wait reason.
    pub(crate) fn remove(&mut self, id: &ProposalShortId) -> Option<WaitingEntry> {
        let entry = self.by_id.remove(id)?;
        debug!("waiting room remove {}", entry.tx.hash());
        let size = entry.tx.data().serialized_size_in_block();
        match &entry.reason {
            WaitReason::ParentsMissing { .. } => {
                self.parents_missing.len = self.parents_missing.len.saturating_sub(1);
                self.parents_missing.tx_size = self.parents_missing.tx_size.saturating_sub(size);
            }
            WaitReason::InputsBlocked { .. } => {
                self.inputs_blocked.len = self.inputs_blocked.len.saturating_sub(1);
                self.inputs_blocked.tx_size = self.inputs_blocked.tx_size.saturating_sub(size);
            }
            WaitReason::RaceLost { .. } => {}
        }
        match &entry.reason {
            WaitReason::ParentsMissing { parents } => {
                for parent in parents {
                    if let Some(set) = self.by_parent.get_mut(parent) {
                        set.remove(id);
                        if set.is_empty() {
                            self.by_parent.remove(parent);
                        }
                    }
                }
            }
            WaitReason::InputsBlocked { inputs } => {
                for input in inputs {
                    if let Some(set) = self.by_outpoint.get_mut(input) {
                        set.remove(id);
                        if set.is_empty() {
                            self.by_outpoint.remove(input);
                        }
                    }
                }
            }
            WaitReason::RaceLost { winner } => {
                if let Some(set) = self.by_winner.get_mut(winner) {
                    set.remove(id);
                    if set.is_empty() {
                        self.by_winner.remove(winner);
                    }
                }
            }
        }
        Some(entry)
    }

    /// Wake and remove every entry that lost to `winner`, returning their
    /// resolved forms. The caller decides their fate: really rejected
    /// (winner committed) or restored (winner left the pipeline).
    pub(crate) fn wake_by_winner(&mut self, winner: &ProposalShortId) -> Vec<ResolvedTx> {
        let ids = self.by_winner.remove(winner).unwrap_or_default();
        ids.into_iter()
            // Go through `remove` so accounting and index maintenance stay
            // in exactly one place (`by_winner` was detached above, so
            // `remove` simply finds no winner set to update).
            .filter_map(|id| self.remove(&id))
            .filter_map(|entry| entry.resolved.map(|resolved| *resolved))
            .collect()
    }

    /// Look up a `RaceLost` entry by its own id (for pipeline query paths).
    pub(crate) fn find_held(&self, id: &ProposalShortId) -> Option<&WaitingEntry> {
        self.by_id
            .get(id)
            .filter(|entry| matches!(entry.reason, WaitReason::RaceLost { .. }))
    }

    /// Snapshot of every `RaceLost` entry id (for bulk filtering).
    pub(crate) fn held_ids(&self) -> HashSet<ProposalShortId> {
        self.by_winner.values().flatten().cloned().collect()
    }

    pub(crate) fn remove_many(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.remove(&id);
        }
        self.shrink_to_fit();
    }

    /// Remove all entries submitted by the given peer.
    ///
    /// Returns the short ids of the removed entries so callers can clean up
    /// any related in-flight state.
    pub(crate) fn remove_by_peer(&mut self, peer: PeerIndex) -> Vec<ProposalShortId> {
        let ids: Vec<_> = self
            .by_id
            .iter()
            .filter(|(_, entry)| entry.peer() == Some(peer))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.remove(id);
        }
        ids
    }

    pub(crate) fn clear(&mut self) {
        self.by_id.clear();
        self.by_parent.clear();
        self.by_winner.clear();
        self.by_outpoint.clear();
        self.parents_missing = GroupStat::default();
        self.inputs_blocked = GroupStat::default();
        self.parents_missing_order.clear();
        self.inputs_blocked_order.clear();
        // An empty room has no expirable entry: reset the watermark so the
        // next `wait()` does not run a pointless expiry scan.
        self.expiry_watermark = u64::MAX;
    }

    /// Force an entry's expiry into the past (and the watermark with it),
    /// so tests can drive the expiry scan deterministically.
    #[cfg(test)]
    pub(crate) fn expire_entry_for_test(&mut self, id: &ProposalShortId) {
        if let Some(entry) = self.by_id.get_mut(id) {
            entry.expires_at = 0;
            self.expiry_watermark = 0;
        }
    }

    /// Iterate `InputsBlocked` entries (the conflict-recovery set).
    pub(crate) fn inputs_blocked_entries(&self) -> impl Iterator<Item = &WaitingEntry> {
        self.by_id
            .values()
            .filter(|entry| matches!(entry.reason, WaitReason::InputsBlocked { .. }))
    }

    /// Ids of the entries waiting on the given parent transaction (spending
    /// or cell-depending on any of its outputs).
    pub(crate) fn find_by_parent(&self, parent: &TransactionView) -> Vec<&ProposalShortId> {
        self.by_parent
            .get(&parent.hash())
            .map(|set| set.iter().collect())
            .unwrap_or_default()
    }

    /// `InputsBlocked` candidates recoverable via any of the given inputs:
    /// every candidate whose inputs are *all* currently free (per the
    /// caller's liveness check). The liveness check is a callback so this
    /// query works for both the service-level room and the pool-level one:
    /// a candidate that still conflicts with the current pool state must
    /// not be recovered (it would be rejected again and, with both
    /// conflicting txs cached, can trigger an infinite recover/reject
    /// loop).
    pub(crate) fn recoverable_by_inputs(
        &self,
        inputs: impl Iterator<Item = OutPoint>,
        mut all_inputs_free: impl FnMut(&TransactionView) -> bool,
    ) -> Vec<(TransactionView, TxSource)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(ids) = self.by_outpoint.get(&input) {
                for id in ids {
                    if seen.insert(id.clone())
                        && let Some(entry) = self.by_id.get(id)
                        && matches!(entry.reason, WaitReason::InputsBlocked { .. })
                        && all_inputs_free(&entry.tx)
                    {
                        result.push((entry.tx.clone(), entry.source));
                    }
                }
            }
        }
        result
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.by_id, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_parent, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_winner, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_outpoint, SHRINK_THRESHOLD);
    }

    /// Evict expired entries first, then arbitrary entries from any reason
    /// group that is over its budget. Returns all evicted entries so the
    /// caller can route them by reason (orphan rejects, held restores).
    fn limit_size(&mut self) -> Vec<WaitingEntry> {
        let mut evicted = vec![];

        // The expiry scan runs only when the watermark says something might
        // be expired; on the hot path (nothing old enough) it is skipped
        // entirely instead of scanning the whole table per insert.
        // `InputsBlocked` entries carry `u64::MAX` as their expiry, so they
        // never enter the expired set and never pull the watermark down.
        let now = ckb_systemtime::unix_time().as_secs();
        if now >= self.expiry_watermark {
            let expires: Vec<_> = self
                .by_id
                .iter()
                .filter_map(|(id, entry)| (entry.expires_at <= now).then_some(id))
                .cloned()
                .collect();

            for id in expires {
                if let Some(entry) = self.remove(&id) {
                    evicted.push(entry);
                }
            }

            // Recompute the watermark from what is left (see the field doc:
            // removal leaves it stale-low, which only costs one scan per
            // removal until it is recomputed here).
            self.expiry_watermark = self
                .by_id
                .values()
                .map(|entry| entry.expires_at)
                .min()
                .unwrap_or(u64::MAX);
        }

        // Per-reason budget loops: evict the *oldest* entries of the
        // reason whose group is over budget (`RaceLost` is charged by the
        // resolved-lifecycle budget instead). Candidates come
        // from the per-reason insertion-order queues; ids already removed
        // through other paths are skipped lazily.
        loop {
            let parents_over = self.parents_missing.len > MAX_PARENTS_MISSING_ENTRIES
                || self.parents_missing.tx_size > MAX_PARENTS_MISSING_TX_SIZE;
            let blocked_over = self.inputs_blocked.len > MAX_INPUTS_BLOCKED_ENTRIES
                || self.inputs_blocked.tx_size > MAX_INPUTS_BLOCKED_TX_SIZE;
            if !parents_over && !blocked_over {
                break;
            }
            let order = if parents_over {
                &mut self.parents_missing_order
            } else {
                &mut self.inputs_blocked_order
            };
            let mut candidate = None;
            while let Some(id) = order.pop_front() {
                if self.by_id.contains_key(&id) {
                    candidate = Some(id);
                    break;
                }
            }
            let Some(id) = candidate else { break };
            if let Some(entry) = self.remove(&id) {
                evicted.push(entry);
            }
        }

        if !evicted.is_empty() {
            self.shrink_to_fit();
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::pool_map::Status;
    use crate::component::tests::util::build_tx;
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_db::RocksDB;
    use ckb_snapshot::Snapshot;
    use ckb_store::ChainDB;
    use ckb_types::U256;
    use ckb_types::core::{Capacity, cell::ResolvedTransaction};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn test_snapshot() -> Arc<Snapshot> {
        let tmp_dir = TempDir::new().expect("create temp dir");
        let consensus = Arc::new(ConsensusBuilder::default().build());
        let store = ChainDB::new(RocksDB::open_in(&tmp_dir, 6), Default::default());
        Arc::new(Snapshot::new(
            consensus.genesis_block().header(),
            U256::zero(),
            Default::default(),
            store.get_snapshot(),
            Default::default(),
            consensus,
        ))
    }

    fn dummy_resolved(tx: TransactionView) -> ResolvedTx {
        ResolvedTx {
            tx: tx.clone(),
            rtx: std::sync::Arc::new(ResolvedTransaction::dummy_resolve(tx.clone())),
            status: Status::Pending,
            fee: Capacity::zero(),
            tx_size: tx.data().serialized_size_in_block(),
            pre_resolve_tip: Default::default(),
            snapshot: test_snapshot(),
            source: TxSource::Local,
            epoch: 0,
            verified: None,
            resident_permit: None,
        }
    }

    /// A duplicate `wait_resolved` must re-park the entry with the newer
    /// wait reason (and its resolved form), never silently drop it: an
    /// orphan reclassified into the verify queue and then displaced must
    /// stay recoverable through the winner's lifecycle.
    #[test]
    fn wait_resolved_replaces_duplicate_with_new_reason() {
        let mut room = WaitingRoom::new();
        let tx = build_tx(vec![(&Byte32::zero(), 21)], 1);
        let id = tx.proposal_short_id();

        // Park as an orphan first.
        room.wait(
            tx.clone(),
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: tx.unique_parents(),
            },
        );
        assert!(matches!(
            room.get(&id).unwrap().reason,
            WaitReason::ParentsMissing { .. }
        ));

        // Displaced by a stronger candidate: the newer reason must win,
        // the resolved form must be carried, and the entry must stay
        // wakeable through the new winner.
        let winner = ProposalShortId::from_tx_hash(&Byte32::new([9u8; 32]));
        let (retained, _evicted) = room.wait_resolved(
            dummy_resolved(tx.clone()),
            WaitReason::RaceLost {
                winner: winner.clone(),
            },
        );
        assert!(retained);
        let entry = room.get(&id).unwrap();
        assert!(matches!(entry.reason, WaitReason::RaceLost { .. }));
        assert!(entry.resolved.is_some());

        let woken = room.wake_by_winner(&winner);
        assert_eq!(woken.len(), 1);
        assert_eq!(woken[0].tx.hash(), tx.hash());
        assert!(room.get(&id).is_none());
    }

    /// `wake_by_winner` must route through `remove` so accounting and the
    /// other indexes stay consistent: entries parked under other reasons
    /// (and their per-reason statistics) are untouched by the wake.
    #[test]
    fn wake_by_winner_keeps_other_reasons_and_stats_intact() {
        let mut room = WaitingRoom::new();
        let winner = ProposalShortId::from_tx_hash(&Byte32::new([9u8; 32]));

        let held_tx = build_tx(vec![(&Byte32::zero(), 31)], 1);
        let held_id = held_tx.proposal_short_id();
        room.wait_resolved(
            dummy_resolved(held_tx),
            WaitReason::RaceLost {
                winner: winner.clone(),
            },
        );

        let orphan_tx = build_tx(vec![(&Byte32::zero(), 32)], 1);
        let orphan_id = orphan_tx.proposal_short_id();
        room.wait(
            orphan_tx,
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: HashSet::from([Byte32::zero()]),
            },
        );

        let woken = room.wake_by_winner(&winner);
        assert_eq!(woken.len(), 1);
        assert!(room.get(&held_id).is_none());
        assert!(room.get(&orphan_id).is_some(), "orphan must be untouched");
        assert_eq!(room.len(), 1);
        assert_eq!(room.parents_missing_len(), 1);
        // Wake again: nothing left, no double-decrement of any accounting.
        assert!(room.wake_by_winner(&winner).is_empty());
        assert_eq!(room.parents_missing_len(), 1);
    }

    /// The expiry scan must be skipped entirely while now < watermark (no
    /// full-table scan per insert), and `InputsBlocked` entries never
    /// expire (like the retired conflicts LRU).
    #[test]
    fn expiry_scan_is_gated_by_watermark_and_skips_inputs_blocked() {
        let mut room = WaitingRoom::new();
        let now = ckb_systemtime::unix_time().as_secs();

        // An orphan that is already past its expiry on paper.
        let orphan_tx = build_tx(vec![(&Byte32::zero(), 22)], 1);
        let orphan_id = orphan_tx.proposal_short_id();
        room.wait(
            orphan_tx,
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: HashSet::from([Byte32::zero()]),
            },
        );
        room.by_id.get_mut(&orphan_id).unwrap().expires_at = now - 10;

        // An InputsBlocked entry (gets expires_at = u64::MAX from `wait`).
        let conflict_tx = build_tx(vec![(&Byte32::zero(), 23)], 1);
        let conflict_id = conflict_tx.proposal_short_id();
        room.wait(
            conflict_tx,
            TxSource::Local,
            WaitReason::InputsBlocked {
                inputs: HashSet::from([OutPoint::new(Byte32::zero(), 0)]),
            },
        );

        // Watermark in the future: the expiry scan must be skipped even
        // though the orphan is past its expiry.
        room.expiry_watermark = now + 3600;
        let filler = build_tx(vec![(&Byte32::zero(), 24)], 1);
        room.wait(
            filler,
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: HashSet::from([Byte32::new([1u8; 32])]),
            },
        );
        assert!(
            room.get(&orphan_id).is_some(),
            "expiry scan must be skipped before the watermark"
        );

        // Watermark reached: the orphan expires, the InputsBlocked entry
        // must stay.
        room.expiry_watermark = now;
        let filler2 = build_tx(vec![(&Byte32::zero(), 25)], 1);
        room.wait(
            filler2,
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: HashSet::from([Byte32::new([2u8; 32])]),
            },
        );
        assert!(
            room.get(&orphan_id).is_none(),
            "expired orphan must be evicted once the watermark is reached"
        );
        assert!(
            room.get(&conflict_id).is_some(),
            "InputsBlocked entries must never expire"
        );
    }

    /// Budget eviction must take the *oldest* entries first (FIFO), not an
    /// arbitrary HashMap order — and each eviction must stay O(1) (no
    /// full-table find per evicted entry).
    #[test]
    fn budget_eviction_is_oldest_first() {
        let mut room = WaitingRoom::new();
        let mut ids = Vec::new();
        for i in 0..4u8 {
            let tx = build_tx(vec![(&Byte32::zero(), 30 + i as u32)], 1);
            ids.push(tx.proposal_short_id());
            room.wait(
                tx,
                TxSource::Local,
                WaitReason::ParentsMissing {
                    parents: HashSet::from([Byte32::new([i; 32])]),
                },
            );
        }

        // Force one eviction at a time by driving the count budget down to
        // three via the internal loop directly: park one more with a tiny
        // budget simulated by manually shrinking the group limit is not
        // possible, so evict through `limit_size` by temporarily inflating
        // the group size instead.
        room.parents_missing.tx_size = MAX_PARENTS_MISSING_TX_SIZE + 1;
        let evicted = room.limit_size();
        assert_eq!(evicted.len(), 1);
        assert_eq!(
            evicted[0].tx.proposal_short_id(),
            ids[0],
            "the oldest entry must be evicted first"
        );
        assert!(room.get(&ids[0]).is_none());
        for id in &ids[1..] {
            assert!(room.get(id).is_some());
        }

        // The next eviction takes the second-oldest.
        room.parents_missing.tx_size = MAX_PARENTS_MISSING_TX_SIZE + 1;
        let evicted = room.limit_size();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].tx.proposal_short_id(), ids[1]);
    }

    /// A `RaceLost` re-park must keep the *original* expiry: a stalled
    /// winner must not hold the loser forever, and refreshing the expiry
    /// on every restore/re-hold cycle would defeat it.
    #[test]
    fn wait_resolved_repark_preserves_race_lost_expiry() {
        let mut room = WaitingRoom::new();
        let winner = ProposalShortId::from_tx_hash(&Byte32::new([8u8; 32]));
        let tx = build_tx(vec![(&Byte32::zero(), 33)], 1);
        let id = tx.proposal_short_id();

        room.wait_resolved(
            dummy_resolved(tx.clone()),
            WaitReason::RaceLost {
                winner: winner.clone(),
            },
        );
        let original_expiry = {
            let entry = room.get(&id).unwrap();
            entry.expires_at
        };

        // Simulate the passage of time and a restore/re-hold cycle.
        {
            let entry = room.by_id.get_mut(&id).unwrap();
            entry.expires_at = original_expiry - 1_000;
        }
        room.wait_resolved(dummy_resolved(tx), WaitReason::RaceLost { winner });

        let entry = room.get(&id).unwrap();
        assert_eq!(
            entry.expires_at,
            original_expiry - 1_000,
            "a RaceLost re-park must not refresh the expiry"
        );
    }

    /// `clear()` must reset `expiry_watermark` to `u64::MAX`: the next
    /// `wait()` after a clear must not trigger a stale expiry scan from
    /// the previous generation's watermark (bug #24).
    #[test]
    fn clear_resets_expiry_watermark() {
        let mut room = WaitingRoom::new();
        let tx = build_tx(vec![(&Byte32::zero(), 40)], 1);

        // Park an entry to push the watermark down from u64::MAX.
        room.wait(
            tx.clone(),
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: tx.unique_parents(),
            },
        );
        assert!(
            room.expiry_watermark < u64::MAX,
            "watermark must be pushed down by a parked entry"
        );

        room.clear();
        assert_eq!(
            room.expiry_watermark,
            u64::MAX,
            "clear must reset expiry_watermark to u64::MAX"
        );
        assert_eq!(room.len(), 0);

        // The next wait must not see a stale watermark: it should compute
        // a fresh expiry from the current time.
        let tx2 = build_tx(vec![(&Byte32::zero(), 41)], 1);
        room.wait(
            tx2,
            TxSource::Local,
            WaitReason::ParentsMissing {
                parents: Default::default(),
            },
        );
        assert!(
            room.expiry_watermark > 0,
            "watermark must be recomputed after clear"
        );
    }
}
