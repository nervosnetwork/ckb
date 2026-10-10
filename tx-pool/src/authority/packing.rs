//! Selection borrows immutable accepted owners. Causal ancestry determines fee
//! priority; complete read-before-spend prerequisites determine block eligibility.

mod graph;
mod ordering;
mod run;
mod traversal;

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

#[cfg(test)]
#[path = "tests/packing.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/packing_graph.rs"]
mod graph_tests;
