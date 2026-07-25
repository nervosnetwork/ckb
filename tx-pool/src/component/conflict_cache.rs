//! Bounded historical cache for verified transactions rejected because an
//! accepted pool transaction currently consumes one of their inputs.
//!
//! This is not an executable pipeline owner. Recovery admits the raw
//! transaction into the coordinator and then removes the cache entry while
//! holding the same `TxPool` write lock. Keeping the cache inside `TxPool`
//! makes input release, candidate discovery, and ownership transfer one
//! lock-domain transaction.

use crate::constants::{SHRINK_THRESHOLD, lazy_ticket_compaction_limit};
use crate::tx_source::TxSource;
use crate::util::compact_packed;
use ckb_types::core::TransactionView;
use ckb_types::packed::{Byte32, OutPoint};
use ckb_util::shrink_to_fit;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

const MAX_ENTRIES: usize = 10_000;
const MAX_RESIDENT_SIZE: usize = 50_000_000;
const ENTRY_RESIDENT_OVERHEAD: usize = 512;
const OUTPOINT_INDEX_RESIDENT_OVERHEAD: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct ConflictEntry {
    pub(crate) tx: TransactionView,
    pub(crate) source: TxSource,
    // Every input and cell dependency whose availability can make this
    // historical transaction executable again. Accepted RBF victims provide
    // expanded dep-group members as well. A compact Vec avoids a second
    // per-entry hash table while `by_outpoint` remains the reverse wake index.
    recovery_outpoints: Vec<OutPoint>,
    generation: u64,
    resident_charge: usize,
    /// Identity of the pool mutation that most recently recorded this entry.
    /// Discovery for the same mutation skips it without retaining a copied
    /// set of up to every RBF victim on each pending release event.
    release_event: Option<Arc<ConflictReleaseEvent>>,
}

/// Opaque identity shared by conflict records and freed-input discovery from
/// one pool mutation. Pointer identity is sufficient and cannot wrap or be
/// attacker-chosen.
#[derive(Debug)]
pub(crate) struct ConflictReleaseEvent(());

impl ConflictReleaseEvent {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self(()))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ConflictRecoveryCandidate {
    pub(crate) tx: TransactionView,
    pub(crate) source: TxSource,
    pub(crate) recovery_outpoints: Vec<OutPoint>,
}

