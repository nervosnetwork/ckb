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
const INPUT_INDEX_RESIDENT_OVERHEAD: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct ConflictEntry {
    pub(crate) tx: TransactionView,
    pub(crate) source: TxSource,
    // Inputs are already serialized in the bounded transaction payload and a
    // verified transaction cannot contain duplicates. A compact Vec avoids a
    // second per-entry hash table while `by_outpoint` remains the lookup index.
    inputs: Vec<OutPoint>,
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

    #[cfg(test)]
    pub(crate) fn set_limits_for_test(&mut self, max_entries: usize, max_resident_size: usize) {
        assert!(self.by_hash.len() <= max_entries);
        assert!(self.total_resident_size <= max_resident_size);
        self.max_entries_override = Some(max_entries);
        self.max_resident_size_override = Some(max_resident_size);
    }

    #[cfg(test)]
    pub(crate) fn contains_hash(&self, hash: &Byte32) -> bool {
        self.by_hash.contains_key(hash)
    }

    pub(crate) fn insert(
        &mut self,
        tx: TransactionView,
        source: TxSource,
    ) -> (bool, Vec<ConflictEntry>) {
        self.insert_for_release(tx, source, None)
    }

    pub(crate) fn insert_for_release(
        &mut self,
        tx: TransactionView,
        source: TxSource,
        release_event: Option<Arc<ConflictReleaseEvent>>,
    ) -> (bool, Vec<ConflictEntry>) {
        let hash = tx.hash();
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
            if stronger_source || trusted_witness_refresh {
                let old_charge = existing.resident_charge;
                let new_charge = conflict_entry_resident_charge(&tx, existing.inputs.len());
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
                existing.tx = tx;
                existing.source = source;
                existing.generation = generation;
                existing.resident_charge = new_charge;
                existing.release_event = release_event;
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
        // Both the entry-local inputs and the reverse-index keys can outlive
        // the transaction view that first introduced a shared outpoint.
        // Store compact molecule entities rather than slices into that tx.
        let inputs: Vec<_> = tx
            .input_pts_iter()
            .map(|input| compact_packed(&input))
            .collect();
        let resident_charge = conflict_entry_resident_charge(&tx, inputs.len());
        if resident_charge > self.max_resident_size() {
            return (false, Vec::new());
        }
        for input in &inputs {
            self.by_outpoint
                .entry(input.clone())
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
                inputs,
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
        for input in &entry.inputs {
            if let Some(ids) = self.by_outpoint.get_mut(input) {
                ids.remove(hash);
                if ids.is_empty() {
                    self.by_outpoint.remove(input);
                    self.discovery_pending.remove(input);
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

    #[cfg(test)]
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
                        && let Some(entry) = self.by_hash.get(id)
                        && all_inputs_free(&entry.tx)
                    {
                        result.push((entry.tx.clone(), entry.source));
                    }
                }
            }
        }
        result
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
            if !seen.insert(input.clone())
                || !self
                    .by_outpoint
                    .get(&input)
                    .is_some_and(|ids| !ids.is_empty())
            {
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
        mut all_inputs_free: impl FnMut(&TransactionView) -> bool,
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
            if excluded_by_same_release || !all_inputs_free(&entry.tx) {
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

    #[cfg(test)]
    pub(crate) fn schedule_recoverable_by_inputs(
        &mut self,
        inputs: impl Iterator<Item = OutPoint>,
        mut all_inputs_free: impl FnMut(&TransactionView) -> bool,
    ) -> usize {
        self.schedule_discovery_by_inputs(inputs, None);
        let mut scheduled = 0;
        while !self.discovery_pending.is_empty() {
            scheduled += self
                .discover_recoverable(usize::MAX, &mut all_inputs_free)
                .scheduled;
        }
        scheduled
    }

    #[cfg(test)]
    pub(crate) fn schedule_hashes(
        &mut self,
        hashes: impl Iterator<Item = Byte32>,
        mut eligible: impl FnMut(&TransactionView) -> bool,
    ) -> usize {
        let mut added = 0;
        for hash in hashes {
            let Some(entry) = self.by_hash.get(&hash).filter(|entry| eligible(&entry.tx)) else {
                continue;
            };
            let generation = entry.generation;
            if !self.recovery_scheduled.contains_key(&hash) {
                self.recovery_scheduled.insert(hash.clone(), generation);
                self.recovery_queue.push_back((generation, hash));
                added += 1;
            }
        }
        added
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
            .filter_map(|(generation, hash)| {
                (self.recovery_scheduled.get(hash) == Some(generation)).then(|| hash.clone())
            })
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

    #[cfg(test)]
    fn audit(&self) -> Result<(), &'static str> {
        if self.by_hash.len() > self.max_entries()
            || self.total_resident_size > self.max_resident_size()
        {
            return Err("conflict cache exceeds its resident budget");
        }
        let actual_size = self
            .by_hash
            .values()
            .try_fold(0usize, |total, entry| {
                total.checked_add(conflict_entry_resident_charge(
                    &entry.tx,
                    entry.inputs.len(),
                ))
            })
            .ok_or("conflict cache byte accounting overflow")?;
        if actual_size != self.total_resident_size
            || self.by_hash.values().any(|entry| {
                entry.resident_charge
                    != conflict_entry_resident_charge(&entry.tx, entry.inputs.len())
            })
        {
            return Err("conflict cache byte accounting mismatch");
        }
        for (hash, entry) in &self.by_hash {
            if entry.tx.hash() != *hash
                || entry.inputs.iter().any(|input| {
                    !self
                        .by_outpoint
                        .get(input)
                        .is_some_and(|hashes| hashes.contains(hash))
                })
            {
                return Err("conflict cache entry/index mismatch");
            }
            if self
                .insertion_order
                .iter()
                .filter(|(generation, queued)| *generation == entry.generation && queued == hash)
                .count()
                != 1
            {
                return Err("conflict cache live insertion ticket mismatch");
            }
        }
        for (input, hashes) in &self.by_outpoint {
            for hash in hashes {
                if !self
                    .by_hash
                    .get(hash)
                    .is_some_and(|entry| entry.inputs.contains(input))
                {
                    return Err("conflict cache reverse input index mismatch");
                }
            }
        }
        for (hash, generation) in &self.recovery_scheduled {
            if !self
                .by_hash
                .get(hash)
                .is_some_and(|entry| entry.generation == *generation)
                || self
                    .recovery_queue
                    .iter()
                    .filter(|(queued_generation, queued_hash)| {
                        queued_generation == generation && queued_hash == hash
                    })
                    .count()
                    != 1
            {
                return Err("conflict cache live recovery ticket mismatch");
            }
        }
        for (input, state) in &self.discovery_pending {
            if !self
                .by_outpoint
                .get(input)
                .is_some_and(|ids| !ids.is_empty())
                || self
                    .discovery_queue
                    .iter()
                    .filter(|(generation, queued)| {
                        *generation == state.generation && queued == input
                    })
                    .count()
                    != 1
            {
                return Err("conflict cache live discovery ticket mismatch");
            }
        }
        Ok(())
    }
}

fn conflict_entry_resident_charge(tx: &TransactionView, input_count: usize) -> usize {
    tx.data()
        .serialized_size_in_block()
        .checked_add(ENTRY_RESIDENT_OVERHEAD)
        .and_then(|bytes| {
            input_count
                .checked_mul(INPUT_INDEX_RESIDENT_OVERHEAD)
                .and_then(|input_bytes| bytes.checked_add(input_bytes))
        })
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        ConflictCache, ConflictReleaseEvent, INPUT_INDEX_RESIDENT_OVERHEAD, MAX_ENTRIES,
        SHRINK_THRESHOLD, conflict_entry_resident_charge,
    };
    use crate::tx_source::TxSource;
    use ckb_types::packed::{Byte32, CellInput, OutPoint};
    use ckb_types::{
        bytes::Bytes,
        core::{TransactionBuilder, TransactionView},
        packed,
        prelude::*,
    };
    use std::collections::HashSet;
    use std::sync::Arc;

    fn tx(seed: u8) -> TransactionView {
        TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(Byte32::new([seed; 32]), 0), 0))
            .build()
    }

    fn indexed_tx(seed: u32) -> TransactionView {
        let mut hash = [0u8; 32];
        hash[..4].copy_from_slice(&seed.to_le_bytes());
        TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(Byte32::new(hash), 0), 0))
            .build()
    }

    fn shared_input_tx(shared: &OutPoint, seed: u32) -> TransactionView {
        let mut hash = [0u8; 32];
        hash[..4].copy_from_slice(&seed.to_le_bytes());
        TransactionBuilder::default()
            .input(CellInput::new(shared.clone(), 0))
            .input(CellInput::new(OutPoint::new(Byte32::new(hash), 0), 0))
            .build()
    }

    fn with_cached_hash(tx: TransactionView, hash: Byte32) -> TransactionView {
        packed::TransactionView::new_builder()
            .data(tx.data())
            .hash(hash)
            .witness_hash(tx.witness_hash())
            .build()
            .unpack()
    }

    #[test]
    fn complete_hash_identity_retains_short_id_collisions() {
        let mut first_hash = [0x11; 32];
        let mut second_hash = first_hash;
        first_hash[31] = 1;
        second_hash[31] = 2;
        let first = with_cached_hash(tx(90), Byte32::new(first_hash));
        let second = with_cached_hash(tx(91), Byte32::new(second_hash));
        assert_eq!(first.proposal_short_id(), second.proposal_short_id());
        assert_ne!(first.hash(), second.hash());

        let mut cache = ConflictCache::new();
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert!(cache.insert(second.clone(), TxSource::Local).0);
        assert_eq!(cache.len(), 2);
        assert!(cache.contains_hash(&first.hash()));
        assert!(cache.contains_hash(&second.hash()));
        cache.audit().unwrap();
    }

    #[test]
    fn resident_charge_accounts_for_each_materialized_input_index() {
        let one = tx(1);
        let shared = one.input_pts_iter().next().unwrap();
        let two = shared_input_tx(&shared, 2);
        let payload_delta = two
            .data()
            .serialized_size_in_block()
            .checked_sub(one.data().serialized_size_in_block())
            .unwrap();
        assert_eq!(
            conflict_entry_resident_charge(&two, 2) - conflict_entry_resident_charge(&one, 1),
            payload_delta + INPUT_INDEX_RESIDENT_OVERHEAD
        );
    }

    #[test]
    fn trusted_duplicate_promotes_source_and_witness_without_merging_collisions() {
        let mut cache = ConflictCache::new();
        let remote = tx(42)
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
            .build();
        let proposal = remote
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
            .build();
        assert_eq!(remote.hash(), proposal.hash());
        assert_ne!(remote.witness_hash(), proposal.witness_hash());
        assert!(
            cache
                .insert(
                    remote,
                    TxSource::Remote {
                        cycles: 1,
                        peer: 9.into(),
                    },
                )
                .0
        );

        // Promotion updates the existing historical owner rather than adding
        // a second record. `inserted == false` still means no new cache slot.
        assert!(!cache.insert(proposal.clone(), TxSource::Proposal).0);
        let entry = cache.entries().next().unwrap();
        assert_eq!(entry.source, TxSource::Proposal);
        assert_eq!(entry.tx.witness_hash(), proposal.witness_hash());

        // Equal trusted strength may refresh a witness-bearing payload; the
        // first proposal must not pin a later authoritative variant forever.
        let refreshed = proposal
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from_static(b"refreshed-proposal").pack()])
            .build();
        assert!(!cache.insert(refreshed.clone(), TxSource::Proposal).0);
        let entry = cache.entries().next().unwrap();
        assert_eq!(entry.tx.witness_hash(), refreshed.witness_hash());

        // A weaker source cannot downgrade or replace the trusted witness.
        assert!(
            !cache
                .insert(
                    proposal
                        .as_advanced_builder()
                        .set_witnesses(vec![Bytes::from_static(b"later-remote").pack()])
                        .build(),
                    TxSource::Remote {
                        cycles: 2,
                        peer: 10.into(),
                    },
                )
                .0
        );
        let entry = cache.entries().next().unwrap();
        assert_eq!(entry.source, TxSource::Proposal);
        assert_eq!(entry.tx.witness_hash(), refreshed.witness_hash());
        cache.audit().unwrap();
    }

    #[test]
    fn trusted_promotion_reissues_recovery_and_fifo_generation() {
        let mut cache = ConflictCache::new();
        let remote = indexed_tx(0)
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from_static(b"remote").pack()])
            .build();
        let proposal = remote
            .as_advanced_builder()
            .set_witnesses(vec![Bytes::from_static(b"proposal").pack()])
            .build();
        let promoted_hash = proposal.hash();
        let input = proposal.input_pts_iter().next().unwrap();
        assert!(
            cache
                .insert(
                    remote,
                    TxSource::Remote {
                        cycles: 1,
                        peer: 9.into(),
                    },
                )
                .0
        );
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(input), |_| true),
            1
        );
        for seed in 1..MAX_ENTRIES as u32 {
            assert!(
                cache
                    .insert(
                        indexed_tx(seed),
                        TxSource::Remote {
                            cycles: 1,
                            peer: 10.into(),
                        },
                    )
                    .0
            );
        }

        let (_, promotion_evictions) = cache.insert(proposal.clone(), TxSource::Proposal);
        assert!(promotion_evictions.is_empty());
        let recovered = cache.pop_recovery_candidate().unwrap();
        assert_eq!(recovered.tx.witness_hash(), proposal.witness_hash());
        assert_eq!(recovered.source, TxSource::Proposal);

        let (_, evicted) = cache.insert(
            indexed_tx(MAX_ENTRIES as u32),
            TxSource::Remote {
                cycles: 1,
                peer: 10.into(),
            },
        );
        assert_eq!(evicted.len(), 1);
        assert_ne!(evicted[0].tx.hash(), promoted_hash);
        assert!(
            cache
                .entries()
                .any(|entry| entry.tx.hash() == promoted_hash),
            "the refreshed trusted owner must not inherit the attacker's stale FIFO age"
        );
        cache.audit().unwrap();
    }

    #[test]
    fn stale_eviction_ticket_cannot_remove_a_readmitted_hash() {
        let mut cache = ConflictCache::new();
        let first = tx(1);
        let hash = first.hash();
        assert!(cache.insert(first, TxSource::Local).0);
        assert!(cache.remove(&hash).is_some());
        assert!(cache.insert(tx(2), TxSource::Local).0);
        assert!(cache.insert(tx(1), TxSource::Local).0);

        let (stale_generation, stale_hash) = cache.insertion_order.pop_front().unwrap();
        assert_eq!(stale_hash, hash);
        assert_ne!(
            cache.by_hash.get(&stale_hash).unwrap().generation,
            stale_generation
        );
        assert!(cache.by_hash.contains_key(&hash));
        cache.audit().unwrap();
    }

    #[test]
    fn recovery_index_requires_every_input_to_be_free_and_unindexes_remove() {
        let mut cache = ConflictCache::new();
        let candidate = tx(3);
        let hash = candidate.hash();
        let input = candidate.input_pts_iter().next().unwrap();
        assert!(cache.insert(candidate.clone(), TxSource::Local).0);
        assert!(
            cache
                .recoverable_by_inputs(std::iter::once(input.clone()), |_| false)
                .is_empty()
        );
        assert_eq!(
            cache
                .recoverable_by_inputs(std::iter::once(input.clone()), |_| true)
                .len(),
            1
        );
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(input.clone()), |_| true),
            1
        );
        assert_eq!(cache.recovery_len(), 1);
        assert!(cache.remove(&hash).is_some());
        assert_eq!(cache.recovery_len(), 0);
        assert!(cache.pop_recovery_candidate().is_none());
        assert!(
            cache
                .recoverable_by_inputs(std::iter::once(input), |_| true)
                .is_empty()
        );
        cache.audit().unwrap();
    }

    #[test]
    fn stale_recovery_ticket_cannot_reorder_a_readmitted_hash() {
        let mut cache = ConflictCache::new();
        let first = tx(4);
        let first_hash = first.hash();
        let first_input = first.input_pts_iter().next().unwrap();
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(first_input.clone()), |_| true),
            1
        );
        assert!(cache.remove(&first_hash).is_some());

        let second = tx(5);
        let second_input = second.input_pts_iter().next().unwrap();
        assert!(cache.insert(second.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(second_input), |_| true),
            1
        );
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs(std::iter::once(first_input), |_| true),
            1
        );

        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            second.hash(),
            "the stale first-generation ticket must not jump the readmitted tx ahead"
        );
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            first.hash()
        );
        assert!(cache.pop_recovery_candidate().is_none());
        cache.audit().unwrap();
    }

    #[test]
    fn adversarial_remove_reinsert_churn_keeps_lazy_ticket_storage_bounded() {
        let mut cache = ConflictCache::new();
        let stable = tx(6);
        assert!(cache.insert(stable, TxSource::Local).0);

        let churned = tx(7);
        let churned_hash = churned.hash();
        let churned_input = churned.input_pts_iter().next().unwrap();
        for _ in 0..SHRINK_THRESHOLD.saturating_mul(4) {
            assert!(cache.insert(churned.clone(), TxSource::Local).0);
            assert_eq!(
                cache
                    .schedule_recoverable_by_inputs(std::iter::once(churned_input.clone()), |_| {
                        true
                    }),
                1
            );
            assert!(cache.remove(&churned_hash).is_some());
        }

        let bound = cache
            .by_hash
            .len()
            .saturating_mul(2)
            .saturating_add(SHRINK_THRESHOLD);
        assert!(cache.insertion_order.len() <= bound);
        assert!(cache.recovery_queue.len() <= SHRINK_THRESHOLD);
        assert!(cache.recovery_scheduled.is_empty());
        cache.audit().unwrap();
    }

    #[test]
    fn generation_rebase_preserves_scheduled_recovery_order() {
        let mut cache = ConflictCache::new();
        let first = tx(8);
        let second = tx(9);
        let first_input = first.input_pts_iter().next().unwrap();
        let second_input = second.input_pts_iter().next().unwrap();
        assert!(cache.insert(first.clone(), TxSource::Local).0);
        assert!(cache.insert(second.clone(), TxSource::Local).0);
        assert_eq!(
            cache.schedule_recoverable_by_inputs([first_input, second_input].into_iter(), |_| true),
            2
        );

        cache.next_generation = u64::MAX;
        assert!(cache.insert(tx(10), TxSource::Local).0);
        assert_eq!(cache.recovery_len(), 2);
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            first.hash()
        );
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            second.hash()
        );
        cache.audit().unwrap();
    }

    #[test]
    fn freed_input_discovery_is_probe_bounded_and_level_triggered() {
        let mut cache = ConflictCache::new();
        let shared = OutPoint::new(Byte32::new([0xf0; 32]), 0);
        let mut expected = HashSet::new();
        for seed in 0..10 {
            let candidate = shared_input_tx(&shared, seed);
            expected.insert(candidate.hash());
            assert!(cache.insert(candidate, TxSource::Local).0);
        }

        assert_eq!(
            cache.schedule_discovery_by_inputs(std::iter::once(shared), None),
            1
        );
        let first = cache.discover_recoverable(3, |_| true);
        assert_eq!(first.examined, 3);
        assert_eq!(first.scheduled, 3);
        assert!(first.pending);
        assert_eq!(cache.recovery_len(), 3);

        let second = cache.discover_recoverable(2, |_| true);
        assert_eq!(second.examined, 2);
        assert_eq!(second.scheduled, 2);
        assert!(second.pending);
        assert_eq!(cache.recovery_len(), 5);

        let final_slice = cache.discover_recoverable(100, |_| true);
        assert_eq!(final_slice.examined, 5);
        assert_eq!(final_slice.scheduled, 5);
        assert!(!final_slice.pending);

        let mut recovered = HashSet::new();
        while let Some(candidate) = cache.pop_recovery_candidate() {
            recovered.insert(candidate.tx.hash());
        }
        assert_eq!(recovered, expected);
        cache.audit().unwrap();
    }

    #[test]
    fn release_identity_excludes_only_records_from_the_same_pool_mutation() {
        let mut cache = ConflictCache::new();
        let shared = OutPoint::new(Byte32::new([0xf2; 32]), 0);
        let same_release = shared_input_tx(&shared, 1);
        let independent = shared_input_tx(&shared, 2);
        let release = ConflictReleaseEvent::new();
        assert!(
            cache
                .insert_for_release(
                    same_release.clone(),
                    TxSource::Local,
                    Some(Arc::clone(&release)),
                )
                .0
        );
        assert!(cache.insert(independent.clone(), TxSource::Local).0);

        assert_eq!(
            cache.schedule_discovery_by_inputs(std::iter::once(shared.clone()), Some(release),),
            1
        );
        let progress = cache.discover_recoverable(usize::MAX, |_| true);
        assert_eq!(progress.scheduled, 1);
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            independent.hash()
        );
        assert!(cache.pop_recovery_candidate().is_none());

        cache.remove(&independent.hash());
        assert_eq!(
            cache.schedule_discovery_by_inputs(std::iter::once(shared), None),
            1
        );
        cache.discover_recoverable(usize::MAX, |_| true);
        assert_eq!(
            cache.pop_recovery_candidate().unwrap().tx.hash(),
            same_release.hash()
        );
        cache.audit().unwrap();
    }

    #[test]
    fn repeated_hot_input_release_does_not_restart_discovery_cursor() {
        let mut cache = ConflictCache::new();
        let shared = OutPoint::new(Byte32::new([0xf1; 32]), 0);
        let mut expected = HashSet::new();
        for seed in 0..32 {
            let candidate = shared_input_tx(&shared, seed);
            expected.insert(candidate.hash());
            assert!(cache.insert(candidate, TxSource::Local).0);
        }

        assert_eq!(
            cache.schedule_discovery_by_inputs(std::iter::once(shared.clone()), None),
            1
        );
        for _ in 0..32 {
            let progress = cache.discover_recoverable(1, |_| true);
            assert_eq!(progress.examined, 1);
            // Simulate an accepted blocker repeatedly occupying and freeing
            // the same hot input before the current pass has completed.
            assert_eq!(
                cache.schedule_discovery_by_inputs(std::iter::once(shared.clone()), None),
                0,
                "an already-live outpoint owns exactly one discovery ticket"
            );
        }

        assert_eq!(
            cache.recovery_len(),
            expected.len(),
            "repeated release must not starve ids after the cursor head"
        );
        let mut recovered = HashSet::new();
        while let Some(candidate) = cache.pop_recovery_candidate() {
            recovered.insert(candidate.tx.hash());
        }
        assert_eq!(recovered, expected);
        cache.audit().unwrap();
    }

    #[test]
    fn cancelled_discovery_churn_keeps_lazy_ticket_storage_bounded() {
        let mut cache = ConflictCache::new();
        let candidate = tx(77);
        let hash = candidate.hash();
        let input = candidate.input_pts_iter().next().unwrap();
        for _ in 0..SHRINK_THRESHOLD.saturating_mul(4) {
            assert!(cache.insert(candidate.clone(), TxSource::Local).0);
            assert_eq!(
                cache.schedule_discovery_by_inputs(std::iter::once(input.clone()), None),
                1
            );
            assert!(cache.remove(&hash).is_some());
        }
        assert!(cache.discovery_pending.is_empty());
        assert!(cache.discovery_queue.len() <= SHRINK_THRESHOLD);
        cache.audit().unwrap();
    }
}
