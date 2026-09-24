//! Selection borrows immutable accepted owners. Causal ancestry determines fee
//! priority; complete read-before-spend prerequisites determine block eligibility.

mod graph;
mod ordering;

use super::{
    membership::{Aggregate, EvictionRank},
    model::{self, Accepted, Error, Status},
};
use crate::component::{
    entry::TxEntry,
    sort_key::{AncestorsScoreSortKey, TransactionPriority},
};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, tx_pool::get_transaction_weight},
    packed::{Byte32, ProposalShortId, ProposalShortIdReader},
    prelude::Reader,
};
use graph::{Graph, Links};
use ordering::Precedence;
use std::{
    borrow::Cow,
    cmp::Reverse,
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

/// Candidate order is the graph's node order. Construction either compiles that
/// exact population or reuses a cache whose source identities match in order.
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
                traversal.stack.extend(&self.graph.parents[ancestor]);
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

    fn priority(&self, status: Option<Status>) -> BinaryHeap<PackageOrderKey<'_>> {
        BinaryHeap::from(
            self.candidates
                .iter()
                .zip(&self.graph.ancestors)
                .enumerate()
                .filter(|(_, (candidate, _))| {
                    status.is_none_or(|status| candidate.status == status)
                })
                .map(|(index, (candidate, &ancestors))| {
                    package_order_key(index, candidate, ancestors)
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
        std::iter::from_fn(move || {
            priority
                .pop()
                .map(|(_, Reverse(index))| &self.candidates[index])
        })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "The priority heap contains only indices of this candidate slice."
    )]
    pub(super) fn proposal_short_ids(&self, limit: u64) -> Vec<ProposalShortId> {
        let mut priority = self.priority(Some(Status::Pending));
        std::iter::from_fn(|| priority.pop())
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(|(_, Reverse(index))| self.candidates[index].proposal.to_entity())
            .collect()
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

type PackageOrderKey<'a> = (TransactionPriority<'a>, Reverse<usize>);

fn package_order_key<'a>(
    index: usize,
    candidate: &'a Candidate<'_>,
    aggregate: PackageAggregate,
) -> PackageOrderKey<'a> {
    (
        TransactionPriority {
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
        },
        Reverse(index),
    )
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

    fn pack_transactions_with_failure_bound(
        &self,
        limits: TemplatePackingLimits,
        max_consecutive_failures: usize,
    ) -> Result<Vec<TxEntry>, PackingError> {
        let Some(run) = PackingRun::new(self, limits)? else {
            return Ok(Vec::new());
        };
        run.pack(max_consecutive_failures)
    }
}

/// All mutable state for one block selection. Queue membership, selected
/// ancestors and descendant scores advance together after each package.
struct PackingRun<'selection, 'owner> {
    selection: &'selection Selection<'owner>,
    precedence: Precedence,
    limits: TemplatePackingLimits,
    aggregates: Cow<'selection, [PackageAggregate]>,
    states: Vec<CandidatePackingState>,
    original: BinaryHeap<PackageOrderKey<'selection>>,
    modified: BTreeSet<PackageOrderKey<'selection>>,
    live_children: Vec<usize>,
    selected: Vec<usize>,
    selected_bytes: usize,
    selected_cycles: Cycle,
    consecutive_failures: usize,
    minimum_bytes: usize,
    traversal: Traversal,
    package: Vec<usize>,
    adjustments: Vec<PackageAggregate>,
    changed: Vec<usize>,
}

