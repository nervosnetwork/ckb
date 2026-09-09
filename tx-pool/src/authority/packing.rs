//! Pure transaction selection and block packing over immutable accepted owners.
//!
//! This module owns no authoritative transaction state. Its bounded, short-lived
//! graph and score overlay serve template construction and fee estimation outside guards.

mod ordering;

use super::{
    membership::{self, Aggregate, Members},
    model::{self, Accepted, Error, Status},
};
use crate::component::{entry::TxEntry, sort_key::AncestorsScoreSortKey};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, FeeRate, tx_pool::get_transaction_weight},
    packed::{Byte32, ProposalShortId},
};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeSet, HashMap, hash_map::Entry},
    sync::Arc,
};

const MAX_CONDITIONAL_CYCLE_ROUNDS: usize = 64;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PackingError {
    Arithmetic,
    Projection,
    CausalCycle,
}
impl From<PackingError> for Error {
    fn from(error: PackingError) -> Self {
        match error {
            PackingError::Arithmetic => Error::Full("template arithmetic".into()),
            PackingError::Projection | PackingError::CausalCycle => Error::Fault("template graph"),
        }
    }
}
type EvictionRank = (Status, FeeRate, usize, u64, Byte32);
pub(super) struct Candidate {
    hash: Byte32,
    proposal: ProposalShortId,
    status: Status,
    arrival: u64,
    pub(super) accepted: Accepted,
    ancestors: Aggregate,
    score: AncestorsScoreSortKey,
    eviction: EvictionRank,
}
impl Candidate {
    fn hash(&self) -> &Byte32 {
        &self.hash
    }
    fn proposal_short_id(&self) -> &ProposalShortId {
        &self.proposal
    }
    fn parents(&self) -> &BTreeSet<Byte32> {
        &self.accepted.parents
    }
}
pub(super) struct Selection {
    candidates: Vec<Candidate>,
    dependency_edge_bound: usize,
}
impl Selection {
    pub(super) fn new(
        owners: Vec<Arc<model::Entry>>,
        snapshot: &Snapshot,
        max_ancestors: usize,
    ) -> Result<Self, Error> {
        let members: Members = owners
            .into_iter()
            .filter(|entry| entry.accepted().is_some())
            .map(|entry| (entry.hash(), entry))
            .collect();
        let totals = membership::aggregates(&members, max_ancestors)?;
        let mut candidates = Vec::with_capacity(members.len());
        let mut dependency_edge_bound = 0usize;
        for (hash, entry) in members {
            let accepted = entry.accepted().ok_or(Error::Stale)?.clone();
            let (ancestors, descendants) = *totals.get(&hash).ok_or(Error::Stale)?;
            let score = AncestorsScoreSortKey {
                fee: accepted.fee,
                weight: get_transaction_weight(accepted.size, accepted.cycles),
                ancestors_fee: Capacity::shannons(
                    u64::try_from(ancestors.fee).map_err(|_| Error::Full("template fee".into()))?,
                ),
                ancestors_weight: get_transaction_weight(ancestors.bytes, ancestors.cycles),
            };
            let status = accepted.status(snapshot);
            let eviction = (
                status,
                FeeRate::calculate(accepted.fee, score.weight).max(FeeRate::calculate(
                    descendants.fee(),
                    get_transaction_weight(descendants.bytes, descendants.cycles),
                )),
                descendants.count,
                entry.arrival,
                hash.clone(),
            );
            dependency_edge_bound = dependency_edge_bound
                .checked_add(accepted.dependencies().count())
                .ok_or(Error::Full("template edges".into()))?;
            candidates.push(Candidate {
                hash,
                proposal: entry.proposal(),
                status,
                arrival: entry.arrival,
                accepted,
                ancestors,
                score,
                eviction,
            });
        }
        candidates.sort_unstable_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.arrival.cmp(&b.arrival))
                .then_with(|| a.hash.cmp(&b.hash))
        });
        Ok(Self {
            candidates,
            dependency_edge_bound,
        })
    }
    fn eviction_ranks(&self) -> Vec<EvictionRank> {
        self.candidates
            .iter()
            .map(|candidate| candidate.eviction.clone())
            .collect()
    }
    pub(super) fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    pub(super) fn proposal_short_ids(
        &self,
        limit: u64,
    ) -> Result<Vec<ProposalShortId>, PackingError> {
        let mut ordered = Vec::with_capacity(self.candidates.len());
        ordered.extend(
            self.candidates
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    (candidate.status == Status::Pending).then_some(index)
                }),
        );
        let selected = match usize::try_from(limit) {
            Ok(limit) => limit.min(ordered.len()),
            Err(_) => ordered.len(),
        };
        let mut proposals = Vec::with_capacity(selected);
        for index in ordered.into_iter().take(selected) {
            proposals.push(
                self.candidates
                    .get(index)
                    .ok_or(PackingError::Projection)?
                    .proposal_short_id()
                    .clone(),
            );
        }
        Ok(proposals)
    }

    fn candidate_index(&self) -> Result<HashMap<Byte32, usize>, PackingError> {
        let mut by_hash = HashMap::with_capacity(self.candidates.len());
        for (index, candidate) in self.candidates.iter().enumerate() {
            if by_hash.insert(candidate.hash.clone(), index).is_some() {
                return Err(PackingError::Projection);
            }
        }
        Ok(by_hash)
    }

    /// Proposed candidates whose complete package ancestor closure is also
    /// Proposed, returned in deterministic parent-first order. Packing and the
    /// unbounded test projection share this one eligibility compiler.
    fn package_eligible_proposed(
        &self,
        by_hash: &HashMap<Byte32, usize>,
    ) -> Result<Vec<usize>, PackingError> {
        let mut eligible = Vec::with_capacity(self.candidates.len());
        eligible.resize(self.candidates.len(), false);
        let causal = package_indices(&self.candidates, by_hash)?;
        for index in &causal {
            let candidate = self
                .candidates
                .get(*index)
                .ok_or(PackingError::Projection)?;
            let mut parents_eligible = true;
            for parent in candidate.accepted.parents.iter() {
                let parent_index = by_hash
                    .get(parent)
                    .copied()
                    .ok_or(PackingError::Projection)?;
                if !eligible
                    .get(parent_index)
                    .copied()
                    .ok_or(PackingError::Projection)?
                {
                    parents_eligible = false;
                    break;
                }
            }
            *eligible.get_mut(*index).ok_or(PackingError::Projection)? =
                candidate.status == Status::Proposed && parents_eligible;
        }

        let mut selected = Vec::with_capacity(causal.len());
        for index in causal {
            if eligible
                .get(index)
                .copied()
                .ok_or(PackingError::Projection)?
            {
                selected.push(index);
            }
        }
        Ok(selected)
    }
}

