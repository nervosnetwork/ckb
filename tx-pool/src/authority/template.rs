//! Immutable tx-pool input for concurrent block-template construction.
//!
//! The authority owns accepted membership and status. This module captures one
//! coherent, bounded read receipt containing only immutable `Arc` payloads and
//! payload-free derived order/relation facts. Topology and packing work run
//! after the authority guard opens; the block assembler remains a separate
//! owner of rebuildable output.

use super::{
    plan::{
        AcceptedOrderKey, AncestorAggregate, EvictionOrderKey, MembershipProjection, StatusCounts,
    },
    shard::ShardedOwnerReadCut,
    source::PoolTemplateVersions,
    state::{
        AcceptedAtMillis, AcceptedStatus, ApplySequence, CandidateMetrics, ChainViewId,
        ExpandedFootprint, OwnedTx, ProposalId, RawTxHash,
    },
};
use crate::block_assembler::{CandidateUncleSourceReceipt, ResetEpoch, TemplateRevision};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::cell::ResolvedTransaction,
    packed::{OutPoint, ProposalShortId},
};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

/// Exact SCC shedding removes one weakest feedback vertex from every cyclic
/// component per round. Dense hostile components eventually retain only their
/// strongest representative so template construction remains deterministic
/// and bounded.
const MAX_CONDITIONAL_CYCLE_ROUNDS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateReadError {
    Allocation,
    Arithmetic,
    Projection,
    CausalCycle,
}

#[derive(Clone, Debug)]
pub(super) struct TemplateCandidate {
    hash: RawTxHash,
    proposal: ProposalId,
    status: AcceptedStatus,
    accepted_at: AcceptedAtMillis,
    metrics: CandidateMetrics,
    ancestors: AncestorAggregate,
    resolved: Arc<ResolvedTransaction>,
    footprint: Arc<ExpandedFootprint>,
    parents: Vec<RawTxHash>,
    order: AcceptedOrderKey,
    eviction: EvictionOrderKey,
}

impl TemplateCandidate {
    pub(super) fn hash(&self) -> &RawTxHash {
        &self.hash
    }

    pub(super) fn proposal_short_id(&self) -> &ProposalShortId {
        &self.proposal.0
    }

    pub(super) fn accepted_at(&self) -> AcceptedAtMillis {
        self.accepted_at
    }

    pub(super) fn metrics(&self) -> &CandidateMetrics {
        &self.metrics
    }

    pub(super) fn ancestors(&self) -> AncestorAggregate {
        self.ancestors
    }

    pub(super) fn resolved(&self) -> &Arc<ResolvedTransaction> {
        &self.resolved
    }

    #[cfg(test)]
    pub(super) fn dependency_edge_count(&self) -> usize {
        self.footprint.dependencies().len()
    }

    pub(super) fn parents(&self) -> &[RawTxHash] {
        &self.parents
    }

    pub(super) fn order(&self) -> &AcceptedOrderKey {
        &self.order
    }
}

#[derive(Debug)]
struct CapturedAccepted {
    hash: RawTxHash,
    proposal: ProposalId,
    status: AcceptedStatus,
    accepted_at: AcceptedAtMillis,
    metrics: CandidateMetrics,
    ancestors: AncestorAggregate,
    resolved: Arc<ResolvedTransaction>,
    footprint: Arc<ExpandedFootprint>,
    parents: Vec<RawTxHash>,
    order: AcceptedOrderKey,
    eviction: EvictionOrderKey,
}

#[derive(Debug)]
pub(super) struct AuthorityTemplateReadReceipt {
    chain_view: ChainViewId,
    sources: PoolTemplateVersions,
    captured: Vec<CapturedAccepted>,
    dependency_edge_bound: usize,
}

#[derive(Debug)]
pub(super) struct TemplateSelectionReceipt {
    sources: PoolTemplateVersions,
    candidates: Vec<TemplateCandidate>,
    dependency_edge_bound: usize,
}

/// One immutable tx-pool template input whose payloads, source versions and
/// chain snapshot were captured under the same authority-store read guard.
/// Construction work may consume this value only after that guard has opened;
/// no caller can splice a pool receipt from one chain view onto another
/// snapshot.
#[derive(Debug)]
pub(super) struct AuthorityTemplateInput {
    snapshot: Arc<Snapshot>,
    selection: TemplateSelectionReceipt,
}

impl AuthorityTemplateInput {
    pub(super) fn from_capture(
        snapshot: Arc<Snapshot>,
        receipt: AuthorityTemplateReadReceipt,
    ) -> Result<Self, TemplateReadError> {
        if receipt.chain_view().tip().0 != snapshot.tip_hash() {
            return Err(TemplateReadError::Projection);
        }
        Ok(Self {
            snapshot,
            selection: receipt.into_selection()?,
        })
    }

    pub(super) fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    pub(super) fn selection(&self) -> &TemplateSelectionReceipt {
        &self.selection
    }

    pub(super) fn pool_source_cut(&self) -> TemplatePoolSourceCut {
        TemplatePoolSourceCut(self.selection.sources)
    }

    pub(super) fn source_cut(&self, uncles: CandidateUncleSourceReceipt) -> TemplateSourceCut {
        self.selection.source_cut(uncles)
    }
}