#[derive(Debug)]
struct ConflictDiscoveryState {
    generation: u64,
    cursor: Option<Byte32>,
    /// RBF entries removed by the same mutation remain historical records,
    /// but must not be immediately re-admitted just because their own parent
    /// was removed.
    excluded_event: Option<Arc<ConflictReleaseEvent>>,
    /// A release that arrives while this outpoint is already being scanned
    /// must not reset `cursor`: repeated blocker churn could otherwise keep
    /// discovery pinned to the first candidate forever. Coalesce all such
    /// releases into one level-triggered follow-up pass, retaining the latest
    /// same-mutation exclusion set.
    rerun: bool,
    rerun_excluded_event: Option<Arc<ConflictReleaseEvent>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ConflictDiscoveryProgress {
    pub(crate) examined: usize,
    pub(crate) scheduled: usize,
    pub(crate) pending: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ConflictCache {
    // Historical ownership is keyed by the complete raw transaction hash.
    // ProposalShortId is a wire/proposal lookup key and must never collapse
    // two distinct cache residents into one identity.
    by_hash: HashMap<Byte32, ConflictEntry>,
    // Ordered hashes give bounded discovery a stable cursor. A HashSet here
    // forced callers to clone or scan the complete fan-out while holding the
    // authoritative TxPool write lock.
    by_outpoint: HashMap<OutPoint, BTreeSet<Byte32>>,
    insertion_order: VecDeque<(u64, Byte32)>,
    /// Level-triggered transfer work. The cache remains the sole owner until
    /// a candidate is synchronously admitted to the coordinator; queue
    /// tickets carry the cache generation so a stale ticket can never act on
    /// a removed-and-readmitted transaction with the same full hash.
    recovery_queue: VecDeque<(u64, Byte32)>,
    recovery_scheduled: HashMap<Byte32, u64>,
    /// Freed-input discovery work, separate from executable recovery. A
    /// generation prevents a stale ticket/cursor from acting on a later
    /// release event for the same outpoint.
    discovery_queue: VecDeque<(u64, OutPoint)>,
    discovery_pending: HashMap<OutPoint, ConflictDiscoveryState>,
    total_resident_size: usize,
    next_generation: u64,
    next_discovery_generation: u64,
    #[cfg(test)]
    max_entries_override: Option<usize>,
    #[cfg(test)]
    max_resident_size_override: Option<usize>,
}

impl ConflictCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn len(&self) -> usize {
        self.by_hash.len()
    }

    fn max_entries(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = self.max_entries_override {
            return limit;
        }
        MAX_ENTRIES
    }

    fn max_resident_size(&self) -> usize {
        #[cfg(test)]
        if let Some(limit) = self.max_resident_size_override {
            return limit;
        }
        MAX_RESIDENT_SIZE
    }

    pub(crate) fn insert(
        &mut self,
        tx: TransactionView,
        source: TxSource,
    ) -> (bool, Vec<ConflictEntry>) {
        let recovery_outpoints = raw_recovery_outpoints(&tx);
        self.insert_with_outpoints_for_release(tx, source, recovery_outpoints, None)
    }

    pub(crate) fn insert_for_release(
        &mut self,
        tx: TransactionView,
        source: TxSource,
        release_event: Option<Arc<ConflictReleaseEvent>>,
    ) -> (bool, Vec<ConflictEntry>) {
        let recovery_outpoints = raw_recovery_outpoints(&tx);
        self.insert_with_outpoints_for_release(tx, source, recovery_outpoints, release_event)
    }

    pub(crate) fn insert_with_outpoints_for_release(
        &mut self,
        tx: TransactionView,
        source: TxSource,
        recovery_outpoints: impl IntoIterator<Item = OutPoint>,
        release_event: Option<Arc<ConflictReleaseEvent>>,
    ) -> (bool, Vec<ConflictEntry>) {
        let hash = tx.hash();
        let recovery_outpoints = compact_unique_outpoints(recovery_outpoints);
        if let Some(existing) = self.by_hash.get(&hash) {
            // Source strength is monotonic across every lifecycle owner. A
            // Local/Proposal resubmission of a historical remote candidate
            // must not later recover with stale peer limits or expiry. Replace
            // the witness-bearing view at the same time; raw hash equality
            // guarantees that the indexed inputs are unchanged.
            let stronger_source = source.trust() > existing.source.trust();
            let trusted_witness_refresh = source.trust() == existing.source.trust()
                && !matches!(source, TxSource::Remote { .. })
                && existing.tx.witness_hash() != tx.witness_hash();
            let existing_outpoints: HashSet<_> =
                existing.recovery_outpoints.iter().cloned().collect();
            let added_outpoints = recovery_outpoints
                .into_iter()
                .filter(|out_point| !existing_outpoints.contains(out_point))
                .collect::<Vec<_>>();
            let metadata_extended = !added_outpoints.is_empty();
            if stronger_source || trusted_witness_refresh || metadata_extended {
                let replace_payload = stronger_source || trusted_witness_refresh;
                let old_charge = existing.resident_charge;
                let mut merged_outpoints = existing.recovery_outpoints.clone();
                merged_outpoints.extend(added_outpoints.iter().cloned());
                let new_charge = conflict_entry_resident_charge(
                    if replace_payload { &tx } else { &existing.tx },
                    merged_outpoints.len(),
                );
                if new_charge > self.max_resident_size() {
                    return (
                        false,
                        self.remove(&hash)
                            .into_iter()
                            .collect::<Vec<ConflictEntry>>(),
                    );
                }
                // A trusted replacement is a new authoritative arrival for
                // eviction purposes. Keeping the old FIFO generation lets an
                // attacker pre-seed a tiny remote witness at the oldest slot,
                // then make the later, larger proposal evict itself while
                // newer remote audit entries survive.
                let generation = self.allocate_generation();
                let was_scheduled = self.recovery_scheduled.remove(&hash).is_some();
                let existing = self
                    .by_hash
                    .get_mut(&hash)
                    .expect("the duplicate entry remains present");
                if replace_payload {
                    existing.tx = tx;
                    existing.source = source;
                }
                existing.generation = generation;
                existing.resident_charge = new_charge;
                existing.release_event = release_event;
                existing.recovery_outpoints = merged_outpoints;
                for out_point in added_outpoints {
                    self.by_outpoint
                        .entry(out_point)
                        .or_default()
                        .insert(hash.clone());
                }
                self.total_resident_size = self
                    .total_resident_size
                    .checked_sub(old_charge)
                    .and_then(|size| size.checked_add(new_charge))
                    .expect("conflict cache byte accounting is bounded and exact");
                self.insertion_order.push_back((generation, hash.clone()));
                if was_scheduled {
                    self.recovery_scheduled.insert(hash.clone(), generation);
                    self.recovery_queue.push_back((generation, hash));
                }
                let evicted = self.evict_over_budget();
                self.compact_tickets_if_needed();
                return (false, evicted);
            }
            self.by_hash
                .get_mut(&hash)
                .expect("the duplicate entry remains present")
                .release_event = release_event;
            return (false, Vec::new());
        }
        let inserted_hash = hash.clone();
        let generation = self.allocate_generation();
        // Both the entry-local wake points and the reverse-index keys can outlive
        // the transaction view that first introduced a shared outpoint.
        // Store compact molecule entities rather than slices into that tx.
        let resident_charge = conflict_entry_resident_charge(&tx, recovery_outpoints.len());
        if resident_charge > self.max_resident_size() {
            return (false, Vec::new());
        }
        for out_point in &recovery_outpoints {
            self.by_outpoint
                .entry(out_point.clone())
                .or_default()
                .insert(hash.clone());
        }
        self.total_resident_size = self
            .total_resident_size
            .checked_add(resident_charge)
            .expect("conflict cache resident charge cannot overflow its byte budget");
        self.insertion_order.push_back((generation, hash.clone()));
        self.by_hash.insert(
            hash,
            ConflictEntry {
                tx,
                source,
                recovery_outpoints,
                generation,
                resident_charge,
                release_event,
            },
        );

        let evicted = self.evict_over_budget();
        (self.by_hash.contains_key(&inserted_hash), evicted)
    }

    fn evict_over_budget(&mut self) -> Vec<ConflictEntry> {
        let mut evicted = Vec::new();
        while self.by_hash.len() > self.max_entries()
            || self.total_resident_size > self.max_resident_size()
        {
            let Some((generation, oldest)) = self.insertion_order.pop_front() else {
                break;
            };
            if self
                .by_hash
                .get(&oldest)
                .is_some_and(|entry| entry.generation == generation)
                && let Some(entry) = self.remove(&oldest)
            {
                evicted.push(entry);
            }
        }
        assert!(
            self.by_hash.len() <= self.max_entries()
                && self.total_resident_size <= self.max_resident_size(),
            "conflict-cache FIFO/index invariant could not enforce its resident budget"
        );
        evicted
    }

    pub(crate) fn remove(&mut self, hash: &Byte32) -> Option<ConflictEntry> {
        let entry = self.by_hash.remove(hash)?;
        self.recovery_scheduled.remove(&entry.tx.hash());
        self.total_resident_size = self
            .total_resident_size
            .checked_sub(entry.resident_charge)
            .expect("conflict cache byte accounting is exact");
        for out_point in &entry.recovery_outpoints {
            if let Some(ids) = self.by_outpoint.get_mut(out_point) {
                ids.remove(hash);
                if ids.is_empty() {
                    self.by_outpoint.remove(out_point);
                    self.discovery_pending.remove(out_point);
                }
            }
        }
        self.compact_tickets_if_needed();
        self.shrink_to_fit();
        Some(entry)
    }

    pub(crate) fn remove_hash(&mut self, hash: &Byte32) -> bool {
        self.remove(hash).is_some()
    }

    /// Register freed inputs for bounded recovery discovery without walking
    /// their candidate fan-out.
    pub(crate) fn schedule_discovery_by_inputs(
        &mut self,
        inputs: impl Iterator<Item = OutPoint>,
        excluded_event: Option<Arc<ConflictReleaseEvent>>,
    ) -> usize {
        let mut seen = HashSet::new();
        let mut scheduled = 0;
        for input in inputs {
            if !seen.insert(input.clone()) {
                continue;
            }
            let has_candidates = self
                .by_outpoint
                .get(&input)
                .is_some_and(|ids| !ids.is_empty());
            if !has_candidates {
                continue;
            }
            if let Some(state) = self.discovery_pending.get_mut(&input) {
                state.rerun = true;
                state.rerun_excluded_event = excluded_event.clone();
                continue;
            }
            // The released outpoint commonly comes from a removed pool
            // transaction. Discovery may remain queued after its callback
            // payload has published, so it must not keep that whole packed
            // transaction alive through a shared slice.
            let input = compact_packed(&input);
            let generation = self.allocate_discovery_generation();
            self.discovery_pending.insert(
                input.clone(),
                ConflictDiscoveryState {
                    generation,
                    cursor: None,
                    excluded_event: excluded_event.clone(),
                    rerun: false,
                    rerun_excluded_event: None,
                },
            );
            self.discovery_queue.push_back((generation, input));
            scheduled += 1;
        }
        self.compact_tickets_if_needed();
        scheduled
    }

    /// Probe at most `limit` cached candidates, rotating across freed
    /// outpoints. Eligible candidates become recovery tickets but stay owned
    /// by this cache until the later atomic coordinator handoff.
    pub(crate) fn discover_recoverable(
        &mut self,
        limit: usize,
        mut ready: impl FnMut(&TransactionView, &[OutPoint]) -> bool,
    ) -> ConflictDiscoveryProgress {
        let mut examined = 0;
        let mut scheduled = 0;
        while examined < limit {
            let Some((generation, input)) = self.discovery_queue.pop_front() else {
                break;
            };
            let Some(state) = self
                .discovery_pending
                .get(&input)
                .filter(|state| state.generation == generation)
            else {
                continue;
            };
            let next_hash = self.by_outpoint.get(&input).and_then(|hashes| {
                state.cursor.as_ref().map_or_else(
                    || hashes.iter().next().cloned(),
                    |cursor| hashes.range((Excluded(cursor), Unbounded)).next().cloned(),
                )
            });
            let Some(hash) = next_hash else {
                let rerun = self
                    .discovery_pending
                    .get(&input)
                    .is_some_and(|state| state.rerun);
                if rerun {
                    let state = self
                        .discovery_pending
                        .get_mut(&input)
                        .expect("discovery rerun retains its state");
                    state.cursor = None;
                    state.excluded_event = state.rerun_excluded_event.take();
                    state.rerun = false;
                    self.discovery_queue.push_back((generation, input));
                } else {
                    self.discovery_pending.remove(&input);
                }
                continue;
            };

            // Advance before checking eligibility: stale/excluded/blocked
            // entries still consume probe budget. Requeueing after one hash is
            // deliberate round-robin fairness for high-fan-out inputs.
            let state = self
                .discovery_pending
                .get_mut(&input)
                .expect("validated discovery state remains present");
            state.cursor = Some(hash.clone());
            let excluded_event = state.excluded_event.clone();
            self.discovery_queue.push_back((generation, input));
            examined += 1;

            let Some(entry) = self.by_hash.get(&hash) else {
                continue;
            };
            let excluded_by_same_release = excluded_event
                .as_ref()
                .zip(entry.release_event.as_ref())
                .is_some_and(|(release, recorded)| Arc::ptr_eq(release, recorded));
            if excluded_by_same_release || !ready(&entry.tx, &entry.recovery_outpoints) {
                continue;
            }
            let entry_generation = entry.generation;
            if !self.recovery_scheduled.contains_key(&hash) {
                self.recovery_scheduled
                    .insert(hash.clone(), entry_generation);
                self.recovery_queue.push_back((entry_generation, hash));
                scheduled += 1;
            }
        }
        self.compact_tickets_if_needed();
        ConflictDiscoveryProgress {
            examined,
            scheduled,
            pending: !self.discovery_pending.is_empty(),
        }
    }

    pub(crate) fn pop_recovery_candidate(&mut self) -> Option<ConflictRecoveryCandidate> {
        while let Some((generation, hash)) = self.recovery_queue.pop_front() {
            if self.recovery_scheduled.get(&hash) != Some(&generation) {
                continue;
            }
            let Some(entry) = self
                .by_hash
                .get(&hash)
                .filter(|entry| entry.generation == generation)
            else {
                self.recovery_scheduled.remove(&hash);
                continue;
            };
            self.recovery_scheduled.remove(&hash);
            return Some(ConflictRecoveryCandidate {
                tx: entry.tx.clone(),
                source: entry.source,
                recovery_outpoints: entry.recovery_outpoints.clone(),
            });
        }
        None
    }

    pub(crate) fn reschedule_recovery(&mut self, hash: &Byte32) -> bool {
        let Some(entry) = self.by_hash.get(hash) else {
            return false;
        };
        let generation = entry.generation;
        if self.recovery_scheduled.contains_key(hash) {
            return false;
        }
        self.recovery_scheduled.insert(hash.clone(), generation);
        self.recovery_queue.push_back((generation, hash.clone()));
        true
    }

    pub(crate) fn recovery_len(&self) -> usize {
        self.recovery_scheduled.len()
    }

    pub(crate) fn discovery_len(&self) -> usize {
        self.discovery_pending.len()
    }

    /// Cancel executable transfer work without deleting historical conflict
    /// records. Used by the pipeline epoch barrier: old-epoch maintenance must
    /// not resurrect a pre-pool owner after clear.
    pub(crate) fn clear_recovery_schedule(&mut self) {
        self.recovery_queue.clear();
        self.recovery_scheduled.clear();
        self.discovery_queue.clear();
        self.discovery_pending.clear();
    }

    pub(crate) fn entries(&self) -> impl Iterator<Item = &ConflictEntry> {
        self.by_hash.values()
    }

    pub(crate) fn clear(&mut self) {
        self.by_hash.clear();
        self.by_outpoint.clear();
        self.insertion_order.clear();
        self.clear_recovery_schedule();
        self.total_resident_size = 0;
        self.next_generation = 0;
        self.next_discovery_generation = 0;
    }

    fn allocate_generation(&mut self) -> u64 {
        if self.next_generation == u64::MAX {
            self.rebase_generations();
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    fn rebase_generations(&mut self) {
        let scheduled_order = self
            .recovery_queue
            .iter()
            .filter(|(generation, hash)| self.recovery_scheduled.get(hash) == Some(generation))
            .map(|(_, hash)| hash.clone())
            .collect::<Vec<_>>();
        let mut compact = VecDeque::with_capacity(self.by_hash.len());
        let mut generation = 0u64;
        while let Some((old_generation, hash)) = self.insertion_order.pop_front() {
            let Some(entry) = self.by_hash.get_mut(&hash) else {
                continue;
            };
            if entry.generation != old_generation {
                continue;
            }
            entry.generation = generation;
            compact.push_back((generation, hash));
            generation += 1;
        }
        self.insertion_order = compact;
        self.next_generation = generation;

        self.recovery_queue.clear();
        self.recovery_scheduled.clear();
        for hash in scheduled_order {
            let Some(entry) = self.by_hash.get(&hash) else {
                continue;
            };
            if self
                .recovery_scheduled
                .insert(hash.clone(), entry.generation)
                .is_none()
            {
                self.recovery_queue.push_back((entry.generation, hash));
            }
        }
    }

    fn allocate_discovery_generation(&mut self) -> u64 {
        if self.next_discovery_generation == u64::MAX {
            self.rebase_discovery_generations();
        }
        let generation = self.next_discovery_generation;
        self.next_discovery_generation += 1;
        generation
    }

    fn rebase_discovery_generations(&mut self) {
        let mut live = Vec::with_capacity(self.discovery_pending.len());
        let mut seen = HashSet::with_capacity(self.discovery_pending.len());
        while let Some((generation, input)) = self.discovery_queue.pop_front() {
            if self
                .discovery_pending
                .get(&input)
                .is_some_and(|state| state.generation == generation)
                && seen.insert(input.clone())
            {
                live.push(input);
            }
        }
        // Defensive completion: a live state should always own one ticket,
        // but preserve it if an earlier invariant violation dropped that
        // ticket rather than losing level-triggered recovery work.
        live.extend(
            self.discovery_pending
                .keys()
                .filter(|input| !seen.contains(*input))
                .cloned(),
        );
        for (generation, input) in live.into_iter().enumerate() {
            let generation = generation as u64;
            if let Some(state) = self.discovery_pending.get_mut(&input) {
                state.generation = generation;
                self.discovery_queue.push_back((generation, input));
            }
        }
        self.next_discovery_generation = self.discovery_pending.len() as u64;
    }

    /// Both FIFO lists use lazy stale tickets. Rebuild after enough churn so
    /// repeated remove/reinsert traffic cannot grow metadata without bound.
    fn compact_tickets_if_needed(&mut self) {
        let insertion_bound = lazy_ticket_compaction_limit(self.by_hash.len());
        if self.insertion_order.len() > insertion_bound {
            let mut live = self
                .by_hash
                .iter()
                .map(|(hash, entry)| (entry.generation, hash.clone()))
                .collect::<Vec<_>>();
            live.sort_unstable_by_key(|(generation, _)| *generation);
            self.insertion_order = live.into();
        }

        let recovery_bound = lazy_ticket_compaction_limit(self.recovery_scheduled.len());
        if self.recovery_queue.len() > recovery_bound {
            self.recovery_queue
                .retain(|(generation, hash)| self.recovery_scheduled.get(hash) == Some(generation));
        }

        let discovery_bound = lazy_ticket_compaction_limit(self.discovery_pending.len());
        if self.discovery_queue.len() > discovery_bound {
            self.discovery_queue.retain(|(generation, input)| {
                self.discovery_pending
                    .get(input)
                    .is_some_and(|state| state.generation == *generation)
            });
        }
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.by_hash, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_outpoint, SHRINK_THRESHOLD);
        shrink_to_fit!(self.recovery_scheduled, SHRINK_THRESHOLD);
        shrink_to_fit!(self.discovery_pending, SHRINK_THRESHOLD);
    }
}

fn conflict_entry_resident_charge(tx: &TransactionView, outpoint_count: usize) -> usize {
    tx.data()
        .serialized_size_in_block()
        .checked_add(ENTRY_RESIDENT_OVERHEAD)
        .and_then(|bytes| {
            outpoint_count
                .checked_mul(OUTPOINT_INDEX_RESIDENT_OVERHEAD)
                .and_then(|index_bytes| bytes.checked_add(index_bytes))
        })
        .unwrap_or(usize::MAX)
}

fn raw_recovery_outpoints(tx: &TransactionView) -> Vec<OutPoint> {
    compact_unique_outpoints(
        tx.input_pts_iter()
            .chain(tx.cell_deps().into_iter().map(|dep| dep.out_point())),
    )
}

fn compact_unique_outpoints(outpoints: impl IntoIterator<Item = OutPoint>) -> Vec<OutPoint> {
    let mut seen = HashSet::new();
    outpoints
        .into_iter()
        .filter_map(|out_point| {
            let out_point = compact_packed(&out_point);
            seen.insert(out_point.clone()).then_some(out_point)
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/conflict_cache.rs"]
mod tests;
