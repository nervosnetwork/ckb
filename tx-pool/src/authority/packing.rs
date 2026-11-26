//! Selection borrows immutable accepted owners. Causal ancestry determines fee
//! priority; complete read-before-spend prerequisites determine block eligibility.

mod graph;
mod ordering;
mod run;
mod traversal;

use super::{
    eviction::EvictionRank,
    membership::Aggregate,
    model::{self, Accepted, Error, Status},
    state::FastHashMap,
    store::Captured,
};
use crate::component::{
    entry::TxEntry,
    sort_key::{AncestorsScoreSortKey, TransactionPriority},
};
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, tx_pool::get_transaction_weight},
    packed::{Byte32, ProposalShortId, ProposalShortIdReader},
    prelude::{Reader, Unpack},
};
use graph::{Graph, Links};
use ordering::Precedence;
use run::PackingRun;
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    sync::{Arc, Weak},
};
use traversal::Traversal;

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
        Self::capture_with(owners, status)
    }

    fn capture_with(
        owners: &'a [Arc<model::Entry>],
        status: impl Fn(ProposalShortIdReader<'_>) -> Status,
    ) -> Vec<Self> {
        owners
            .iter()
            .filter_map(|owner| {
                owner.accepted().map(|accepted| {
                    let proposal = owner.transaction.proposal_short_id_reader();
                    let phase = accepted.status_with(|| status(proposal));
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

    pub(super) fn owner(&self) -> &Arc<model::Entry> {
        self.owner
    }
}

/// Incremental hash-indexed projection of `ProposalView` owned by the sole
/// template loop. Direct successor transitions apply only changed short IDs
/// via `ProposalView::status_changes` without traversing the persistent tree.
#[derive(Default)]
struct CachedProposals {
    view: ProposalView,
    statuses: FastHashMap<[u8; ProposalShortId::TOTAL_SIZE], Status>,
}

impl CachedProposals {
    fn update(&mut self, next: &ProposalView) {
        if let Some(changes) = self.view.status_changes(next, usize::MAX) {
            for (id, _, after) in changes {
                let key: [u8; ProposalShortId::TOTAL_SIZE] = id.unpack();
                if after == Status::Pending {
                    self.statuses.remove(&key);
                } else {
                    self.statuses.insert(key, after);
                }
            }
        } else {
            self.statuses.clear();
            for id in next.gap_ids() {
                self.statuses.insert(id.unpack(), Status::Gap);
            }
            for id in next.proposed_ids() {
                self.statuses.insert(id.unpack(), Status::Proposed);
            }
        }
        self.view = next.clone();
    }

    fn status(&self, id: ProposalShortIdReader<'_>) -> Status {
        if self.statuses.is_empty() {
            return Status::Pending;
        }
        let mut key = [0u8; ProposalShortId::TOTAL_SIZE];
        key.copy_from_slice(id.as_slice());
        self.statuses.get(&key).copied().unwrap_or(Status::Pending)
    }
}

/// Optional reuse by the sole template loop. Weak source identities retain no
/// retired transaction payload; proposal phase is recomputed for every capture.
#[derive(Default)]
pub(super) struct Cache {
    graph: Option<CachedGraph>,
    proposals: CachedProposals,
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
    #[cfg(test)]
    pub(super) fn selection<'a>(
        &mut self,
        owners: &'a [Arc<model::Entry>],
        snapshot: &Snapshot,
        max_ancestors: usize,
    ) -> Result<Selection<'a>, Error> {
        self.selection_with(owners, snapshot, max_ancestors, false)
    }

    pub(super) fn selection_captured<'a>(
        &mut self,
        captured: &'a Captured,
        max_ancestors: usize,
    ) -> Result<Selection<'a>, Error> {
        self.selection_with(
            &captured.owners,
            &captured.snapshot,
            max_ancestors,
            captured.independent_leaves,
        )
    }

    fn selection_with<'a>(
        &mut self,
        owners: &'a [Arc<model::Entry>],
        snapshot: &Snapshot,
        max_ancestors: usize,
        independent_leaves: bool,
    ) -> Result<Selection<'a>, Error> {
        self.proposals.update(snapshot.proposals());
        let candidates = Candidate::capture_with(owners, |id| self.proposals.status(id));
        let graph = match &self.graph {
            Some(cached) if cached.matches(&candidates, max_ancestors) => Arc::clone(&cached.graph),
            _ => {
                // Release obsolete derivations before building their replacement.
                self.graph = None;
                let graph = Arc::new(if independent_leaves {
                    Graph::new_independent(&candidates, max_ancestors)?
                } else {
                    Graph::new(&candidates, max_ancestors)?
                });
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
        Ok(Selection {
            candidates,
            graph,
            independent_leaves,
        })
    }
}

/// Candidate order is the graph's node order. Construction either compiles that
/// exact population or reuses a cache whose source identities match in order.
pub(super) struct Selection<'a> {
    candidates: Vec<Candidate<'a>>,
    graph: Arc<Graph>,
    independent_leaves: bool,
}
impl<'a> Selection<'a> {
    pub(super) fn new(
        owners: &'a [Arc<model::Entry>],
        snapshot: &Snapshot,
        max_ancestors: usize,
    ) -> Result<Self, Error> {
        let candidates = Candidate::capture(owners, snapshot);
        let graph = Arc::new(Graph::new(&candidates, max_ancestors)?);
        Ok(Self {
            candidates,
            graph,
            independent_leaves: false,
        })
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
            let mut pass = traversal.begin()?;
            pass.stack.push(index);
            while let Some(ancestor) = pass.stack.pop() {
                if !pass.visit(ancestor) {
                    continue;
                }
                totals[ancestor] = totals[ancestor]
                    .add(own)
                    .map_err(|_| PackingError::Arithmetic)?;
                pass.stack.extend(&self.graph.parents[ancestor]);
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
                    candidate.hash,
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

    /// Resolve block prerequisites once for both proposals and transactions.
    pub(super) fn for_block(&self) -> Result<BlockSelection<'_, 'a>, PackingError> {
        Ok(BlockSelection {
            selection: self,
            precedence: self.precedence()?,
        })
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
            cycles: candidate.accepted.scripts.cycles(),
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

type PackageOrderKey<'a> = (TransactionPriority<&'a Byte32>, Reverse<usize>);

fn package_order_key<'a>(
    index: usize,
    candidate: &'a Candidate<'_>,
    aggregate: PackageAggregate,
) -> PackageOrderKey<'a> {
    (
        TransactionPriority {
            score: AncestorsScoreSortKey {
                fee: candidate.accepted.fee,
                weight: get_transaction_weight(
                    candidate.accepted.size,
                    candidate.accepted.scripts.cycles(),
                ),
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

/// One captured population and its acyclic read-before-spend prerequisites.
/// Proposal prefixes and transaction packages use the same cycle decisions.
/// Transaction packing consumes the plan, releasing it before DAO calculation.
pub(super) struct BlockSelection<'selection, 'owner> {
    selection: &'selection Selection<'owner>,
    precedence: Precedence,
}

impl BlockSelection<'_, '_> {
    /// Emit each Pending prerequisite before its consumer. Every prefix remains
    /// closed over Pending prerequisites, including after optional byte fitting.
    #[expect(
        clippy::indexing_slicing,
        reason = "Priority and prerequisite indices belong to this captured population."
    )]
    pub(super) fn proposal_short_ids(&self, limit: u64) -> Vec<ProposalShortId> {
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        if limit == 0 {
            return Vec::new();
        }
        let mut priority = self.selection.priority(Some(Status::Pending));
        let mut proposals = Vec::with_capacity(limit.min(priority.len()));
        if priority.is_empty() {
            return proposals;
        }
        let mut visited = vec![false; self.selection.candidates.len()];
        let mut stack = Vec::new();
        while let Some((_, Reverse(index))) = priority.pop() {
            if visited[index] || !self.precedence.active[index] {
                continue;
            }
            visited[index] = true;
            stack.push((index, self.precedence.prerequisites(index).iter()));
            while let Some((index, parents)) = stack.last_mut() {
                if let Some(&parent) = parents.next() {
                    if !visited[parent] {
                        visited[parent] = true;
                        stack.push((parent, self.precedence.prerequisites(parent).iter()));
                    }
                } else {
                    let candidate = &self.selection.candidates[*index];
                    stack.pop();
                    // Gap and Proposed need no slot, but their Pending
                    // prerequisites must still be visited. Marks are shared
                    // across roots; each node and edge is traversed at most once.
                    if candidate.status == Status::Pending {
                        proposals.push(candidate.proposal.to_entity());
                        if proposals.len() == limit {
                            return proposals;
                        }
                    }
                }
            }
        }
        proposals
    }

    pub(super) fn pack_transactions(
        self,
        limits: TemplatePackingLimits,
    ) -> Result<Vec<TxEntry>, PackingError> {
        self.pack_transactions_with_failure_bound(limits, MAX_CONSECUTIVE_PACKING_FAILURES)
    }

    fn pack_transactions_with_failure_bound(
        self,
        limits: TemplatePackingLimits,
        max_consecutive_failures: usize,
    ) -> Result<Vec<TxEntry>, PackingError> {
        let Some(run) = PackingRun::new(&self, limits)? else {
            return Ok(Vec::new());
        };
        run.pack(max_consecutive_failures)
    }
}

#[cfg(test)]
#[path = "tests/packing.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/packing_graph.rs"]
mod graph_tests;

#[cfg(test)]
#[path = "tests/packing_proposals.rs"]
mod proposal_tests;
