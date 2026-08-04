//! Pure block-template transaction packing over one authority read receipt.
//!
//! This module owns no transaction state. It builds a bounded, short-lived
//! score overlay after the authority guard has opened, then returns only the
//! block-bounded immutable payloads selected by that computation.

use super::{
    plan::AncestorAggregate,
    state::{AcceptedAtMillis, Arrival, CandidateMetrics, RawTxHash},
    template::{TemplateCandidate, TemplateReadError, TemplateSelectionReceipt},
};
use crate::component::{entry::TxEntry, sort_key::AncestorsScoreSortKey};
use ckb_types::core::{
    Capacity, Cycle, cell::ResolvedTransaction, tx_pool::get_transaction_weight,
};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeSet, HashMap, hash_map::Entry},
    sync::Arc,
};

const MAX_CONSECUTIVE_PACKING_FAILURES: usize = 4_000;
const DESCENDANTS_CACHE_MEMBER_BUDGET: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplatePackingLimits {
    serialized_bytes: usize,
    cycles: Cycle,
}

impl TemplatePackingLimits {
    pub(super) const fn new(serialized_bytes: usize, cycles: Cycle) -> Self {
        Self {
            serialized_bytes,
            cycles,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PackedTemplateTransaction {
    accepted_at: AcceptedAtMillis,
    metrics: CandidateMetrics,
    resolved: Arc<ResolvedTransaction>,
}

#[derive(Debug)]
pub(super) struct PackedTemplateTransactions {
    entries: Vec<PackedTemplateTransaction>,
}

impl PackedTemplateTransactions {
    /// Convert only the block-bounded selected payloads into the established
    /// assembler DTO. The exact accepted timestamp was captured with the same
    /// authority receipt, so conversion reconstructs no membership graph.
    pub(super) fn into_tx_entries(self) -> Vec<TxEntry> {
        self.entries
            .into_iter()
            .map(|entry| {
                TxEntry::new_with_timestamp(
                    entry.resolved,
                    entry.metrics.cost.cycles,
                    entry.metrics.fee,
                    entry.metrics.cost.serialized_bytes,
                    entry.accepted_at.0,
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackageAggregate {
    entries: usize,
    serialized_bytes: usize,
    cycles: Cycle,
    fee: Capacity,
}

impl PackageAggregate {
    fn from_ancestor(aggregate: AncestorAggregate) -> Self {
        Self {
            entries: aggregate.entries,
            serialized_bytes: aggregate.serialized_bytes,
            cycles: aggregate.cycles,
            fee: aggregate.fee,
        }
    }

    fn one(candidate: &TemplateCandidate) -> Self {
        Self {
            entries: 1,
            serialized_bytes: candidate.metrics().cost.serialized_bytes,
            cycles: candidate.metrics().cost.cycles,
            fee: candidate.metrics().fee,
        }
    }

    fn checked_add(self, incoming: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(incoming.entries)?,
            serialized_bytes: self
                .serialized_bytes
                .checked_add(incoming.serialized_bytes)?,
            cycles: self.cycles.checked_add(incoming.cycles)?,
            fee: self.fee.safe_add(incoming.fee).ok()?,
        })
    }

    fn checked_sub(self, removed: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(removed.entries)?,
            serialized_bytes: self
                .serialized_bytes
                .checked_sub(removed.serialized_bytes)?,
            cycles: self.cycles.checked_sub(removed.cycles)?,
            fee: self.fee.safe_sub(removed.fee).ok()?,
        })
    }

    fn fits(self, limits: TemplatePackingLimits) -> bool {
        self.serialized_bytes <= limits.serialized_bytes && self.cycles <= limits.cycles
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PackageOrderKey {
    score: AncestorsScoreSortKey,
    arrival: Arrival,
    hash: RawTxHash,
    index: usize,
}

impl PackageOrderKey {
    fn new(index: usize, candidate: &TemplateCandidate, aggregate: PackageAggregate) -> Self {
        Self {
            score: AncestorsScoreSortKey {
                fee: candidate.metrics().fee,
                weight: get_transaction_weight(
                    candidate.metrics().cost.serialized_bytes,
                    candidate.metrics().cost.cycles,
                ),
                ancestors_fee: aggregate.fee,
                ancestors_weight: get_transaction_weight(
                    aggregate.serialized_bytes,
                    aggregate.cycles,
                ),
            },
            arrival: candidate.order().arrival(),
            hash: candidate.hash().clone(),
            index,
        }
    }
}

impl Ord for PackageOrderKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.cmp(&self.hash))
            .then_with(|| other.index.cmp(&self.index))
    }
}

impl PartialOrd for PackageOrderKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CandidatePackingState {
    Ineligible,
    Queued,
    Examining,
    Failed,
    Selected,
}

struct DescendantsCache {
    cached: HashMap<usize, Vec<usize>>,
    cached_members: usize,
    marks: Vec<u64>,
    generation: u64,
    stack: Vec<usize>,
}

impl DescendantsCache {
    fn new(candidate_count: usize, eligible_count: usize) -> Result<Self, TemplateReadError> {
        let mut cached = HashMap::new();
        cached
            .try_reserve(eligible_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut marks = Vec::new();
        marks
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        marks.resize(candidate_count, 0);
        let mut stack = Vec::new();
        stack
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        Ok(Self {
            cached,
            cached_members: 0,
            marks,
            generation: 0,
            stack,
        })
    }

    fn descendants<'cache>(
        &'cache mut self,
        start: usize,
        children: &[Vec<usize>],
    ) -> Result<Cow<'cache, [usize]>, TemplateReadError> {
        if self.cached.contains_key(&start) {
            let cached = self
                .cached
                .get(&start)
                .ok_or(TemplateReadError::Projection)?;
            return Ok(Cow::Borrowed(cached));
        }

        self.generation = match self.generation.checked_add(1) {
            Some(generation) => generation,
            None => {
                self.marks.fill(0);
                1
            }
        };
        self.stack.clear();
        self.stack.extend(
            children
                .get(start)
                .ok_or(TemplateReadError::Projection)?
                .iter()
                .copied(),
        );
        let mut descendants = Vec::new();
        descendants
            .try_reserve(children.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        while let Some(index) = self.stack.pop() {
            let mark = self
                .marks
                .get_mut(index)
                .ok_or(TemplateReadError::Projection)?;
            if *mark == self.generation {
                continue;
            }
            *mark = self.generation;
            descendants.push(index);
            self.stack.extend(
                children
                    .get(index)
                    .ok_or(TemplateReadError::Projection)?
                    .iter()
                    .copied(),
            );
        }

        let projected = self
            .cached_members
            .checked_add(descendants.len())
            .ok_or(TemplateReadError::Arithmetic)?;
        if projected <= DESCENDANTS_CACHE_MEMBER_BUDGET {
            self.cached_members = projected;
            match self.cached.entry(start) {
                Entry::Vacant(slot) => Ok(Cow::Borrowed(slot.insert(descendants))),
                Entry::Occupied(_) => Err(TemplateReadError::Projection),
            }
        } else {
            Ok(Cow::Owned(descendants))
        }
    }
}

impl TemplateSelectionReceipt {
    pub(super) fn pack_transactions(
        &self,
        limits: TemplatePackingLimits,
    ) -> Result<PackedTemplateTransactions, TemplateReadError> {
        self.pack_transactions_with_failure_bound(limits, MAX_CONSECUTIVE_PACKING_FAILURES)
    }

    fn pack_transactions_with_failure_bound(
        &self,
        limits: TemplatePackingLimits,
        max_consecutive_failures: usize,
    ) -> Result<PackedTemplateTransactions, TemplateReadError> {
        let candidates = self.candidates();
        let by_hash = self.candidate_index()?;
        let eligible = self.causally_eligible_proposed(&by_hash)?;
        let candidate_count = candidates.len();

        let mut eligible_flags = Vec::new();
        eligible_flags
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        eligible_flags.resize(candidate_count, false);
        let mut causal_rank = Vec::new();
        causal_rank
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        causal_rank.resize(candidate_count, None);
        for (rank, index) in eligible.iter().copied().enumerate() {
            *eligible_flags
                .get_mut(index)
                .ok_or(TemplateReadError::Projection)? = true;
            *causal_rank
                .get_mut(index)
                .ok_or(TemplateReadError::Projection)? = Some(rank);
        }

        let mut children = Vec::new();
        children
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        children.extend((0..candidate_count).map(|_| Vec::new()));
        for child in eligible.iter().copied() {
            let candidate = candidates.get(child).ok_or(TemplateReadError::Projection)?;
            for parent in candidate.parents() {
                let parent = by_hash
                    .get(parent)
                    .copied()
                    .ok_or(TemplateReadError::Projection)?;
                if !eligible_flags
                    .get(parent)
                    .copied()
                    .ok_or(TemplateReadError::Projection)?
                {
                    return Err(TemplateReadError::Projection);
                }
                let parent_children = children
                    .get_mut(parent)
                    .ok_or(TemplateReadError::Projection)?;
                parent_children
                    .try_reserve(1)
                    .map_err(|_| TemplateReadError::Allocation)?;
                parent_children.push(child);
            }
        }
        for next in &mut children {
            next.sort_unstable();
            next.dedup();
        }

        let mut aggregates = Vec::new();
        aggregates
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        aggregates.resize(candidate_count, None);
        let mut states = Vec::new();
        states
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        states.resize(candidate_count, CandidatePackingState::Ineligible);
        let mut queue = BTreeSet::new();
        for index in eligible.iter().copied() {
            let candidate = candidates.get(index).ok_or(TemplateReadError::Projection)?;
            let aggregate = PackageAggregate::from_ancestor(candidate.ancestors());
            if aggregate
                .checked_sub(PackageAggregate::one(candidate))
                .is_none()
            {
                return Err(TemplateReadError::Projection);
            }
            let key = PackageOrderKey::new(index, candidate, aggregate);
            if &key.score != candidate.order().score() {
                return Err(TemplateReadError::Projection);
            }
            *aggregates
                .get_mut(index)
                .ok_or(TemplateReadError::Projection)? = Some(aggregate);
            if aggregate.fits(limits) {
                if !queue.insert(key) {
                    return Err(TemplateReadError::Projection);
                }
                *states.get_mut(index).ok_or(TemplateReadError::Projection)? =
                    CandidatePackingState::Queued;
            }
        }

        let mut selected = Vec::new();
        selected
            .try_reserve(eligible.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut selected_bytes = 0usize;
        let mut selected_cycles = 0u64;
        let mut consecutive_failures = 0usize;
        let mut descendants = DescendantsCache::new(candidate_count, eligible.len())?;
        let mut package_marks = Vec::new();
        package_marks
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        package_marks.resize(candidate_count, 0u64);
        let mut package_generation = 0u64;
        let mut package = Vec::new();
        package
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut stack = Vec::new();
        stack
            .try_reserve(candidate_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut adjustments = HashMap::<usize, PackageAggregate>::new();
        adjustments
            .try_reserve(eligible.len())
            .map_err(|_| TemplateReadError::Allocation)?;

        while let Some(key) = queue.pop_last() {
            let index = key.index;
            if states.get(index) != Some(&CandidatePackingState::Queued) {
                return Err(TemplateReadError::Projection);
            }
            let candidate = candidates.get(index).ok_or(TemplateReadError::Projection)?;
            let aggregate = aggregates
                .get(index)
                .copied()
                .flatten()
                .ok_or(TemplateReadError::Projection)?;
            if key != PackageOrderKey::new(index, candidate, aggregate) {
                return Err(TemplateReadError::Projection);
            }
            *states.get_mut(index).ok_or(TemplateReadError::Projection)? =
                CandidatePackingState::Examining;
            let projected_bytes = selected_bytes
                .checked_add(aggregate.serialized_bytes)
                .ok_or(TemplateReadError::Arithmetic)?;
            let projected_cycles = selected_cycles
                .checked_add(aggregate.cycles)
                .ok_or(TemplateReadError::Arithmetic)?;
            if projected_bytes > limits.serialized_bytes || projected_cycles > limits.cycles {
                *states.get_mut(index).ok_or(TemplateReadError::Projection)? =
                    CandidatePackingState::Failed;
                consecutive_failures = consecutive_failures
                    .checked_add(1)
                    .ok_or(TemplateReadError::Arithmetic)?;
                if consecutive_failures > max_consecutive_failures {
                    break;
                }
                continue;
            }

            package_generation = match package_generation.checked_add(1) {
                Some(generation) => generation,
                None => {
                    package_marks.fill(0);
                    1
                }
            };
            package.clear();
            stack.clear();
            stack.push(index);
            while let Some(member) = stack.pop() {
                match states.get(member).copied() {
                    Some(CandidatePackingState::Selected) => continue,
                    Some(
                        CandidatePackingState::Queued
                        | CandidatePackingState::Examining
                        | CandidatePackingState::Failed,
                    ) => {}
                    Some(CandidatePackingState::Ineligible) | None => {
                        return Err(TemplateReadError::Projection);
                    }
                }
                let mark = package_marks
                    .get_mut(member)
                    .ok_or(TemplateReadError::Projection)?;
                if *mark == package_generation {
                    continue;
                }
                *mark = package_generation;
                package.push(member);
                let member_candidate = candidates
                    .get(member)
                    .ok_or(TemplateReadError::Projection)?;
                for parent in member_candidate.parents() {
                    stack.push(
                        by_hash
                            .get(parent)
                            .copied()
                            .ok_or(TemplateReadError::Projection)?,
                    );
                }
            }
            for member in &package {
                if causal_rank.get(*member).copied().flatten().is_none() {
                    return Err(TemplateReadError::Projection);
                }
            }
            package.sort_unstable_by_key(|member| causal_rank.get(*member).copied().flatten());

            let package_aggregate = package.iter().try_fold(
                PackageAggregate {
                    entries: 0,
                    serialized_bytes: 0,
                    cycles: 0,
                    fee: Capacity::zero(),
                },
                |total, member| {
                    let candidate = candidates
                        .get(*member)
                        .ok_or(TemplateReadError::Projection)?;
                    total
                        .checked_add(PackageAggregate::one(candidate))
                        .ok_or(TemplateReadError::Arithmetic)
                },
            )?;
            if package_aggregate != aggregate {
                return Err(TemplateReadError::Projection);
            }

            adjustments.clear();
            for member in package.iter().copied() {
                if states.get(member) == Some(&CandidatePackingState::Queued) {
                    let member_candidate = candidates
                        .get(member)
                        .ok_or(TemplateReadError::Projection)?;
                    let member_aggregate = aggregates
                        .get(member)
                        .copied()
                        .flatten()
                        .ok_or(TemplateReadError::Projection)?;
                    if !queue.remove(&PackageOrderKey::new(
                        member,
                        member_candidate,
                        member_aggregate,
                    )) {
                        return Err(TemplateReadError::Projection);
                    }
                }
                *states
                    .get_mut(member)
                    .ok_or(TemplateReadError::Projection)? = CandidatePackingState::Selected;
                selected.push(member);

                let delta = PackageAggregate::one(
                    candidates
                        .get(member)
                        .ok_or(TemplateReadError::Projection)?,
                );
                for descendant in descendants.descendants(member, &children)?.iter().copied() {
                    if matches!(states.get(descendant), Some(CandidatePackingState::Queued)) {
                        match adjustments.entry(descendant) {
                            Entry::Occupied(mut slot) => {
                                let adjusted = slot
                                    .get()
                                    .checked_add(delta)
                                    .ok_or(TemplateReadError::Arithmetic)?;
                                slot.insert(adjusted);
                            }
                            Entry::Vacant(slot) => {
                                slot.insert(delta);
                            }
                        }
                    }
                }
            }

            for (descendant, delta) in adjustments.drain() {
                if !matches!(states.get(descendant), Some(CandidatePackingState::Queued)) {
                    continue;
                }
                let descendant_candidate = candidates
                    .get(descendant)
                    .ok_or(TemplateReadError::Projection)?;
                let previous = aggregates
                    .get(descendant)
                    .copied()
                    .flatten()
                    .ok_or(TemplateReadError::Projection)?;
                if states.get(descendant) == Some(&CandidatePackingState::Queued)
                    && !queue.remove(&PackageOrderKey::new(
                        descendant,
                        descendant_candidate,
                        previous,
                    ))
                {
                    return Err(TemplateReadError::Projection);
                }
                let remaining = previous
                    .checked_sub(delta)
                    .ok_or(TemplateReadError::Projection)?;
                *aggregates
                    .get_mut(descendant)
                    .ok_or(TemplateReadError::Projection)? = Some(remaining);
                if !queue.insert(PackageOrderKey::new(
                    descendant,
                    descendant_candidate,
                    remaining,
                )) {
                    return Err(TemplateReadError::Projection);
                }
                *states
                    .get_mut(descendant)
                    .ok_or(TemplateReadError::Projection)? = CandidatePackingState::Queued;
            }

            selected_bytes = projected_bytes;
            selected_cycles = projected_cycles;
            consecutive_failures = 0;
        }

        let ordered = self.order_packed_indices(selected, &by_hash)?;
        let mut entries = Vec::new();
        entries
            .try_reserve(ordered.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut final_bytes = 0usize;
        let mut final_cycles = 0u64;
        for index in ordered {
            let candidate = candidates.get(index).ok_or(TemplateReadError::Projection)?;
            final_bytes = final_bytes
                .checked_add(candidate.metrics().cost.serialized_bytes)
                .ok_or(TemplateReadError::Arithmetic)?;
            final_cycles = final_cycles
                .checked_add(candidate.metrics().cost.cycles)
                .ok_or(TemplateReadError::Arithmetic)?;
            entries.push(PackedTemplateTransaction {
                accepted_at: candidate.accepted_at(),
                metrics: candidate.metrics().clone(),
                resolved: Arc::clone(candidate.resolved()),
            });
        }
        if final_bytes > limits.serialized_bytes || final_cycles > limits.cycles {
            return Err(TemplateReadError::Projection);
        }
        Ok(PackedTemplateTransactions { entries })
    }
}

#[cfg(test)]
#[path = "tests/support/packing.rs"]
mod test_support;