impl AuthorityTemplateReadReceipt {
    pub(super) fn capture(
        chain_view: ChainViewId,
        sources: PoolTemplateVersions,
        entries: &ShardedOwnerReadCut<'_>,
        counts: Option<StatusCounts>,
        membership: &MembershipProjection,
    ) -> Result<Self, TemplateReadError> {
        let counts = counts.ok_or(TemplateReadError::Arithmetic)?;
        let accepted_count = counts
            .pending
            .checked_add(counts.gap)
            .and_then(|count| count.checked_add(counts.proposed))
            .ok_or(TemplateReadError::Arithmetic)?;
        let mut captured = Vec::new();
        captured
            .try_reserve(accepted_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut dependency_edge_bound = 0usize;
        for order in membership.accepted_order().rev() {
            let hash = order.hash();
            let Some(OwnedTx::Accepted(entry)) = entries.get(hash) else {
                return Err(TemplateReadError::Projection);
            };
            if hash != &entry.record.identity.raw
                || entry.proof.payload().identity() != &entry.record.identity
            {
                return Err(TemplateReadError::Projection);
            }
            let parent_set = membership
                .parents(hash)
                .ok_or(TemplateReadError::Projection)?;
            let ancestors = membership
                .ancestor_aggregate(hash)
                .ok_or(TemplateReadError::Projection)?;
            let eviction = membership
                .eviction_order_for(hash, entry)
                .ok_or(TemplateReadError::Projection)?;
            dependency_edge_bound = dependency_edge_bound
                .checked_add(entry.proof.payload().footprint().dependencies().len())
                .ok_or(TemplateReadError::Arithmetic)?;
            let mut parents = Vec::new();
            parents
                .try_reserve(parent_set.len())
                .map_err(|_| TemplateReadError::Allocation)?;
            parents.extend(parent_set.iter().cloned());
            captured.push(CapturedAccepted {
                hash: hash.clone(),
                proposal: entry.record.identity.proposal.clone(),
                status: entry.status(),
                accepted_at: entry.accepted_at,
                metrics: entry.proof.metrics().clone(),
                ancestors,
                resolved: Arc::clone(entry.proof.payload().resolved_transaction()),
                footprint: Arc::clone(entry.proof.payload().footprint()),
                parents,
                order: order.clone(),
                eviction,
            });
        }
        if captured.len() != accepted_count {
            return Err(TemplateReadError::Projection);
        }

        Ok(Self {
            chain_view,
            sources,
            captured,
            dependency_edge_bound,
        })
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        &self.chain_view
    }

    /// Consume the owned cut after the authority guard has been released.
    /// Parent canonicalization and later selection then touch no authority
    /// state or lock; exact ancestor ranking was already captured from the
    /// same membership projection as the payloads.
    pub(super) fn into_selection(mut self) -> Result<TemplateSelectionReceipt, TemplateReadError> {
        for entry in &mut self.captured {
            entry.parents.sort_unstable();
        }
        let mut candidates = Vec::new();
        candidates
            .try_reserve(self.captured.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for entry in self.captured {
            candidates.push(TemplateCandidate {
                hash: entry.hash,
                proposal: entry.proposal,
                status: entry.status,
                accepted_at: entry.accepted_at,
                metrics: entry.metrics,
                ancestors: entry.ancestors,
                resolved: entry.resolved,
                footprint: entry.footprint,
                parents: entry.parents,
                order: entry.order,
                eviction: entry.eviction,
            });
        }
        Ok(TemplateSelectionReceipt {
            sources: self.sources,
            candidates,
            dependency_edge_bound: self.dependency_edge_bound,
        })
    }
}

impl TemplateSelectionReceipt {
    pub(super) fn source_cut(&self, uncles: CandidateUncleSourceReceipt) -> TemplateSourceCut {
        TemplateSourceCut::new(self.sources, uncles)
    }

    pub(super) fn candidates(&self) -> &[TemplateCandidate] {
        &self.candidates
    }

    pub(super) fn proposal_short_ids(
        &self,
        limit: u64,
    ) -> Result<Vec<ProposalShortId>, TemplateReadError> {
        let ordered = self.ordered_indices([AcceptedStatus::Pending])?;
        let selected = match usize::try_from(limit) {
            Ok(limit) => limit.min(ordered.len()),
            Err(_) => ordered.len(),
        };
        let mut proposals = Vec::new();
        proposals
            .try_reserve(selected)
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in ordered.into_iter().take(selected) {
            proposals.push(
                self.candidates
                    .get(index)
                    .ok_or(TemplateReadError::Projection)?
                    .proposal_short_id()
                    .clone(),
            );
        }
        Ok(proposals)
    }

    pub(super) fn candidate_index(&self) -> Result<HashMap<RawTxHash, usize>, TemplateReadError> {
        let mut by_hash = HashMap::new();
        by_hash
            .try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for (index, candidate) in self.candidates.iter().enumerate() {
            if by_hash.insert(candidate.hash.clone(), index).is_some() {
                return Err(TemplateReadError::Projection);
            }
        }
        Ok(by_hash)
    }

    /// Proposed candidates whose complete causal ancestor closure is also
    /// Proposed, returned in deterministic parent-first order. Packing and the
    /// unbounded test projection share this one eligibility compiler.
    pub(super) fn causally_eligible_proposed(
        &self,
        by_hash: &HashMap<RawTxHash, usize>,
    ) -> Result<Vec<usize>, TemplateReadError> {
        let mut eligible = Vec::new();
        eligible
            .try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        eligible.resize(self.candidates.len(), false);
        let causal = causal_indices(&self.candidates, by_hash)?;
        for index in &causal {
            let candidate = self
                .candidates
                .get(*index)
                .ok_or(TemplateReadError::Projection)?;
            let mut parents_eligible = true;
            for parent in &candidate.parents {
                let parent_index = by_hash
                    .get(parent)
                    .copied()
                    .ok_or(TemplateReadError::Projection)?;
                if !eligible
                    .get(parent_index)
                    .copied()
                    .ok_or(TemplateReadError::Projection)?
                {
                    parents_eligible = false;
                    break;
                }
            }
            *eligible
                .get_mut(*index)
                .ok_or(TemplateReadError::Projection)? =
                candidate.status == AcceptedStatus::Proposed && parents_eligible;
        }

        let mut selected = Vec::new();
        selected
            .try_reserve(causal.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in causal {
            if eligible
                .get(index)
                .copied()
                .ok_or(TemplateReadError::Projection)?
            {
                selected.push(index);
            }
        }
        Ok(selected)
    }

    pub(super) fn order_packed_indices(
        &self,
        selected: Vec<usize>,
        by_hash: &HashMap<RawTxHash, usize>,
    ) -> Result<Vec<usize>, TemplateReadError> {
        self.order_conditionally_safe(selected, by_hash)
    }

    fn order_conditionally_safe(
        &self,
        selected: Vec<usize>,
        by_hash: &HashMap<RawTxHash, usize>,
    ) -> Result<Vec<usize>, TemplateReadError> {
        if selected.len() < 2 {
            return Ok(selected);
        }

        let mut rank = Vec::new();
        rank.try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        rank.resize(self.candidates.len(), None);
        let mut active = Vec::new();
        active
            .try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        active.resize(self.candidates.len(), false);
        for (position, index) in selected.iter().copied().enumerate() {
            let slot = rank.get_mut(index).ok_or(TemplateReadError::Projection)?;
            if slot.replace(position).is_some() {
                return Err(TemplateReadError::Projection);
            }
            *active.get_mut(index).ok_or(TemplateReadError::Projection)? = true;
        }

        let mut cycle_round = 0usize;
        loop {
            let graph = self.conditional_graph(&active, by_hash)?;
            let ordered = topological_active_order(&active, &rank, &graph.children)?;
            if ordered.len() == active.iter().filter(|is_active| **is_active).count() {
                return Ok(ordered);
            }

            let mut cyclic = strongly_connected_active(&active, &graph.children)?;
            cyclic.retain(|component| component.len() > 1);
            if cyclic.is_empty() {
                return Err(TemplateReadError::Projection);
            }
            cycle_round = cycle_round
                .checked_add(1)
                .ok_or(TemplateReadError::Arithmetic)?;
            let bounded_fallback = cycle_round > MAX_CONDITIONAL_CYCLE_ROUNDS;
            let mut roots = Vec::new();
            roots
                .try_reserve(active.len())
                .map_err(|_| TemplateReadError::Allocation)?;
            roots.resize(active.len(), false);
            for component in cyclic {
                let chosen = self.cycle_representative(&component, bounded_fallback)?;
                if bounded_fallback {
                    for index in component {
                        if index != chosen {
                            *roots.get_mut(index).ok_or(TemplateReadError::Projection)? = true;
                        }
                    }
                } else {
                    *roots.get_mut(chosen).ok_or(TemplateReadError::Projection)? = true;
                }
            }
            drop_causal_descendants(&mut active, roots, &graph.causal_children)?;
            if active.iter().filter(|is_active| **is_active).count() < 2 {
                let mut retained = Vec::new();
                retained
                    .try_reserve(1)
                    .map_err(|_| TemplateReadError::Allocation)?;
                retained.extend(
                    selected
                        .iter()
                        .copied()
                        .filter(|index| active.get(*index).is_some_and(|is_active| *is_active)),
                );
                return Ok(retained);
            }
        }
    }

    fn conditional_graph(
        &self,
        active: &[bool],
        by_hash: &HashMap<RawTxHash, usize>,
    ) -> Result<SelectedGraph, TemplateReadError> {
        if active.len() != self.candidates.len() {
            return Err(TemplateReadError::Projection);
        }
        let mut causal_edges = Vec::new();
        let mut input_count = 0usize;
        let mut dependency_count = 0usize;
        for (child, candidate) in self.candidates.iter().enumerate() {
            if !active
                .get(child)
                .copied()
                .ok_or(TemplateReadError::Projection)?
            {
                continue;
            }
            input_count = input_count
                .checked_add(candidate.resolved.transaction.inputs().len())
                .ok_or(TemplateReadError::Arithmetic)?;
            dependency_count = dependency_count
                .checked_add(candidate.footprint.dependencies().len())
                .ok_or(TemplateReadError::Arithmetic)?;
            for parent in &candidate.parents {
                let parent = *by_hash.get(parent).ok_or(TemplateReadError::Projection)?;
                if active.get(parent).is_some_and(|is_active| *is_active) {
                    causal_edges
                        .try_reserve(1)
                        .map_err(|_| TemplateReadError::Allocation)?;
                    causal_edges.push((parent, child));
                }
            }
        }
        if dependency_count > self.dependency_edge_bound {
            return Err(TemplateReadError::Projection);
        }
        let edge_capacity = causal_edges
            .len()
            .checked_add(dependency_count)
            .ok_or(TemplateReadError::Arithmetic)?;
        let mut edges = HashSet::new();
        edges
            .try_reserve(edge_capacity)
            .map_err(|_| TemplateReadError::Allocation)?;
        edges.extend(causal_edges.iter().copied());

        let mut spenders = HashMap::<OutPoint, usize>::new();
        spenders
            .try_reserve(input_count)
            .map_err(|_| TemplateReadError::Allocation)?;
        for (index, candidate) in self.candidates.iter().enumerate() {
            if !active
                .get(index)
                .copied()
                .ok_or(TemplateReadError::Projection)?
            {
                continue;
            }
            for input in candidate.resolved.transaction.input_pts_iter() {
                if spenders.insert(input, index).is_some() {
                    return Err(TemplateReadError::Projection);
                }
            }
        }
        for (reader, candidate) in self.candidates.iter().enumerate() {
            if !active
                .get(reader)
                .copied()
                .ok_or(TemplateReadError::Projection)?
            {
                continue;
            }
            for dependency in candidate.footprint.dependencies() {
                if let Some(spender) = spenders.get(dependency).copied()
                    && spender != reader
                {
                    edges.insert((reader, spender));
                }
            }
        }
        SelectedGraph::from_edges(active.len(), edges, causal_edges)
    }

    fn cycle_representative(
        &self,
        component: &[usize],
        strongest: bool,
    ) -> Result<usize, TemplateReadError> {
        let mut selected = *component.first().ok_or(TemplateReadError::Projection)?;
        for candidate in component.iter().copied().skip(1) {
            let selected_order = &self
                .candidates
                .get(selected)
                .ok_or(TemplateReadError::Projection)?
                .eviction;
            let candidate_order = &self
                .candidates
                .get(candidate)
                .ok_or(TemplateReadError::Projection)?
                .eviction;
            let replace = if strongest {
                candidate_order > selected_order
            } else {
                candidate_order < selected_order
            };
            if replace {
                selected = candidate;
            }
        }
        Ok(selected)
    }

    fn ordered_indices<const N: usize>(
        &self,
        statuses: [AcceptedStatus; N],
    ) -> Result<Vec<usize>, TemplateReadError> {
        let mut ordered = Vec::new();
        ordered
            .try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        ordered.extend(
            self.candidates
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    statuses.contains(&candidate.status).then_some(index)
                }),
        );
        Ok(ordered)
    }
}

struct SelectedGraph {
    children: Vec<Vec<usize>>,
    causal_children: Vec<Vec<usize>>,
}

impl SelectedGraph {
    fn from_edges(
        len: usize,
        edges: HashSet<(usize, usize)>,
        causal_edges: Vec<(usize, usize)>,
    ) -> Result<Self, TemplateReadError> {
        let mut child_counts = Vec::new();
        child_counts
            .try_reserve(len)
            .map_err(|_| TemplateReadError::Allocation)?;
        child_counts.resize(len, 0usize);
        for (parent, child) in &edges {
            if parent == child || *parent >= len || *child >= len {
                return Err(TemplateReadError::Projection);
            }
            let count = child_counts
                .get_mut(*parent)
                .ok_or(TemplateReadError::Projection)?;
            *count = count.checked_add(1).ok_or(TemplateReadError::Arithmetic)?;
        }
        let mut causal_counts = Vec::new();
        causal_counts
            .try_reserve(len)
            .map_err(|_| TemplateReadError::Allocation)?;
        causal_counts.resize(len, 0usize);
        for (parent, child) in &causal_edges {
            if parent == child || *parent >= len || *child >= len {
                return Err(TemplateReadError::Projection);
            }
            let count = causal_counts
                .get_mut(*parent)
                .ok_or(TemplateReadError::Projection)?;
            *count = count.checked_add(1).ok_or(TemplateReadError::Arithmetic)?;
        }

        let mut children = Vec::new();
        children
            .try_reserve(len)
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut causal_children = Vec::new();
        causal_children
            .try_reserve(len)
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in 0..len {
            let mut next = Vec::new();
            next.try_reserve(
                *child_counts
                    .get(index)
                    .ok_or(TemplateReadError::Projection)?,
            )
            .map_err(|_| TemplateReadError::Allocation)?;
            children.push(next);
            let mut causal_next = Vec::new();
            causal_next
                .try_reserve(
                    *causal_counts
                        .get(index)
                        .ok_or(TemplateReadError::Projection)?,
                )
                .map_err(|_| TemplateReadError::Allocation)?;
            causal_children.push(causal_next);
        }
        for (parent, child) in edges {
            children
                .get_mut(parent)
                .ok_or(TemplateReadError::Projection)?
                .push(child);
        }
        for (parent, child) in causal_edges {
            causal_children
                .get_mut(parent)
                .ok_or(TemplateReadError::Projection)?
                .push(child);
        }
        for next in &mut children {
            next.sort_unstable();
        }
        for next in &mut causal_children {
            next.sort_unstable();
        }
        Ok(Self {
            children,
            causal_children,
        })
    }
}

fn topological_active_order(
    active: &[bool],
    rank: &[Option<usize>],
    children: &[Vec<usize>],
) -> Result<Vec<usize>, TemplateReadError> {
    if active.len() != rank.len() || active.len() != children.len() {
        return Err(TemplateReadError::Projection);
    }
    let mut indegree = Vec::new();
    indegree
        .try_reserve(active.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    indegree.resize(active.len(), 0usize);
    for (parent, next) in children.iter().enumerate() {
        if !active
            .get(parent)
            .copied()
            .ok_or(TemplateReadError::Projection)?
        {
            continue;
        }
        for child in next {
            if !active.get(*child).is_some_and(|is_active| *is_active) {
                return Err(TemplateReadError::Projection);
            }
            let degree = indegree
                .get_mut(*child)
                .ok_or(TemplateReadError::Projection)?;
            *degree = degree.checked_add(1).ok_or(TemplateReadError::Arithmetic)?;
        }
    }

    let mut ready = BTreeSet::new();
    for (index, is_active) in active.iter().copied().enumerate() {
        if is_active
            && indegree
                .get(index)
                .copied()
                .ok_or(TemplateReadError::Projection)?
                == 0
        {
            let position = rank
                .get(index)
                .and_then(|position| *position)
                .ok_or(TemplateReadError::Projection)?;
            ready.insert((position, index));
        }
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve(active.iter().filter(|is_active| **is_active).count())
        .map_err(|_| TemplateReadError::Allocation)?;
    while let Some((_position, index)) = ready.pop_first() {
        ordered.push(index);
        for child in children.get(index).ok_or(TemplateReadError::Projection)? {
            let degree = indegree
                .get_mut(*child)
                .ok_or(TemplateReadError::Projection)?;
            *degree = degree.checked_sub(1).ok_or(TemplateReadError::Projection)?;
            if *degree == 0 {
                let position = rank
                    .get(*child)
                    .and_then(|position| *position)
                    .ok_or(TemplateReadError::Projection)?;
                ready.insert((position, *child));
            }
        }
    }
    Ok(ordered)
}

/// Iterative Kosaraju traversal; template input is attacker-shaped, so no
/// recursive stack growth is permitted.
fn strongly_connected_active(
    active: &[bool],
    children: &[Vec<usize>],
) -> Result<Vec<Vec<usize>>, TemplateReadError> {
    if active.len() != children.len() {
        return Err(TemplateReadError::Projection);
    }
    let mut visited = Vec::new();
    visited
        .try_reserve(active.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    visited.resize(active.len(), false);
    let mut finish = Vec::new();
    finish
        .try_reserve(active.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    let stack_capacity = active
        .len()
        .checked_mul(2)
        .ok_or(TemplateReadError::Arithmetic)?;
    let mut stack = Vec::new();
    stack
        .try_reserve(stack_capacity)
        .map_err(|_| TemplateReadError::Allocation)?;
    for start in 0..active.len() {
        if !active
            .get(start)
            .copied()
            .ok_or(TemplateReadError::Projection)?
            || visited
                .get(start)
                .copied()
                .ok_or(TemplateReadError::Projection)?
        {
            continue;
        }
        stack.push((start, false));
        while let Some((index, expanded)) = stack.pop() {
            if expanded {
                finish.push(index);
                continue;
            }
            let seen = visited
                .get_mut(index)
                .ok_or(TemplateReadError::Projection)?;
            if *seen {
                continue;
            }
            *seen = true;
            stack.push((index, true));
            for child in children
                .get(index)
                .ok_or(TemplateReadError::Projection)?
                .iter()
                .rev()
            {
                if !active.get(*child).is_some_and(|is_active| *is_active) {
                    return Err(TemplateReadError::Projection);
                }
                if !visited
                    .get(*child)
                    .copied()
                    .ok_or(TemplateReadError::Projection)?
                {
                    stack.push((*child, false));
                }
            }
        }
    }

    let mut parent_counts = Vec::new();
    parent_counts
        .try_reserve(active.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    parent_counts.resize(active.len(), 0usize);
    for (parent, next) in children.iter().enumerate() {
        if !active
            .get(parent)
            .copied()
            .ok_or(TemplateReadError::Projection)?
        {
            continue;
        }
        for child in next {
            let count = parent_counts
                .get_mut(*child)
                .ok_or(TemplateReadError::Projection)?;
            *count = count.checked_add(1).ok_or(TemplateReadError::Arithmetic)?;
        }
    }
    let mut parents = Vec::new();
    parents
        .try_reserve(active.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    for count in parent_counts {
        let mut row = Vec::new();
        row.try_reserve(count)
            .map_err(|_| TemplateReadError::Allocation)?;
        parents.push(row);
    }
    for (parent, next) in children.iter().enumerate() {
        if !active
            .get(parent)
            .copied()
            .ok_or(TemplateReadError::Projection)?
        {
            continue;
        }
        for child in next {
            parents
                .get_mut(*child)
                .ok_or(TemplateReadError::Projection)?
                .push(parent);
        }
    }
    for previous in &mut parents {
        previous.sort_unstable();
    }

    visited.fill(false);
    let mut components = Vec::new();
    components
        .try_reserve(active.iter().filter(|is_active| **is_active).count())
        .map_err(|_| TemplateReadError::Allocation)?;
    stack.clear();
    for start in finish.into_iter().rev() {
        if visited
            .get(start)
            .copied()
            .ok_or(TemplateReadError::Projection)?
        {
            continue;
        }
        *visited
            .get_mut(start)
            .ok_or(TemplateReadError::Projection)? = true;
        stack.push((start, false));
        let mut component = Vec::new();
        while let Some((index, _)) = stack.pop() {
            component
                .try_reserve(1)
                .map_err(|_| TemplateReadError::Allocation)?;
            component.push(index);
            for parent in parents
                .get(index)
                .ok_or(TemplateReadError::Projection)?
                .iter()
                .rev()
            {
                let seen = visited
                    .get_mut(*parent)
                    .ok_or(TemplateReadError::Projection)?;
                if !*seen {
                    *seen = true;
                    stack.push((*parent, false));
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    Ok(components)
}

fn drop_causal_descendants(
    active: &mut [bool],
    mut dropped: Vec<bool>,
    causal_children: &[Vec<usize>],
) -> Result<(), TemplateReadError> {
    if active.len() != dropped.len() || active.len() != causal_children.len() {
        return Err(TemplateReadError::Projection);
    }
    let mut stack = Vec::new();
    stack
        .try_reserve(active.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    for (index, is_dropped) in dropped.iter().copied().enumerate() {
        if is_dropped {
            if !active
                .get(index)
                .copied()
                .ok_or(TemplateReadError::Projection)?
            {
                return Err(TemplateReadError::Projection);
            }
            stack.push(index);
        }
    }
    if stack.is_empty() {
        return Err(TemplateReadError::Projection);
    }
    while let Some(index) = stack.pop() {
        for child in causal_children
            .get(index)
            .ok_or(TemplateReadError::Projection)?
        {
            let child_dropped = dropped
                .get_mut(*child)
                .ok_or(TemplateReadError::Projection)?;
            if !*child_dropped {
                *child_dropped = true;
                stack.push(*child);
            }
        }
    }
    for (index, is_dropped) in dropped.into_iter().enumerate() {
        if is_dropped {
            *active.get_mut(index).ok_or(TemplateReadError::Projection)? = false;
        }
    }
    Ok(())
}

/// Complete level input for template convergence. Pool versions are captured
/// with accepted payloads under one authority read guard; the uncle version
/// comes from the block assembler's independent bounded candidate authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplatePoolSourceCut(PoolTemplateVersions);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplateSourceCut {
    pool: TemplatePoolSourceCut,
    uncles: CandidateUncleSourceReceipt,
}

impl TemplateSourceCut {
    pub(super) fn new(pool: PoolTemplateVersions, uncles: CandidateUncleSourceReceipt) -> Self {
        Self {
            pool: TemplatePoolSourceCut(pool),
            uncles,
        }
    }

    fn join(self, incoming: Self) -> Self {
        Self {
            pool: self.pool.join(incoming.pool),
            uncles: self.uncles.max(incoming.uncles),
        }
    }

    pub(super) fn chain_source(self) -> ApplySequence {
        self.pool.0.chain
    }

    fn covers(self, target: Self) -> bool {
        self.pool.0.proposals >= target.pool.0.proposals
            && self.pool.0.transactions >= target.pool.0.transactions
            && self.pool.0.chain >= target.pool.0.chain
            && self.uncles >= target.uncles
    }
}

impl TemplatePoolSourceCut {
    pub(super) fn new(versions: PoolTemplateVersions) -> Self {
        Self(versions)
    }

    fn join(self, incoming: Self) -> Self {
        Self(PoolTemplateVersions {
            proposals: self.0.proposals.max(incoming.0.proposals),
            transactions: self.0.transactions.max(incoming.0.transactions),
            chain: self.0.chain.max(incoming.0.chain),
        })
    }

    pub(super) fn proposal_cut(self) -> ProposalSourceCut {
        ProposalSourceCut {
            selection: self.0.proposals,
            chain: self.0.chain,
        }
    }

    pub(super) fn transaction_cut(self) -> TransactionSourceCut {
        TransactionSourceCut {
            selection: self.0.transactions,
            chain: self.0.chain,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProposalSourceCut {
    selection: ApplySequence,
    chain: ApplySequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TransactionSourceCut {
    selection: ApplySequence,
    chain: ApplySequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UncleSourceCut {
    candidates: CandidateUncleSourceReceipt,
    chain: ApplySequence,
    proposals: ApplySequence,
}

impl UncleSourceCut {
    fn proposal_cut(self) -> ProposalSourceCut {
        ProposalSourceCut {
            selection: self.proposals,
            chain: self.chain,
        }
    }
}

impl TemplateSourceCut {
    pub(super) fn proposal_cut(self) -> ProposalSourceCut {
        self.pool.proposal_cut()
    }

    pub(super) fn transaction_cut(self) -> TransactionSourceCut {
        self.pool.transaction_cut()
    }

    pub(super) fn uncle_cut(self) -> UncleSourceCut {
        UncleSourceCut {
            candidates: self.uncles,
            chain: self.pool.0.chain,
            proposals: self.pool.0.proposals,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateComponent {
    Proposals,
    Transactions,
    Uncles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplatePublication {
    Published,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateConvergenceError {
    ResetEpochExhausted,
}

/// Total miner-facing observation of the chain component of the rebuildable
/// template projection.  `Pending` has the replacement lane as its named
/// releaser; `Failed` is the terminal ordinary outcome for the exact unchanged
/// source cut.  None of these states owns chain or transaction policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateChainReadState {
    Published,
    Pending,
    Failed,
}

/// A published component can be chain-safe without having captured an exact
/// pool/candidate source.  The initial base template is the only such state:
/// it is a valid underfilled template for one chain cut, while every component
/// lane must still observe its exact source before declaring convergence.
///
/// Keeping this distinction in the component receipt avoids both false exact
/// coverage and a duplicate scalar read-readiness authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublishedComponentCoverage<C> {
    ChainOnly(ApplySequence),
    Exact(C),
}

impl<C: Copy + PartialEq> PublishedComponentCoverage<C> {
    fn is_exact(self, source: C) -> bool {
        self == Self::Exact(source)
    }
}

impl PublishedComponentCoverage<ProposalSourceCut> {
    fn chain_source(self) -> ApplySequence {
        match self {
            Self::ChainOnly(chain) => chain,
            Self::Exact(source) => source.chain,
        }
    }
}

impl PublishedComponentCoverage<TransactionSourceCut> {
    fn chain_source(self) -> ApplySequence {
        match self {
            Self::ChainOnly(chain) => chain,
            Self::Exact(source) => source.chain,
        }
    }
}

impl PublishedComponentCoverage<UncleSourceCut> {
    fn chain_source(self) -> ApplySequence {
        match self {
            Self::ChainOnly(chain) => chain,
            Self::Exact(source) => source.chain,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TemplateCoverage {
    proposals: Option<PublishedComponentCoverage<ProposalSourceCut>>,
    transactions: Option<PublishedComponentCoverage<TransactionSourceCut>>,
    uncles: Option<PublishedComponentCoverage<UncleSourceCut>>,
}

impl TemplateCoverage {
    fn full(sources: TemplateSourceCut) -> Self {
        Self {
            proposals: Some(PublishedComponentCoverage::Exact(sources.proposal_cut())),
            transactions: Some(PublishedComponentCoverage::Exact(sources.transaction_cut())),
            uncles: Some(PublishedComponentCoverage::Exact(sources.uncle_cut())),
        }
    }

    fn initial_base(chain: ApplySequence) -> Self {
        Self {
            proposals: Some(PublishedComponentCoverage::ChainOnly(chain)),
            transactions: Some(PublishedComponentCoverage::ChainOnly(chain)),
            uncles: Some(PublishedComponentCoverage::ChainOnly(chain)),
        }
    }

    /// The miner-facing template is chain-current only when every independently
    /// published component was constructed from the same chain cut. A reset or
    /// one partial publication therefore cannot stand in for the vector.
    fn coherent_chain_source(&self) -> Option<ApplySequence> {
        let proposals = self.proposals?.chain_source();
        let transactions = self.transactions?.chain_source();
        let uncles = self.uncles?.chain_source();
        (proposals == transactions && transactions == uncles).then_some(proposals)
    }
}

/// Move-only build receipts keep construction concurrent while publication
/// remains an exact, total state transition. Full deliberately has no
/// revision precondition: it wins over racing partial work, but its requested
/// reset epoch prevents it from crossing a reset even before blank content is
/// published. A full prepared for that exact epoch may publish after reset.
pub(super) struct FullTemplateBuild {
    expected_reset: ResetEpoch,
    expected_reset_chain: ApplySequence,
    sources: TemplateSourceCut,
    coverage: TemplateCoverage,
}

impl FullTemplateBuild {
    pub(super) fn chain_source(&self) -> ApplySequence {
        self.expected_reset_chain
    }
}

enum PartialTemplateCoverage {
    Proposals(ProposalSourceCut),
    Transactions(TransactionSourceCut),
    Uncles(UncleSourceCut),
}

pub(super) struct PartialTemplateBuild {
    expected_revision: TemplateRevision,
    coverage: PartialTemplateCoverage,
}

impl PartialTemplateBuild {
    pub(super) fn chain_source(&self) -> ApplySequence {
        match self.coverage {
            PartialTemplateCoverage::Proposals(coverage) => coverage.chain,
            PartialTemplateCoverage::Transactions(coverage) => coverage.chain,
            PartialTemplateCoverage::Uncles(coverage) => coverage.chain,
        }
    }
}

pub(super) struct ResetTemplateBuild {
    epoch: ResetEpoch,
    chain_source: ApplySequence,
}

impl ResetTemplateBuild {
    pub(super) fn epoch(&self) -> ResetEpoch {
        self.epoch
    }

    pub(super) fn chain_source(&self) -> ApplySequence {
        self.chain_source
    }
}

/// Rebuildable output authority for block-template source coverage.
///
/// `desired` is level-triggered and joins every observed source cut.
/// `covered` describes only the content currently published. A full build may
/// overwrite newer partial content and therefore may move coverage backwards;
/// the resulting inequality is the retry fact. Notify/delta delivery is only
/// a hint and cannot erase this state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TemplateConvergence {
    desired: TemplateSourceCut,
    covered: TemplateCoverage,
    desired_reset: ResetEpoch,
    desired_reset_chain: ApplySequence,
    full_required: Option<TemplateSourceCut>,
    failed_replacement_chain: Option<ApplySequence>,
}

impl TemplateConvergence {
    pub(super) fn new(initial: TemplateSourceCut, reset_epoch: ResetEpoch) -> Self {
        Self {
            desired: initial,
            // BlockAssembler::new publishes a chain-safe base template, but it
            // has not captured any exact pool/candidate component source.
            covered: TemplateCoverage::initial_base(initial.chain_source()),
            desired_reset: reset_epoch,
            desired_reset_chain: initial.chain_source(),
            full_required: Some(initial),
            failed_replacement_chain: None,
        }
    }

    pub(super) fn chain_read_state(&self, required: ApplySequence) -> TemplateChainReadState {
        if self.covered.coherent_chain_source() == Some(required) {
            TemplateChainReadState::Published
        } else if self.failed_replacement_chain == Some(required) {
            TemplateChainReadState::Failed
        } else {
            TemplateChainReadState::Pending
        }
    }

    pub(super) fn record_replacement_failure(&mut self, failed: ApplySequence) {
        self.failed_replacement_chain = Some(failed);
    }

    /// A successful replacement step retires an older terminal attempt but
    /// does not declare read readiness; only coherent component coverage does.
    fn record_replacement_progress(&mut self, published: ApplySequence) {
        if self
            .failed_replacement_chain
            .is_some_and(|failed| failed <= published)
        {
            self.failed_replacement_chain = None;
        }
    }

    /// Join rather than replace because pool and uncle cuts come from
    /// independent read authorities. Delayed or duplicate observations are
    /// harmless; an incomparable pair still has one deterministic level join.
    pub(super) fn observe_sources(&mut self, sources: TemplateSourceCut) {
        self.desired = self.desired.join(sources);
    }

    fn observe_pool_sources(&mut self, sources: TemplatePoolSourceCut) {
        self.desired.pool = self.desired.pool.join(sources);
    }

    /// O(1) gate for the ordered reset/full lane. Accepted selection changes
    /// are handled by optimistic component lanes; a full capture is necessary
    /// only for a new chain source, an unpublished reset, or an explicit
    /// component escalation. Observing the joined source here makes the level
    /// durable without turning the probe into a second template authority.
    pub(super) fn replacement_needs_capture(
        &mut self,
        sources: TemplateSourceCut,
        published_reset: ResetEpoch,
    ) -> bool {
        self.observe_sources(sources);
        self.desired.chain_source() > self.desired_reset_chain
            || self.desired_reset != published_reset
            || self.full_required.is_some()
    }

    pub(super) fn proposals_need_capture(&mut self, sources: TemplatePoolSourceCut) -> bool {
        self.observe_pool_sources(sources);
        self.is_pending(TemplateComponent::Proposals)
    }

    pub(super) fn transactions_need_capture(&mut self, sources: TemplatePoolSourceCut) -> bool {
        self.observe_pool_sources(sources);
        self.is_pending(TemplateComponent::Transactions)
    }

    pub(super) fn uncles_need_capture(&mut self, sources: TemplateSourceCut) -> bool {
        self.observe_sources(sources);
        self.is_pending(TemplateComponent::Uncles)
    }

    pub(super) fn begin_pending_full(
        &mut self,
        sources: TemplateSourceCut,
    ) -> Option<FullTemplateBuild> {
        self.observe_sources(sources);
        self.full_required.map(|_| FullTemplateBuild {
            expected_reset: self.desired_reset,
            expected_reset_chain: self.desired_reset_chain,
            sources,
            coverage: TemplateCoverage::full(sources),
        })
    }

    /// Escalate a partial build that cannot publish a definitive component
    /// under the current shared byte budget. Repeated requests coalesce at the
    /// latest source level and add no queue item.
    pub(super) fn require_full(&mut self) -> bool {
        let required = self
            .full_required
            .map_or(self.desired, |current| current.join(self.desired));
        let changed = self.full_required != Some(required);
        self.full_required = Some(required);
        changed
    }

    pub(super) fn begin_pending_proposals(
        &mut self,
        sources: TemplatePoolSourceCut,
        base_revision: TemplateRevision,
    ) -> Option<PartialTemplateBuild> {
        self.observe_pool_sources(sources);
        (!self
            .covered
            .proposals
            .is_some_and(|covered| covered.is_exact(self.desired.proposal_cut())))
        .then(|| PartialTemplateBuild {
            expected_revision: base_revision,
            coverage: PartialTemplateCoverage::Proposals(sources.proposal_cut()),
        })
    }

    pub(super) fn begin_pending_transactions(
        &mut self,
        sources: TemplatePoolSourceCut,
        base_revision: TemplateRevision,
    ) -> Option<PartialTemplateBuild> {
        self.observe_pool_sources(sources);
        (!self
            .covered
            .transactions
            .is_some_and(|covered| covered.is_exact(self.desired.transaction_cut())))
        .then(|| PartialTemplateBuild {
            expected_revision: base_revision,
            coverage: PartialTemplateCoverage::Transactions(sources.transaction_cut()),
        })
    }

    pub(super) fn begin_pending_uncles(
        &mut self,
        sources: TemplateSourceCut,
        base_revision: TemplateRevision,
    ) -> Option<PartialTemplateBuild> {
        self.observe_sources(sources);
        (!self
            .covered
            .uncles
            .is_some_and(|covered| covered.is_exact(self.desired.uncle_cut())))
        .then(|| PartialTemplateBuild {
            expected_revision: base_revision,
            coverage: PartialTemplateCoverage::Uncles(sources.uncle_cut()),
        })
    }

    pub(super) fn mark_reset(
        &mut self,
        sources: TemplateSourceCut,
    ) -> Result<ResetTemplateBuild, TemplateConvergenceError> {
        let epoch = self
            .desired_reset
            .next()
            .ok_or(TemplateConvergenceError::ResetEpochExhausted)?;
        self.observe_sources(sources);
        self.desired_reset = epoch;
        self.desired_reset_chain = sources.chain_source();
        self.require_full();
        Ok(ResetTemplateBuild {
            epoch,
            chain_source: self.desired_reset_chain,
        })
    }

    /// Return the one reset capability for the latest observed chain source.
    /// Re-reading an unchanged pending level reconstructs its token without
    /// manufacturing a new epoch; a newer chain cut supersedes the older
    /// build before either can publish.
    pub(super) fn ensure_reset(
        &mut self,
        sources: TemplateSourceCut,
        published_reset: ResetEpoch,
    ) -> Result<Option<ResetTemplateBuild>, TemplateConvergenceError> {
        let incoming_chain = sources.chain_source();
        self.observe_sources(sources);
        if self.desired_reset > published_reset && incoming_chain == self.desired_reset_chain {
            return Ok(self.begin_pending_reset(published_reset));
        }
        if incoming_chain < self.desired_reset_chain {
            return Ok(self.begin_pending_reset(published_reset));
        }
        self.mark_reset(sources).map(Some)
    }

    /// Reconstruct the exact outstanding reset capability from authoritative
    /// level state. Notification and the first move-only build are only wake
    /// hints; dropping either cannot erase a requested reset.
    pub(super) fn begin_pending_reset(
        &self,
        published_reset: ResetEpoch,
    ) -> Option<ResetTemplateBuild> {
        (self.desired_reset > published_reset).then_some(ResetTemplateBuild {
            epoch: self.desired_reset,
            chain_source: self.desired_reset_chain,
        })
    }

    pub(super) fn publish_full(
        &mut self,
        build: FullTemplateBuild,
        published_reset: ResetEpoch,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        if build.expected_reset != self.desired_reset
            || build.expected_reset != published_reset
            || build.expected_reset_chain != self.desired_reset_chain
        {
            return Ok(TemplatePublication::Stale);
        }
        let progress_chain = build.expected_reset_chain;
        self.covered = build.coverage;
        if self
            .full_required
            .is_some_and(|required| build.sources.covers(required))
        {
            self.full_required = None;
        }
        self.record_replacement_progress(progress_chain);
        Ok(TemplatePublication::Published)
    }

    pub(super) fn publish_partial(
        &mut self,
        build: PartialTemplateBuild,
        published_revision: TemplateRevision,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        if build.expected_revision != published_revision {
            return Ok(TemplatePublication::Stale);
        }
        match build.coverage {
            PartialTemplateCoverage::Proposals(coverage) => {
                self.covered.proposals = Some(PublishedComponentCoverage::Exact(coverage));
                self.covered.transactions = None;
                self.covered.uncles = None;
            }
            PartialTemplateCoverage::Transactions(coverage) => {
                self.covered.transactions = Some(PublishedComponentCoverage::Exact(coverage))
            }
            PartialTemplateCoverage::Uncles(coverage) => {
                self.covered.proposals =
                    Some(PublishedComponentCoverage::Exact(coverage.proposal_cut()));
                self.covered.transactions = None;
                self.covered.uncles = Some(PublishedComponentCoverage::Exact(coverage));
            }
        }
        Ok(TemplatePublication::Published)
    }

    pub(super) fn publish_reset(
        &mut self,
        build: ResetTemplateBuild,
        published_reset: ResetEpoch,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        if build.epoch != self.desired_reset
            || build.chain_source != self.desired_reset_chain
            || build.epoch <= published_reset
        {
            return Ok(TemplatePublication::Stale);
        }
        let progress_chain = build.chain_source;
        self.covered = TemplateCoverage::default();
        self.require_full();
        self.record_replacement_progress(progress_chain);
        Ok(TemplatePublication::Published)
    }

    pub(super) fn is_pending(&self, component: TemplateComponent) -> bool {
        match component {
            TemplateComponent::Proposals => !self
                .covered
                .proposals
                .is_some_and(|covered| covered.is_exact(self.desired.proposal_cut())),
            TemplateComponent::Transactions => !self
                .covered
                .transactions
                .is_some_and(|covered| covered.is_exact(self.desired.transaction_cut())),
            TemplateComponent::Uncles => !self
                .covered
                .uncles
                .is_some_and(|covered| covered.is_exact(self.desired.uncle_cut())),
        }
    }
}

fn causal_indices(
    entries: &[TemplateCandidate],
    by_hash: &HashMap<RawTxHash, usize>,
) -> Result<Vec<usize>, TemplateReadError> {
    let mut preference = Vec::new();
    preference
        .try_reserve(entries.len())
        .map_err(|_| TemplateReadError::Allocation)?;
    preference.extend(entries.iter().map(|entry| &entry.order));
    causal_order(
        entries.len(),
        entries.iter().map(|entry| entry.parents.as_ref()),
        entries.iter().map(|entry| &entry.hash),
        by_hash,
        Some(preference),
    )
}

fn causal_order<'entry>(
    len: usize,
    parents: impl Iterator<Item = &'entry [RawTxHash]>,
    hashes: impl Iterator<Item = &'entry RawTxHash>,
    by_hash: &HashMap<RawTxHash, usize>,
    preference: Option<Vec<&'entry AcceptedOrderKey>>,
) -> Result<Vec<usize>, TemplateReadError> {
    let mut captured_hashes = Vec::new();
    captured_hashes
        .try_reserve(len)
        .map_err(|_| TemplateReadError::Allocation)?;
    captured_hashes.extend(hashes);
    let mut captured_parents = Vec::new();
    captured_parents
        .try_reserve(len)
        .map_err(|_| TemplateReadError::Allocation)?;
    captured_parents.extend(parents);
    if captured_hashes.len() != len || captured_parents.len() != len {
        return Err(TemplateReadError::Projection);
    }
    let mut indegree = Vec::new();
    indegree
        .try_reserve(len)
        .map_err(|_| TemplateReadError::Allocation)?;
    indegree.resize(len, 0usize);
    let mut children = Vec::new();
    children
        .try_reserve(len)
        .map_err(|_| TemplateReadError::Allocation)?;
    children.resize_with(len, Vec::new);
    for (child, parent_hashes) in captured_parents.iter().enumerate() {
        for parent in *parent_hashes {
            let parent = *by_hash.get(parent).ok_or(TemplateReadError::Projection)?;
            let degree = indegree
                .get_mut(child)
                .ok_or(TemplateReadError::Projection)?;
            *degree = degree.checked_add(1).ok_or(TemplateReadError::Arithmetic)?;
            let next = children
                .get_mut(parent)
                .ok_or(TemplateReadError::Projection)?;
            next.try_reserve(1)
                .map_err(|_| TemplateReadError::Allocation)?;
            next.push(child);
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct ReadyKey {
        preference: Option<Reverse<AcceptedOrderKey>>,
        hash: RawTxHash,
        index: usize,
    }
    let ready_key = |index: usize| -> Result<ReadyKey, TemplateReadError> {
        let hash = (*captured_hashes
            .get(index)
            .ok_or(TemplateReadError::Projection)?)
        .clone();
        let preference = preference
            .as_ref()
            .map(|keys| {
                keys.get(index)
                    .cloned()
                    .cloned()
                    .map(Reverse)
                    .ok_or(TemplateReadError::Projection)
            })
            .transpose()?;
        Ok(ReadyKey {
            preference,
            hash,
            index,
        })
    };
    let mut ready = BTreeSet::new();
    for (index, degree) in indegree.iter().copied().enumerate() {
        if degree == 0 {
            ready.insert(ready_key(index)?);
        }
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve(len)
        .map_err(|_| TemplateReadError::Allocation)?;
    while let Some(next) = ready.pop_first() {
        ordered.push(next.index);
        for child in children
            .get(next.index)
            .ok_or(TemplateReadError::Projection)?
        {
            let degree = indegree
                .get_mut(*child)
                .ok_or(TemplateReadError::Projection)?;
            *degree = degree.checked_sub(1).ok_or(TemplateReadError::Projection)?;
            if *degree == 0 {
                ready.insert(ready_key(*child)?);
            }
        }
    }
    if ordered.len() != len {
        return Err(TemplateReadError::CausalCycle);
    }
    Ok(ordered)
}

#[cfg(test)]
#[path = "tests/support/template.rs"]
mod test_support;
