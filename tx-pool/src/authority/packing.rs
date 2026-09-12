//! Selection borrows immutable accepted owners and uses one compiled causal graph.
//! Only this selection's package priorities and remaining budgets change.

mod graph;
mod ordering;

use super::{
    membership::{Aggregate, EvictionRank},
    model::{self, Accepted, Error, Status},
};
use crate::component::{entry::TxEntry, sort_key::AncestorsScoreSortKey};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, tx_pool::get_transaction_weight},
    packed::{Byte32, ProposalShortId, ProposalShortIdReader},
    prelude::Reader,
};
use graph::{Graph, Links};
use std::{
    borrow::Cow,
    cmp::Ordering,
    collections::{BTreeSet, BinaryHeap},
    sync::{Arc, Weak},
};

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

pub(super) struct Candidate<'a> {
    hash: &'a Byte32,
    proposal: ProposalShortIdReader<'a>,
    status: Status,
    arrival: u64,
    owner: &'a Arc<model::Entry>,
    pub(super) accepted: &'a Accepted,
}
impl<'a> Candidate<'a> {
    fn capture(owners: &'a [Arc<model::Entry>], snapshot: &Snapshot) -> Vec<Self> {
        let status = snapshot.proposals().status_lookup(owners.len());
        owners
            .iter()
            .filter_map(|owner| {
                owner.accepted().map(|accepted| {
                    let proposal = owner.transaction.proposal_short_id_reader();
                    let phase = status(proposal);
                    #[cfg(any(test, feature = "internal"))]
                    let phase = accepted.forced_status.unwrap_or(phase);
                    Candidate {
                        hash: owner.transaction.hash_ref(),
                        proposal,
                        status: phase,
                        arrival: owner.arrival,
                        owner,
                        accepted,
                    }
                })
            })
            .collect()
    }

    pub(super) fn hash(&self) -> &Byte32 {
        self.hash
    }
    pub(super) fn proposal_short_id(&self) -> ProposalShortIdReader<'a> {
        self.proposal
    }
    pub(super) fn owner(&self) -> &Arc<model::Entry> {
        self.owner
    }
}

/// Optional reuse by the sole template loop. Weak source identities retain no
/// retired transaction payload; proposal phase is recomputed for every capture.
#[derive(Default)]
pub(super) struct Cache {
    graph: Option<CachedGraph>,
}

struct CachedGraph {
    graph: Arc<Graph>,
    sources: Vec<Weak<model::Entry>>,
    max_ancestors: usize,
}
impl CachedGraph {
    fn matches(&self, candidates: &[Candidate<'_>], max_ancestors: usize) -> bool {
        self.max_ancestors == max_ancestors
            && self.sources.len() == candidates.len()
            && self
                .sources
                .iter()
                .zip(candidates)
                .all(|(source, candidate)| source.as_ptr() == Arc::as_ptr(candidate.owner))
    }
}
impl Cache {
    pub(super) fn selection<'a>(
        &mut self,
        owners: &'a [Arc<model::Entry>],
        snapshot: &Snapshot,
        max_ancestors: usize,
    ) -> Result<Selection<'a>, Error> {
        let candidates = Candidate::capture(owners, snapshot);
        let graph = match &self.graph {
            Some(cached) if cached.matches(&candidates, max_ancestors) => Arc::clone(&cached.graph),
            _ => {
                // Release obsolete derivations before building their replacement.
                self.graph = None;
                let graph = Arc::new(Graph::new(&candidates, max_ancestors)?);
                self.graph = Some(CachedGraph {
                    graph: Arc::clone(&graph),
                    sources: candidates
                        .iter()
                        .map(|candidate| Arc::downgrade(candidate.owner))
                        .collect(),
                    max_ancestors,
                });
                graph
            }
        };
        Ok(Selection { candidates, graph })
    }
}