#[expect(
    clippy::indexing_slicing,
    reason = "The compiled graph defines every candidate, edge and overlay index; all queues are populated from those indices."
)]
impl<'selection, 'owner> PackingRun<'selection, 'owner> {
    fn new(
        selection: &'selection Selection<'owner>,
        limits: TemplatePackingLimits,
    ) -> Result<Option<Self>, PackingError> {
        let len = selection.candidates.len();
        let precedence = selection.precedence()?;
        if precedence.eligible.is_empty() {
            return Ok(None);
        }
        let aggregates = Cow::Borrowed(selection.graph.ancestors.as_slice());
        let mut states = vec![CandidatePackingState::Ineligible; len];
        let mut original = Vec::with_capacity(precedence.eligible.len());
        let mut minimum_bytes = usize::MAX;
        for &index in &precedence.eligible {
            if aggregates[index].fits(limits) {
                minimum_bytes = minimum_bytes.min(selection.candidates[index].accepted.size);
                states[index] = CandidatePackingState::Original;
                original.push(package_order_key(
                    index,
                    &selection.candidates[index],
                    aggregates[index],
                ));
            }
        }
        if original.is_empty() {
            return Ok(None);
        }
        let mut live_children = vec![0usize; len];
        for &index in selection.graph.topological.iter().rev() {
            if states[index].needed() || live_children[index] != 0 {
                for &parent in &selection.graph.parents[index] {
                    live_children[parent] = live_children[parent]
                        .checked_add(1)
                        .ok_or(PackingError::Arithmetic)?;
                }
            }
        }
        let eligible_count = precedence.eligible.len();
        Ok(Some(Self {
            selection,
            precedence,
            limits,
            aggregates,
            states,
            original: BinaryHeap::from(original),
            modified: BTreeSet::new(),
            live_children,
            selected: Vec::with_capacity(eligible_count),
            selected_bytes: 0,
            selected_cycles: 0,
            consecutive_failures: 0,
            minimum_bytes,
            traversal: Traversal::new(len),
            package: Vec::new(),
            adjustments: vec![PackageAggregate::default(); len],
            changed: Vec::new(),
        }))
    }

    fn pack(mut self, max_consecutive_failures: usize) -> Result<Vec<TxEntry>, PackingError> {
        while let Some((index, aggregate)) = self.next_candidate()? {
            let Some(actual) = self.collect_package(index, aggregate)? else {
                self.reject_package(index)?;
                // Unfitting spend packages must not prevent their independently
                // fitting readers from making the first progress in this block.
                if self.consecutive_failures > max_consecutive_failures && !self.selected.is_empty()
                {
                    break;
                }
                continue;
            };
            self.select_package(actual)?;
            // Every remaining package contains a queued candidate's own bytes,
            // even after selected ancestors are subtracted. Keep checked-add
            // errors observable when synthetic limits approach integer bounds.
            if self
                .limits
                .serialized_bytes
                .saturating_sub(self.selected_bytes)
                < self.minimum_bytes
                && self
                    .selected_bytes
                    .checked_add(self.limits.serialized_bytes)
                    .is_some()
                && self
                    .selected_cycles
                    .checked_add(self.limits.cycles)
                    .is_some()
            {
                break;
            }
            self.reprice_descendants()?;
        }
        self.into_entries()
    }

    /// Take and validate the highest live score from the original or updated
    /// queue. Its state becomes Examining until the package settles.
    fn next_candidate(&mut self) -> Result<Option<(usize, PackageAggregate)>, PackingError> {
        while self.original.peek().is_some_and(|(_, Reverse(index))| {
            self.states[*index] != CandidatePackingState::Original
        }) {
            self.original.pop();
        }
        let key = match (self.original.peek(), self.modified.last()) {
            (None, None) => return Ok(None),
            (Some(initial), Some(updated)) if updated > initial => self.modified.pop_last(),
            (None, Some(_)) => self.modified.pop_last(),
            _ => self.original.pop(),
        }
        .ok_or(PackingError::Projection)?;
        let (_, Reverse(index)) = key;
        let aggregate = self.aggregates[index];
        if !self.states[index].queued()
            || key != package_order_key(index, &self.selection.candidates[index], aggregate)
        {
            return Err(PackingError::Projection);
        }
        self.states[index] = CandidatePackingState::Examining;
        Ok(Some((index, aggregate)))
    }

    fn collect_package(
        &mut self,
        index: usize,
        aggregate: PackageAggregate,
    ) -> Result<Option<PackageAggregate>, PackingError> {
        let remaining = TemplatePackingLimits::new(
            self.limits
                .serialized_bytes
                .checked_sub(self.selected_bytes)
                .ok_or(PackingError::Projection)?,
            self.limits
                .cycles
                .checked_sub(self.selected_cycles)
                .ok_or(PackingError::Projection)?,
        );
        // Preserve checked projected totals even for synthetic integer limits.
        self.selected_bytes
            .checked_add(aggregate.serialized_bytes)
            .ok_or(PackingError::Arithmetic)?;
        self.selected_cycles
            .checked_add(aggregate.cycles)
            .ok_or(PackingError::Arithmetic)?;
        self.precedence.collect_package(
            self.selection,
            index,
            &self.states,
            remaining,
            &mut self.traversal,
            &mut self.package,
        )
    }