#[expect(
    clippy::indexing_slicing,
    reason = "The private hash index and every child index are built from this exact candidate slice."
)]
fn package_indices(
    entries: &[Candidate],
    by_hash: &HashMap<Byte32, usize>,
) -> Result<Vec<usize>, PackingError> {
    // Candidates already have exact fee/arrival/hash preference order.
    let mut indegree = vec![0usize; entries.len()];
    let mut children = vec![Vec::new(); entries.len()];
    for (child, entry) in entries.iter().enumerate() {
        indegree[child] = entry.accepted.parents.len();
        for parent in &entry.accepted.parents {
            let parent = *by_hash.get(parent).ok_or(PackingError::Projection)?;
            children[parent].push(child);
        }
    }
    let mut ready: BTreeSet<_> = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect();
    let mut ordered = Vec::with_capacity(entries.len());
    while let Some(index) = ready.pop_first() {
        ordered.push(index);
        for child in &children[index] {
            indegree[*child] = indegree[*child]
                .checked_sub(1)
                .ok_or(PackingError::Projection)?;
            if indegree[*child] == 0 {
                ready.insert(*child);
            }
        }
    }
    if ordered.len() != entries.len() {
        return Err(PackingError::CausalCycle);
    }
    Ok(ordered)
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PackageAggregate {
    entries: usize,
    serialized_bytes: usize,
    cycles: Cycle,
    fee: Capacity,
}

impl PackageAggregate {
    fn from_ancestor(aggregate: Aggregate) -> Self {
        Self {
            entries: aggregate.count,
            serialized_bytes: aggregate.bytes,
            cycles: aggregate.cycles,
            fee: aggregate.fee(),
        }
    }

    fn one(candidate: &Candidate) -> Self {
        Self {
            entries: 1,
            serialized_bytes: candidate.accepted.size,
            cycles: candidate.accepted.cycles,
            fee: candidate.accepted.fee,
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
    arrival: u64,
    hash: Byte32,
    index: usize,
}

impl PackageOrderKey {
    fn new(index: usize, candidate: &Candidate, aggregate: PackageAggregate) -> Self {
        Self {
            score: AncestorsScoreSortKey {
                fee: candidate.accepted.fee,
                weight: get_transaction_weight(candidate.accepted.size, candidate.accepted.cycles),
                ancestors_fee: aggregate.fee,
                ancestors_weight: get_transaction_weight(
                    aggregate.serialized_bytes,
                    aggregate.cycles,
                ),
            },
            arrival: candidate.arrival,
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
    fn new(candidate_count: usize, eligible_count: usize) -> Self {
        let cached = HashMap::with_capacity(eligible_count);
        let marks = vec![0; candidate_count];
        let stack = Vec::with_capacity(candidate_count);
        Self {
            cached,
            cached_members: 0,
            marks,
            generation: 0,
            stack,
        }
    }

    fn descendants<'cache>(
        &'cache mut self,
        start: usize,
        children: &[Vec<usize>],
    ) -> Result<Cow<'cache, [usize]>, PackingError> {
        if self.cached.contains_key(&start) {
            let cached = self.cached.get(&start).ok_or(PackingError::Projection)?;
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
                .ok_or(PackingError::Projection)?
                .iter()
                .copied(),
        );
        let mut descendants = Vec::with_capacity(children.len());
        while let Some(index) = self.stack.pop() {
            let mark = self.marks.get_mut(index).ok_or(PackingError::Projection)?;
            if *mark == self.generation {
                continue;
            }
            *mark = self.generation;
            descendants.push(index);
            self.stack.extend(
                children
                    .get(index)
                    .ok_or(PackingError::Projection)?
                    .iter()
                    .copied(),
            );
        }

        let projected = self
            .cached_members
            .checked_add(descendants.len())
            .ok_or(PackingError::Arithmetic)?;
        if projected <= DESCENDANTS_CACHE_MEMBER_BUDGET {
            self.cached_members = projected;
            match self.cached.entry(start) {
                Entry::Vacant(slot) => Ok(Cow::Borrowed(slot.insert(descendants))),
                Entry::Occupied(_) => Err(PackingError::Projection),
            }
        } else {
            Ok(Cow::Owned(descendants))
        }
    }
}

impl Selection {
    pub(super) fn pack_transactions(
        &self,
        limits: TemplatePackingLimits,
    ) -> Result<Vec<TxEntry>, PackingError> {
        self.pack_transactions_with_failure_bound(limits, MAX_CONSECUTIVE_PACKING_FAILURES)
    }

    fn pack_transactions_with_failure_bound(
        &self,
        limits: TemplatePackingLimits,
        max_consecutive_failures: usize,
    ) -> Result<Vec<TxEntry>, PackingError> {
        let candidates = self.candidates();
        let by_hash = self.candidate_index()?;
        let eligible = self.package_eligible_proposed(&by_hash)?;
        let candidate_count = candidates.len();

        let mut causal_rank = Vec::with_capacity(candidate_count);
        causal_rank.resize(candidate_count, None);
        for (rank, index) in eligible.iter().copied().enumerate() {
            *causal_rank.get_mut(index).ok_or(PackingError::Projection)? = Some(rank);
        }

        let mut children = Vec::with_capacity(candidate_count);
        children.extend((0..candidate_count).map(|_| Vec::new()));
        for child in eligible.iter().copied() {
            let candidate = candidates.get(child).ok_or(PackingError::Projection)?;
            for parent in candidate.parents() {
                let parent = by_hash
                    .get(parent)
                    .copied()
                    .ok_or(PackingError::Projection)?;
                if causal_rank
                    .get(parent)
                    .ok_or(PackingError::Projection)?
                    .is_none()
                {
                    return Err(PackingError::Projection);
                }
                let parent_children = children.get_mut(parent).ok_or(PackingError::Projection)?;
                parent_children.reserve(1);
                parent_children.push(child);
            }
        }
        for next in &mut children {
            next.sort_unstable();
            next.dedup();
        }

        let mut aggregates = Vec::with_capacity(candidate_count);
        aggregates.resize(candidate_count, None);
        let mut states = Vec::with_capacity(candidate_count);
        states.resize(candidate_count, CandidatePackingState::Ineligible);
        let mut queue = BTreeSet::new();
        for index in eligible.iter().copied() {
            let candidate = candidates.get(index).ok_or(PackingError::Projection)?;
            let aggregate = PackageAggregate::from_ancestor(candidate.ancestors);
            if aggregate
                .checked_sub(PackageAggregate::one(candidate))
                .is_none()
            {
                return Err(PackingError::Projection);
            }
            let key = PackageOrderKey::new(index, candidate, aggregate);
            if key.score != candidate.score {
                return Err(PackingError::Projection);
            }
            *aggregates.get_mut(index).ok_or(PackingError::Projection)? = Some(aggregate);
            if aggregate.fits(limits) {
                if !queue.insert(key) {
                    return Err(PackingError::Projection);
                }
                *states.get_mut(index).ok_or(PackingError::Projection)? =
                    CandidatePackingState::Queued;
            }
        }

        let mut selected = Vec::with_capacity(eligible.len());
        let mut selected_bytes = 0usize;
        let mut selected_cycles = 0u64;
        let mut consecutive_failures = 0usize;
        let mut descendants = DescendantsCache::new(candidate_count, eligible.len());
        let mut package_marks = vec![0u64; candidate_count];
        let mut package_generation = 0u64;
        let mut package = Vec::with_capacity(candidate_count);
        let mut stack = Vec::with_capacity(candidate_count);
        let mut adjustments = HashMap::<usize, PackageAggregate>::with_capacity(eligible.len());

        while let Some(key) = queue.pop_last() {
            let index = key.index;
            if states.get(index) != Some(&CandidatePackingState::Queued) {
                return Err(PackingError::Projection);
            }
            let candidate = candidates.get(index).ok_or(PackingError::Projection)?;
            let aggregate = aggregates
                .get(index)
                .copied()
                .flatten()
                .ok_or(PackingError::Projection)?;
            if key != PackageOrderKey::new(index, candidate, aggregate) {
                return Err(PackingError::Projection);
            }
            *states.get_mut(index).ok_or(PackingError::Projection)? =
                CandidatePackingState::Examining;
            let projected_bytes = selected_bytes
                .checked_add(aggregate.serialized_bytes)
                .ok_or(PackingError::Arithmetic)?;
            let projected_cycles = selected_cycles
                .checked_add(aggregate.cycles)
                .ok_or(PackingError::Arithmetic)?;
            if projected_bytes > limits.serialized_bytes || projected_cycles > limits.cycles {
                *states.get_mut(index).ok_or(PackingError::Projection)? =
                    CandidatePackingState::Failed;
                consecutive_failures = consecutive_failures
                    .checked_add(1)
                    .ok_or(PackingError::Arithmetic)?;
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
                        return Err(PackingError::Projection);
                    }
                }
                let mark = package_marks
                    .get_mut(member)
                    .ok_or(PackingError::Projection)?;
                if *mark == package_generation {
                    continue;
                }
                *mark = package_generation;
                package.push(member);
                let member_candidate = candidates.get(member).ok_or(PackingError::Projection)?;
                for parent in member_candidate.parents() {
                    stack.push(
                        by_hash
                            .get(parent)
                            .copied()
                            .ok_or(PackingError::Projection)?,
                    );
                }
            }
            for member in &package {
                if causal_rank.get(*member).copied().flatten().is_none() {
                    return Err(PackingError::Projection);
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
                    let candidate = candidates.get(*member).ok_or(PackingError::Projection)?;
                    total
                        .checked_add(PackageAggregate::one(candidate))
                        .ok_or(PackingError::Arithmetic)
                },
            )?;
            if package_aggregate != aggregate {
                return Err(PackingError::Projection);
            }

            adjustments.clear();
            for member in package.iter().copied() {
                if states.get(member) == Some(&CandidatePackingState::Queued) {
                    let member_candidate =
                        candidates.get(member).ok_or(PackingError::Projection)?;
                    let member_aggregate = aggregates
                        .get(member)
                        .copied()
                        .flatten()
                        .ok_or(PackingError::Projection)?;
                    if !queue.remove(&PackageOrderKey::new(
                        member,
                        member_candidate,
                        member_aggregate,
                    )) {
                        return Err(PackingError::Projection);
                    }
                }
                *states.get_mut(member).ok_or(PackingError::Projection)? =
                    CandidatePackingState::Selected;
                selected.push(member);

                let delta =
                    PackageAggregate::one(candidates.get(member).ok_or(PackingError::Projection)?);
                for descendant in descendants.descendants(member, &children)?.iter().copied() {
                    if matches!(states.get(descendant), Some(CandidatePackingState::Queued)) {
                        match adjustments.entry(descendant) {
                            Entry::Occupied(mut slot) => {
                                let adjusted = slot
                                    .get()
                                    .checked_add(delta)
                                    .ok_or(PackingError::Arithmetic)?;
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
                let descendant_candidate =
                    candidates.get(descendant).ok_or(PackingError::Projection)?;
                let previous = aggregates
                    .get(descendant)
                    .copied()
                    .flatten()
                    .ok_or(PackingError::Projection)?;
                if !queue.remove(&PackageOrderKey::new(
                    descendant,
                    descendant_candidate,
                    previous,
                )) {
                    return Err(PackingError::Projection);
                }
                let remaining = previous
                    .checked_sub(delta)
                    .ok_or(PackingError::Projection)?;
                *aggregates
                    .get_mut(descendant)
                    .ok_or(PackingError::Projection)? = Some(remaining);
                if !queue.insert(PackageOrderKey::new(
                    descendant,
                    descendant_candidate,
                    remaining,
                )) {
                    return Err(PackingError::Projection);
                }
                *states.get_mut(descendant).ok_or(PackingError::Projection)? =
                    CandidatePackingState::Queued;
            }

            selected_bytes = projected_bytes;
            selected_cycles = projected_cycles;
            consecutive_failures = 0;
        }

        let ordered = self.order_packed_indices(selected, &by_hash)?;
        let mut entries = Vec::with_capacity(ordered.len());
        let mut final_bytes = 0usize;
        let mut final_cycles = 0u64;
        for index in ordered {
            let candidate = candidates.get(index).ok_or(PackingError::Projection)?;
            final_bytes = final_bytes
                .checked_add(candidate.accepted.size)
                .ok_or(PackingError::Arithmetic)?;
            final_cycles = final_cycles
                .checked_add(candidate.accepted.cycles)
                .ok_or(PackingError::Arithmetic)?;
            entries.push(candidate.accepted.projection());
        }
        if final_bytes > limits.serialized_bytes || final_cycles > limits.cycles {
            return Err(PackingError::Projection);
        }
        Ok(entries)
    }
}

#[cfg(test)]
#[path = "tests/packing.rs"]
mod tests;