pub(super) struct Selection<'a> {
    candidates: Vec<Candidate<'a>>,
    graph: Arc<Graph>,
}
impl<'a> Selection<'a> {
    pub(super) fn new(
        owners: &'a [Arc<model::Entry>],
        snapshot: &Snapshot,
        max_ancestors: usize,
    ) -> Result<Self, Error> {
        let candidates = Candidate::capture(owners, snapshot);
        let graph = Arc::new(Graph::new(&candidates, max_ancestors)?);
        Ok(Self { candidates, graph })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Candidates, descendant totals and traversal marks share the compiled graph's checked indices."
    )]
    fn eviction_ranks(&self) -> Result<Vec<EvictionRank>, PackingError> {
        // Descendant aggregates are needed only to resolve a conditional cycle.
        // Add each entry to its bounded ancestor closure; do not enumerate
        // potentially unbounded descendant sets or rebuild the causal graph.
        let mut totals = vec![Aggregate::default(); self.candidates.len()];
        let mut traversal = Traversal::new(self.candidates.len());
        for (index, candidate) in self.candidates.iter().enumerate() {
            let own = Aggregate::one(candidate.accepted);
            traversal.begin()?;
            traversal.stack.push(index);
            while let Some(ancestor) = traversal.stack.pop() {
                if traversal.marks[ancestor] == traversal.generation {
                    continue;
                }
                traversal.marks[ancestor] = traversal.generation;
                totals[ancestor] = totals[ancestor]
                    .add(own)
                    .map_err(|_| PackingError::Arithmetic)?;
                traversal.stack.extend(
                    self.graph
                        .parents
                        .get(ancestor)
                        .ok_or(PackingError::Projection)?,
                );
            }
        }
        Ok(self
            .candidates
            .iter()
            .zip(totals)
            .map(|(candidate, descendants)| {
                EvictionRank::for_accepted(
                    candidate.accepted,
                    candidate.status,
                    descendants,
                    candidate.arrival,
                    candidate.hash.clone(),
                )
            })
            .collect())
    }

    pub(super) fn candidates(&self) -> &[Candidate<'a>] {
        &self.candidates
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Candidates and aggregates have the same checked graph indices."
    )]
    fn priority(&self, status: Option<Status>) -> BinaryHeap<PackageOrderKey<'_>> {
        BinaryHeap::from(
            self.candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| status.is_none_or(|status| candidate.status == status))
                .map(|(index, candidate)| {
                    PackageOrderKey::new(index, candidate, self.graph.ancestors[index])
                })
                .collect::<Vec<_>>(),
        )
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The priority heap contains only indices of this candidate slice."
    )]
    pub(super) fn candidates_by_score(&self) -> impl Iterator<Item = &Candidate<'a>> {
        let mut priority = self.priority(None);
        std::iter::from_fn(move || priority.pop().map(|key| &self.candidates[key.index]))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The priority heap contains only indices of this candidate slice."
    )]
    pub(super) fn proposal_short_ids(&self, limit: u64) -> Vec<ProposalShortId> {
        let mut priority = self.priority(Some(Status::Pending));
        std::iter::from_fn(|| priority.pop())
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(|key| self.candidates[key.index].proposal.to_entity())
            .collect()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The compiled graph validates every endpoint and visits parents before children."
    )]
    fn package_eligible_proposed(&self) -> Result<Vec<usize>, PackingError> {
        let mut eligible = vec![false; self.candidates.len()];
        let mut selected = Vec::with_capacity(self.candidates.len());
        for &index in &self.graph.topological {
            eligible[index] = self.candidates[index].status == Status::Proposed
                && self
                    .graph
                    .parents
                    .get(index)
                    .ok_or(PackingError::Projection)?
                    .iter()
                    .all(|parent| eligible[*parent]);
            if eligible[index] {
                selected.push(index);
            }
        }
        Ok(selected)
    }

    #[cfg(test)]
    fn candidate_index(&self) -> Result<std::collections::HashMap<Byte32, usize>, PackingError> {
        Ok(self
            .candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.hash.clone(), index))
            .collect())
    }
}

const MAX_CONSECUTIVE_PACKING_FAILURES: usize = 4_000;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PackageAggregate {
    entries: usize,
    serialized_bytes: usize,
    cycles: Cycle,
    fee: Capacity,
}
impl PackageAggregate {
    fn one(candidate: &Candidate<'_>) -> Self {
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
struct PackageOrderKey<'a> {
    score: AncestorsScoreSortKey,
    arrival: u64,
    hash: &'a Byte32,
    index: usize,
}
impl<'a> PackageOrderKey<'a> {
    fn new(index: usize, candidate: &'a Candidate<'_>, aggregate: PackageAggregate) -> Self {
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
            hash: candidate.hash(),
            index,
        }
    }
}
impl Ord for PackageOrderKey<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.hash.cmp(self.hash))
            .then_with(|| other.index.cmp(&self.index))
    }
}
impl PartialOrd for PackageOrderKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CandidatePackingState {
    Ineligible,
    Original,
    Modified,
    Examining,
    Failed,
    Selected,
}
impl CandidatePackingState {
    fn queued(self) -> bool {
        matches!(self, Self::Original | Self::Modified)
    }
    fn needed(self) -> bool {
        self.queued() || self == Self::Examining
    }
}