    fn reject_package(&mut self, index: usize) -> Result<(), PackingError> {
        self.states[index] = CandidatePackingState::Failed;
        retire_candidate(
            index,
            &self.states,
            &mut self.live_children,
            &self.selection.graph.parents,
            &mut self.traversal.stack,
        )?;
        self.consecutive_failures = self
            .consecutive_failures
            .checked_add(1)
            .ok_or(PackingError::Arithmetic)?;
        Ok(())
    }

    /// Finish the entire package before visiting its remaining consumers.
    fn select_package(&mut self, actual: PackageAggregate) -> Result<(), PackingError> {
        for &member in self.package.iter().rev() {
            let previous = self.states[member];
            if previous == CandidatePackingState::Modified
                && !self.modified.remove(&package_order_key(
                    member,
                    &self.selection.candidates[member],
                    self.aggregates[member],
                ))
            {
                return Err(PackingError::Projection);
            }
            self.states[member] = CandidatePackingState::Selected;
            if previous.needed() {
                retire_candidate(
                    member,
                    &self.states,
                    &mut self.live_children,
                    &self.selection.graph.parents,
                    &mut self.traversal.stack,
                )?;
            }
        }
        self.selected.extend_from_slice(&self.package);
        self.selected_bytes = self
            .selected_bytes
            .checked_add(actual.serialized_bytes)
            .ok_or(PackingError::Arithmetic)?;
        self.selected_cycles = self
            .selected_cycles
            .checked_add(actual.cycles)
            .ok_or(PackingError::Arithmetic)?;
        self.consecutive_failures = 0;
        Ok(())
    }

    /// Subtract selected members once per live descendant, then replace each
    /// affected queue score after all package members have been counted.
    fn reprice_descendants(&mut self) -> Result<(), PackingError> {
        for &member in &self.package {
            let delta = PackageAggregate::one(&self.selection.candidates[member]);
            self.traversal.begin()?;
            self.traversal
                .stack
                .extend(&self.selection.graph.children[member]);
            while let Some(descendant) = self.traversal.stack.pop() {
                if (!self.states[descendant].needed() && self.live_children[descendant] == 0)
                    || self.traversal.marks[descendant] == self.traversal.generation
                {
                    continue;
                }
                self.traversal.marks[descendant] = self.traversal.generation;
                self.traversal
                    .stack
                    .extend(&self.selection.graph.children[descendant]);
                if self.states[descendant].queued() {
                    if self.adjustments[descendant].entries == 0 {
                        self.changed.push(descendant);
                    }
                    self.adjustments[descendant] = self.adjustments[descendant]
                        .checked_add(delta)
                        .ok_or(PackingError::Arithmetic)?;
                }
            }
        }
        for descendant in self.changed.drain(..) {
            let previous = self.aggregates[descendant];
            if self.states[descendant] == CandidatePackingState::Modified
                && !self.modified.remove(&package_order_key(
                    descendant,
                    &self.selection.candidates[descendant],
                    previous,
                ))
            {
                return Err(PackingError::Projection);
            }
            let remaining = previous
                .checked_sub(self.adjustments[descendant])
                .ok_or(PackingError::Projection)?;
            self.adjustments[descendant] = PackageAggregate::default();
            self.aggregates.to_mut()[descendant] = remaining;
            self.states[descendant] = CandidatePackingState::Modified;
            if !self.modified.insert(package_order_key(
                descendant,
                &self.selection.candidates[descendant],
                remaining,
            )) {
                return Err(PackingError::Projection);
            }
        }
        Ok(())
    }

    fn into_entries(self) -> Result<Vec<TxEntry>, PackingError> {
        let mut entries = Vec::with_capacity(self.selected.len());
        let mut final_bytes = 0usize;
        let mut final_cycles = 0u64;
        for index in self.selected {
            let candidate = &self.selection.candidates[index];
            final_bytes = final_bytes
                .checked_add(candidate.accepted.size)
                .ok_or(PackingError::Arithmetic)?;
            final_cycles = final_cycles
                .checked_add(candidate.accepted.cycles)
                .ok_or(PackingError::Arithmetic)?;
            entries.push(candidate.accepted.projection());
        }
        if final_bytes > self.limits.serialized_bytes || final_cycles > self.limits.cycles {
            return Err(PackingError::Projection);
        }
        Ok(entries)
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
        for &parent in &parents[retired] {
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