/// Reusable ordinal marks and DFS storage for one selection.
struct Traversal {
    marks: Vec<usize>,
    generation: usize,
    stack: Vec<usize>,
}
impl Traversal {
    fn new(len: usize) -> Self {
        Self {
            marks: vec![0; len],
            generation: 0,
            stack: Vec::new(),
        }
    }
    fn begin(&mut self) -> Result<(), PackingError> {
        self.stack.clear();
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(PackingError::Arithmetic)?;
        Ok(())
    }
}

impl Selection<'_> {
    pub(super) fn pack_transactions(
        &self,
        limits: TemplatePackingLimits,
    ) -> Result<Vec<TxEntry>, PackingError> {
        self.pack_transactions_with_failure_bound(limits, MAX_CONSECUTIVE_PACKING_FAILURES)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The compiled graph defines every candidate, edge and overlay index; all queues are populated from those indices."
    )]
    fn pack_transactions_with_failure_bound(
        &self,
        limits: TemplatePackingLimits,
        max_consecutive_failures: usize,
    ) -> Result<Vec<TxEntry>, PackingError> {
        let len = self.candidates.len();
        let eligible = self.package_eligible_proposed()?;
        if eligible.is_empty() {
            return Ok(Vec::new());
        }
        let mut aggregates = Cow::Borrowed(self.graph.ancestors.as_slice());
        let mut states = vec![CandidatePackingState::Ineligible; len];
        let mut original = Vec::with_capacity(eligible.len());
        let mut minimum_bytes = usize::MAX;
        for &index in &eligible {
            if aggregates[index].fits(limits) {
                minimum_bytes = minimum_bytes.min(self.candidates[index].accepted.size);
                states[index] = CandidatePackingState::Original;
                original.push(PackageOrderKey::new(
                    index,
                    &self.candidates[index],
                    aggregates[index],
                ));
            }
        }
        if original.is_empty() {
            return Ok(Vec::new());
        }
        let mut original = BinaryHeap::from(original);
        let mut modified = BTreeSet::<PackageOrderKey<'_>>::new();
        let mut live_children = vec![0usize; len];
        for &index in self.graph.topological.iter().rev() {
            if states[index].needed() || live_children[index] != 0 {
                for &parent in self
                    .graph
                    .parents
                    .get(index)
                    .ok_or(PackingError::Projection)?
                {
                    live_children[parent] = live_children[parent]
                        .checked_add(1)
                        .ok_or(PackingError::Arithmetic)?;
                }
            }
        }

        let mut selected = Vec::with_capacity(eligible.len());
        let mut selected_bytes = 0usize;
        let mut selected_cycles = 0u64;
        let mut consecutive_failures = 0usize;
        let mut traversal = Traversal::new(len);
        let mut positions = vec![0usize; len];
        let mut package = Vec::new();
        let mut adjustments = vec![PackageAggregate::default(); len];
        let mut changed = Vec::new();

        loop {
            while original
                .peek()
                .is_some_and(|key| states[key.index] != CandidatePackingState::Original)
            {
                original.pop();
            }
            let key = match (original.peek(), modified.last()) {
                (None, None) => break,
                (Some(initial), Some(updated)) if updated > initial => modified.pop_last(),
                (None, Some(_)) => modified.pop_last(),
                _ => original.pop(),
            }
            .ok_or(PackingError::Projection)?;
            let index = key.index;
            let aggregate = aggregates[index];
            if !states[index].queued()
                || key != PackageOrderKey::new(index, &self.candidates[index], aggregate)
            {
                return Err(PackingError::Projection);
            }
            states[index] = CandidatePackingState::Examining;
            let projected_bytes = selected_bytes
                .checked_add(aggregate.serialized_bytes)
                .ok_or(PackingError::Arithmetic)?;
            let projected_cycles = selected_cycles
                .checked_add(aggregate.cycles)
                .ok_or(PackingError::Arithmetic)?;
            if projected_bytes > limits.serialized_bytes || projected_cycles > limits.cycles {
                states[index] = CandidatePackingState::Failed;
                retire_candidate(
                    index,
                    &states,
                    &mut live_children,
                    &self.graph.parents,
                    &mut traversal.stack,
                )?;
                consecutive_failures = consecutive_failures
                    .checked_add(1)
                    .ok_or(PackingError::Arithmetic)?;
                if consecutive_failures > max_consecutive_failures {
                    break;
                }
                continue;
            }

            package.clear();
            if aggregate.entries == 1 {
                if !self
                    .graph
                    .parents
                    .get(index)
                    .ok_or(PackingError::Projection)?
                    .iter()
                    .all(|parent| states[*parent] == CandidatePackingState::Selected)
                {
                    return Err(PackingError::Projection);
                }
                package.push(index);
            } else if !self.chain_package(index, &states, &mut package)? {
                self.ordered_package(index, &states, &mut traversal, &mut positions, &mut package)?;
            }
            let actual = package
                .iter()
                .try_fold(PackageAggregate::default(), |sum, member| {
                    sum.checked_add(PackageAggregate::one(&self.candidates[*member]))
                        .ok_or(PackingError::Arithmetic)
                })?;
            if actual != aggregate {
                return Err(PackingError::Projection);
            }

            // Complete the package before finding remaining consumers. Reverse
            // order lets each candidate retire its own queue use exactly once.
            for &member in package.iter().rev() {
                let previous = states[member];
                if previous == CandidatePackingState::Modified
                    && !modified.remove(&PackageOrderKey::new(
                        member,
                        &self.candidates[member],
                        aggregates[member],
                    ))
                {
                    return Err(PackingError::Projection);
                }
                states[member] = CandidatePackingState::Selected;
                if previous.needed() {
                    retire_candidate(
                        member,
                        &states,
                        &mut live_children,
                        &self.graph.parents,
                        &mut traversal.stack,
                    )?;
                }
            }
            selected.extend_from_slice(&package);
            selected_bytes = projected_bytes;
            selected_cycles = projected_cycles;
            consecutive_failures = 0;
            // Every remaining package contains a queued candidate's own bytes,
            // even after selected ancestors are subtracted. Keep checked-add
            // errors observable when synthetic limits approach integer bounds.
            if limits.serialized_bytes.saturating_sub(selected_bytes) < minimum_bytes
                && selected_bytes
                    .checked_add(limits.serialized_bytes)
                    .is_some()
                && selected_cycles.checked_add(limits.cycles).is_some()
            {
                break;
            }

            for &member in &package {
                let delta = PackageAggregate::one(&self.candidates[member]);
                traversal.begin()?;
                traversal.stack.extend(
                    self.graph
                        .children
                        .get(member)
                        .ok_or(PackingError::Projection)?,
                );
                while let Some(descendant) = traversal.stack.pop() {
                    if (!states[descendant].needed() && live_children[descendant] == 0)
                        || traversal.marks[descendant] == traversal.generation
                    {
                        continue;
                    }
                    traversal.marks[descendant] = traversal.generation;
                    traversal.stack.extend(
                        self.graph
                            .children
                            .get(descendant)
                            .ok_or(PackingError::Projection)?,
                    );
                    if states[descendant].queued() {
                        if adjustments[descendant].entries == 0 {
                            changed.push(descendant);
                        }
                        adjustments[descendant] = adjustments[descendant]
                            .checked_add(delta)
                            .ok_or(PackingError::Arithmetic)?;
                    }
                }
            }
            for descendant in changed.drain(..) {
                let previous = aggregates[descendant];
                if states[descendant] == CandidatePackingState::Modified
                    && !modified.remove(&PackageOrderKey::new(
                        descendant,
                        &self.candidates[descendant],
                        previous,
                    ))
                {
                    return Err(PackingError::Projection);
                }
                let remaining = previous
                    .checked_sub(adjustments[descendant])
                    .ok_or(PackingError::Projection)?;
                adjustments[descendant] = PackageAggregate::default();
                aggregates.to_mut()[descendant] = remaining;
                states[descendant] = CandidatePackingState::Modified;
                if !modified.insert(PackageOrderKey::new(
                    descendant,
                    &self.candidates[descendant],
                    remaining,
                )) {
                    return Err(PackingError::Projection);
                }
            }
        }

        let ordered = self.order_packed_indices(selected)?;
        let mut entries = Vec::with_capacity(ordered.len());
        let mut final_bytes = 0usize;
        let mut final_cycles = 0u64;
        for index in ordered {
            let candidate = &self.candidates[index];
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

    /// A residual chain has one forced parent-first order. Selected ancestors
    /// cannot change that order; stop at them instead of revisiting the prefix.
    /// Any residual fork falls back to the complete preference-ordered closure.
    fn chain_package(
        &self,
        mut index: usize,
        states: &[CandidatePackingState],
        package: &mut Vec<usize>,
    ) -> Result<bool, PackingError> {
        package.clear();
        loop {
            match states.get(index).ok_or(PackingError::Projection)? {
                CandidatePackingState::Ineligible | CandidatePackingState::Selected => {
                    return Err(PackingError::Projection);
                }
                _ => package.push(index),
            }
            let mut next = None;
            for &parent in self
                .graph
                .parents
                .get(index)
                .ok_or(PackingError::Projection)?
            {
                if *states.get(parent).ok_or(PackingError::Projection)?
                    != CandidatePackingState::Selected
                    && next.replace(parent).is_some()
                {
                    return Ok(false);
                }
            }
            match next {
                Some(parent) => index = parent,
                None => {
                    package.reverse();
                    return Ok(true);
                }
            }
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The checked graph supplies the complete ancestor closure and its local index mapping."
    )]
    fn ordered_package(
        &self,
        index: usize,
        states: &[CandidatePackingState],
        traversal: &mut Traversal,
        positions: &mut [usize],
        package: &mut Vec<usize>,
    ) -> Result<(), PackingError> {
        package.clear();
        traversal.begin()?;
        traversal.stack.push(index);
        while let Some(member) = traversal.stack.pop() {
            if traversal.marks[member] == traversal.generation {
                continue;
            }
            if states[member] == CandidatePackingState::Ineligible {
                return Err(PackingError::Projection);
            }
            traversal.marks[member] = traversal.generation;
            positions[member] = package.len();
            package.push(member);
            traversal.stack.extend(
                self.graph
                    .parents
                    .get(member)
                    .ok_or(PackingError::Projection)?,
            );
        }
        // Keep already selected ancestors in this local Kahn traversal. The
        // full closure reproduces the global preference order; omit them only
        // from output. Build local edges without scanning external fanout.
        let mut edges = Vec::new();
        let mut indegree = Vec::with_capacity(package.len());
        let mut ready = Vec::new();
        for (child, &member) in package.iter().enumerate() {
            let parents = self
                .graph
                .parents
                .get(member)
                .ok_or(PackingError::Projection)?;
            indegree.push(parents.len());
            if parents.is_empty() {
                ready.push(PackageOrderKey::new(
                    member,
                    &self.candidates[member],
                    self.graph.ancestors[member],
                ));
            }
            for &parent in parents {
                edges.push((positions[parent], child));
            }
        }
        let children = Links::from_edges(package.len(), &edges)?;
        let mut ready = BinaryHeap::from(ready);
        let mut ordered = Vec::with_capacity(package.len());
        let mut visited = 0usize;
        while let Some(key) = ready.pop() {
            let member = key.index;
            visited = visited.checked_add(1).ok_or(PackingError::Arithmetic)?;
            if states[member] != CandidatePackingState::Selected {
                ordered.push(member);
            }
            for &child in children
                .get(positions[member])
                .ok_or(PackingError::Projection)?
            {
                indegree[child] = indegree[child]
                    .checked_sub(1)
                    .ok_or(PackingError::Projection)?;
                if indegree[child] == 0 {
                    let member = package[child];
                    ready.push(PackageOrderKey::new(
                        member,
                        &self.candidates[member],
                        self.graph.ancestors[member],
                    ));
                }
            }
        }
        if visited != package.len() {
            return Err(PackingError::CausalCycle);
        }
        *package = ordered;
        Ok(())
    }
}

/// Called only when a queued/examining candidate permanently loses its own
/// queue use. Child uses are monotone, so every retiring edge is visited once.
#[expect(
    clippy::indexing_slicing,
    reason = "All parents and overlay indices belong to the same compiled graph."
)]
fn retire_candidate(
    index: usize,
    states: &[CandidatePackingState],
    live_children: &mut [usize],
    parents: &Links,
    stack: &mut Vec<usize>,
) -> Result<(), PackingError> {
    if live_children[index] != 0 {
        return Ok(());
    }
    stack.clear();
    stack.push(index);
    while let Some(retired) = stack.pop() {
        for &parent in parents.get(retired).ok_or(PackingError::Projection)? {
            live_children[parent] = live_children[parent]
                .checked_sub(1)
                .ok_or(PackingError::Projection)?;
            if live_children[parent] == 0 && !states[parent].needed() {
                stack.push(parent);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/packing.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/packing_graph.rs"]
mod graph_tests;
