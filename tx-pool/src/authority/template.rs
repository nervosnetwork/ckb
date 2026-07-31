//! Immutable tx-pool input for concurrent block-template construction.
//!
//! The authority owns accepted membership and status. This module captures one
//! coherent, bounded read receipt containing only immutable `Arc` payloads and
//! payload-free derived order/relation facts. Topology and packing work run
//! after the authority guard opens; the block assembler remains a separate
//! owner of rebuildable output.

use super::{
    plan::{AcceptedOrderKey, EvictionOrderKey, MembershipProjection},
    read::AuthorityReadCut,
    source::PoolTemplateVersions,
    state::{
        AcceptedAtMillis, AcceptedStatus, ApplySequence, CandidateMetrics, OwnedTx, ProposalId,
        RawTxHash,
    },
};
use ckb_types::{
    core::cell::ResolvedTransaction,
    packed::{OutPoint, ProposalShortId},
};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

/// Maximum expanded dependency occurrences inspected while imposing the
/// tx-pool-only reader-before-spender order on one proposed selection. The
/// accepted authority already charges these occurrences as resident edges;
/// this second bound limits transient template CPU and memory.
const SELECTED_DEP_ORDERING_BUDGET: usize = 200_000;

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
    resolved: Arc<ResolvedTransaction>,
    parents: Vec<RawTxHash>,
    order: AcceptedOrderKey,
    eviction: EvictionOrderKey,
}

impl TemplateCandidate {
    pub(super) fn hash(&self) -> &RawTxHash {
        &self.hash
    }

    pub(super) fn proposal(&self) -> &ProposalId {
        &self.proposal
    }

    pub(super) fn proposal_short_id(&self) -> &ProposalShortId {
        &self.proposal.0
    }

    pub(super) fn status(&self) -> AcceptedStatus {
        self.status
    }

    pub(super) fn accepted_at(&self) -> AcceptedAtMillis {
        self.accepted_at
    }

    pub(super) fn metrics(&self) -> &CandidateMetrics {
        &self.metrics
    }

    pub(super) fn resolved(&self) -> &Arc<ResolvedTransaction> {
        &self.resolved
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
    resolved: Arc<ResolvedTransaction>,
    parents: Vec<RawTxHash>,
    order: AcceptedOrderKey,
    eviction: EvictionOrderKey,
}

#[derive(Debug)]
pub(super) struct AuthorityTemplateReadReceipt {
    cut: AuthorityReadCut,
    sources: PoolTemplateVersions,
    captured: Vec<CapturedAccepted>,
}

#[derive(Debug)]
pub(super) struct TemplateSelectionReceipt {
    cut: AuthorityReadCut,
    sources: PoolTemplateVersions,
    candidates: Vec<TemplateCandidate>,
}

impl AuthorityTemplateReadReceipt {
    pub(super) fn capture(
        cut: AuthorityReadCut,
        sources: PoolTemplateVersions,
        entries: &HashMap<RawTxHash, OwnedTx>,
        membership: &MembershipProjection,
    ) -> Result<Self, TemplateReadError> {
        let counts = membership.counts();
        let accepted_count = counts
            .pending
            .checked_add(counts.gap)
            .and_then(|count| count.checked_add(counts.proposed))
            .ok_or(TemplateReadError::Arithmetic)?;
        let mut captured = Vec::new();
        captured
            .try_reserve(accepted_count)
            .map_err(|_| TemplateReadError::Allocation)?;
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
            let eviction = membership
                .eviction_order_for(hash, entry)
                .ok_or(TemplateReadError::Projection)?;
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
                resolved: Arc::clone(entry.proof.payload().resolved_transaction()),
                parents,
                order: order.clone(),
                eviction,
            });
        }
        if captured.len() != accepted_count {
            return Err(TemplateReadError::Projection);
        }

        Ok(Self {
            cut,
            sources,
            captured,
        })
    }

    pub(super) fn cut(&self) -> &AuthorityReadCut {
        &self.cut
    }

    pub(super) fn source_cut(&self, uncles: CandidateUncleVersion) -> TemplateSourceCut {
        TemplateSourceCut::new(self.sources, uncles)
    }

    pub(super) fn selected_len(&self) -> usize {
        self.captured.len()
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
                resolved: entry.resolved,
                parents: entry.parents,
                order: entry.order,
                eviction: entry.eviction,
            });
        }
        Ok(TemplateSelectionReceipt {
            cut: self.cut,
            sources: self.sources,
            candidates,
        })
    }
}

impl TemplateSelectionReceipt {
    pub(super) fn cut(&self) -> &AuthorityReadCut {
        &self.cut
    }

    pub(super) fn source_cut(&self, uncles: CandidateUncleVersion) -> TemplateSourceCut {
        TemplateSourceCut::new(self.sources, uncles)
    }

    pub(super) fn candidates(&self) -> &[TemplateCandidate] {
        &self.candidates
    }

    pub(super) fn proposals(&self, limit: usize) -> Result<Vec<ProposalId>, TemplateReadError> {
        let ordered = self.ordered_indices([AcceptedStatus::Pending])?;
        let selected = limit.min(ordered.len());
        let mut proposals = Vec::new();
        proposals
            .try_reserve(selected)
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in ordered.into_iter().take(selected) {
            let candidate = self
                .candidates
                .get(index)
                .ok_or(TemplateReadError::Projection)?;
            proposals.push(candidate.proposal.clone());
        }
        Ok(proposals)
    }

    /// The query rank and proposal selector consume the same immutable key.
    /// Gap remains RPC-pending but is intentionally absent from `proposals`.
    pub(super) fn pending_rank(
        &self,
        hash: &RawTxHash,
    ) -> Result<Option<usize>, TemplateReadError> {
        let mut position = None;
        for (rank, index) in self
            .ordered_indices([AcceptedStatus::Pending, AcceptedStatus::Gap])?
            .into_iter()
            .enumerate()
        {
            let candidate = self
                .candidates
                .get(index)
                .ok_or(TemplateReadError::Projection)?;
            if &candidate.hash == hash {
                position = Some(rank);
                break;
            }
        }
        position
            .map(|rank| rank.checked_add(1).ok_or(TemplateReadError::Arithmetic))
            .transpose()
    }

    /// Return a deterministic, consensus-safe subset of causally eligible
    /// Proposed transactions. The persistent authority stores only causal
    /// producer edges; this pure receipt consumer adds selected-set
    /// `dependency reader -> spender` edges, sheds bounded conditional cycles,
    /// and never writes a second graph back into membership.
    ///
    /// Packing limits and CPFP package iteration are a later pure consumer of
    /// this same receipt. They may call the same selected-set ordering kernel
    /// on a smaller subset; no authority state is read in either case.
    pub(super) fn proposed_parent_first(
        &self,
    ) -> Result<Vec<&TemplateCandidate>, TemplateReadError> {
        self.proposed_parent_first_with_dependency_budget(SELECTED_DEP_ORDERING_BUDGET)
    }

    #[cfg(test)]
    pub(super) fn proposed_parent_first_for_foundation(
        &self,
        dependency_budget: usize,
    ) -> Result<Vec<&TemplateCandidate>, TemplateReadError> {
        self.proposed_parent_first_with_dependency_budget(dependency_budget)
    }

    fn proposed_parent_first_with_dependency_budget(
        &self,
        dependency_budget: usize,
    ) -> Result<Vec<&TemplateCandidate>, TemplateReadError> {
        let mut by_hash = HashMap::new();
        by_hash
            .try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for (index, candidate) in self.candidates.iter().enumerate() {
            if by_hash.insert(candidate.hash.clone(), index).is_some() {
                return Err(TemplateReadError::Projection);
            }
        }

        let mut eligible = Vec::new();
        eligible
            .try_reserve(self.candidates.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        eligible.resize(self.candidates.len(), false);
        let causal = causal_indices(&self.candidates, &by_hash)?;
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

        let mut causally_selected = Vec::new();
        causally_selected
            .try_reserve(causal.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in causal {
            if eligible
                .get(index)
                .copied()
                .ok_or(TemplateReadError::Projection)?
            {
                causally_selected.push(index);
            }
        }
        let selected =
            self.order_conditionally_safe(causally_selected, &by_hash, dependency_budget)?;
        let mut ordered = Vec::new();
        ordered
            .try_reserve(selected.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in selected {
            ordered.push(
                self.candidates
                    .get(index)
                    .ok_or(TemplateReadError::Projection)?,
            );
        }
        Ok(ordered)
    }

    fn order_conditionally_safe(
        &self,
        selected: Vec<usize>,
        by_hash: &HashMap<RawTxHash, usize>,
        dependency_budget: usize,
    ) -> Result<Vec<usize>, TemplateReadError> {
        let selected = self.retain_with_dependency_budget(selected, by_hash, dependency_budget)?;
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
            let graph = self.conditional_graph(&active, by_hash, dependency_budget)?;
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

    fn retain_with_dependency_budget(
        &self,
        selected: Vec<usize>,
        by_hash: &HashMap<RawTxHash, usize>,
        dependency_budget: usize,
    ) -> Result<Vec<usize>, TemplateReadError> {
        let mut remaining = dependency_budget;
        let mut dropped = HashSet::new();
        dropped
            .try_reserve(selected.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        let mut retained = Vec::new();
        retained
            .try_reserve(selected.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in selected {
            let candidate = self
                .candidates
                .get(index)
                .ok_or(TemplateReadError::Projection)?;
            let causal_parent_dropped = candidate.parents.iter().any(|parent| {
                by_hash
                    .get(parent)
                    .is_some_and(|parent_index| dropped.contains(parent_index))
            });
            if causal_parent_dropped {
                dropped.insert(index);
                continue;
            }
            if remaining == 0 {
                if candidate.resolved.related_dep_out_points().next().is_some() {
                    dropped.insert(index);
                } else {
                    retained.push(index);
                }
                continue;
            }
            let inspected = candidate
                .resolved
                .related_dep_out_points()
                .take(remaining.saturating_add(1))
                .count();
            if inspected > remaining {
                remaining = 0;
                dropped.insert(index);
            } else {
                remaining = remaining
                    .checked_sub(inspected)
                    .ok_or(TemplateReadError::Projection)?;
                retained.push(index);
            }
        }
        Ok(retained)
    }

    fn conditional_graph(
        &self,
        active: &[bool],
        by_hash: &HashMap<RawTxHash, usize>,
        dependency_budget: usize,
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
                .checked_add(candidate.resolved.related_dep_out_points().count())
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
        if dependency_count > dependency_budget {
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
            for dependency in candidate.resolved.related_dep_out_points() {
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

/// Version of the block assembler's bounded candidate-uncle authority.
///
/// Candidate uncles are not transaction-pool ownership, so their version is
/// captured separately and joined with a coherent authority receipt only for
/// pure template construction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct CandidateUncleVersion(u64);

impl CandidateUncleVersion {
    pub(super) const INITIAL: Self = Self(0);

    pub(super) fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Complete level input for template convergence. Pool versions are captured
/// with accepted payloads under one authority read guard; the uncle version
/// comes from the block assembler's independent bounded candidate authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplateSourceCut {
    pool: PoolTemplateVersions,
    uncles: CandidateUncleVersion,
}

impl TemplateSourceCut {
    fn new(pool: PoolTemplateVersions, uncles: CandidateUncleVersion) -> Self {
        Self { pool, uncles }
    }

    fn join(self, incoming: Self) -> Self {
        Self {
            pool: PoolTemplateVersions {
                proposals: self.pool.proposals.max(incoming.pool.proposals),
                transactions: self.pool.transactions.max(incoming.pool.transactions),
                chain: self.pool.chain.max(incoming.pool.chain),
            },
            uncles: self.uncles.max(incoming.uncles),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProposalSourceCut {
    selection: ApplySequence,
    chain: ApplySequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TransactionSourceCut {
    selection: ApplySequence,
    chain: ApplySequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UncleSourceCut {
    candidates: CandidateUncleVersion,
    chain: ApplySequence,
    proposals: ApplySequence,
}

impl TemplateSourceCut {
    fn proposal_cut(self) -> ProposalSourceCut {
        ProposalSourceCut {
            selection: self.pool.proposals,
            chain: self.pool.chain,
        }
    }

    fn transaction_cut(self) -> TransactionSourceCut {
        TransactionSourceCut {
            selection: self.pool.transactions,
            chain: self.pool.chain,
        }
    }

    fn uncle_cut(self) -> UncleSourceCut {
        UncleSourceCut {
            candidates: self.uncles,
            chain: self.pool.chain,
            proposals: self.pool.proposals,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateComponent {
    Proposals,
    Transactions,
    Uncles,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TemplateRevision(u64);

impl TemplateRevision {
    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct TemplateResetEpoch(u64);

impl TemplateResetEpoch {
    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplatePublication {
    Published,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateConvergenceError {
    RevisionExhausted,
    ResetEpochExhausted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TemplateCoverage {
    proposals: Option<ProposalSourceCut>,
    transactions: Option<TransactionSourceCut>,
    uncles: Option<UncleSourceCut>,
}

impl TemplateCoverage {
    fn full(sources: TemplateSourceCut) -> Self {
        Self {
            proposals: Some(sources.proposal_cut()),
            transactions: Some(sources.transaction_cut()),
            uncles: Some(sources.uncle_cut()),
        }
    }
}

/// Move-only build receipts keep construction concurrent while publication
/// remains an exact, total state transition. Full deliberately has no
/// revision precondition: it wins over racing partial work, but its requested
/// reset epoch prevents it from crossing a reset even before blank content is
/// published. A full prepared for that exact epoch may publish after reset.
pub(super) struct FullTemplateBuild {
    expected_reset: TemplateResetEpoch,
    coverage: TemplateCoverage,
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

pub(super) struct ResetTemplateBuild {
    epoch: TemplateResetEpoch,
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
    revision: TemplateRevision,
    desired_reset: TemplateResetEpoch,
    published_reset: TemplateResetEpoch,
}

impl TemplateConvergence {
    pub(super) fn new(initial: TemplateSourceCut) -> Self {
        Self {
            desired: initial,
            covered: TemplateCoverage::default(),
            revision: TemplateRevision::default(),
            desired_reset: TemplateResetEpoch::default(),
            published_reset: TemplateResetEpoch::default(),
        }
    }

    /// Join rather than replace because pool and uncle cuts come from
    /// independent read authorities. Delayed or duplicate observations are
    /// harmless; an incomparable pair still has one deterministic level join.
    pub(super) fn observe_sources(&mut self, sources: TemplateSourceCut) {
        self.desired = self.desired.join(sources);
    }

    pub(super) fn begin_full(&mut self, sources: TemplateSourceCut) -> FullTemplateBuild {
        self.observe_sources(sources);
        FullTemplateBuild {
            // `desired_reset`, not `published_reset`, is the serialization
            // fence. Otherwise scheduler timing lets an old full cross the
            // interval between reset request and blank publication.
            expected_reset: self.desired_reset,
            coverage: TemplateCoverage::full(sources),
        }
    }

    pub(super) fn begin_partial(
        &mut self,
        component: TemplateComponent,
        sources: TemplateSourceCut,
    ) -> PartialTemplateBuild {
        self.observe_sources(sources);
        let coverage = match component {
            TemplateComponent::Proposals => {
                PartialTemplateCoverage::Proposals(sources.proposal_cut())
            }
            TemplateComponent::Transactions => {
                PartialTemplateCoverage::Transactions(sources.transaction_cut())
            }
            TemplateComponent::Uncles => PartialTemplateCoverage::Uncles(sources.uncle_cut()),
        };
        PartialTemplateBuild {
            expected_revision: self.revision,
            coverage,
        }
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
        Ok(ResetTemplateBuild { epoch })
    }

    /// Reconstruct the exact outstanding reset capability from authoritative
    /// level state. Notification and the first move-only build are only wake
    /// hints; dropping either cannot erase a requested reset.
    pub(super) fn begin_pending_reset(&self) -> Option<ResetTemplateBuild> {
        (self.desired_reset > self.published_reset).then_some(ResetTemplateBuild {
            epoch: self.desired_reset,
        })
    }

    pub(super) fn publish_full(
        &mut self,
        build: FullTemplateBuild,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        if build.expected_reset != self.desired_reset
            || build.expected_reset != self.published_reset
        {
            return Ok(TemplatePublication::Stale);
        }
        let revision = self
            .revision
            .next()
            .ok_or(TemplateConvergenceError::RevisionExhausted)?;
        self.covered = build.coverage;
        self.revision = revision;
        Ok(TemplatePublication::Published)
    }

    pub(super) fn publish_partial(
        &mut self,
        build: PartialTemplateBuild,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        if build.expected_revision != self.revision {
            return Ok(TemplatePublication::Stale);
        }
        let revision = self
            .revision
            .next()
            .ok_or(TemplateConvergenceError::RevisionExhausted)?;
        match build.coverage {
            PartialTemplateCoverage::Proposals(coverage) => self.covered.proposals = Some(coverage),
            PartialTemplateCoverage::Transactions(coverage) => {
                self.covered.transactions = Some(coverage)
            }
            PartialTemplateCoverage::Uncles(coverage) => self.covered.uncles = Some(coverage),
        }
        self.revision = revision;
        Ok(TemplatePublication::Published)
    }

    pub(super) fn publish_reset(
        &mut self,
        build: ResetTemplateBuild,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        if build.epoch != self.desired_reset || build.epoch <= self.published_reset {
            return Ok(TemplatePublication::Stale);
        }
        let revision = self
            .revision
            .next()
            .ok_or(TemplateConvergenceError::RevisionExhausted)?;
        self.covered = TemplateCoverage::default();
        self.published_reset = build.epoch;
        self.revision = revision;
        Ok(TemplatePublication::Published)
    }

    pub(super) fn is_pending(&self, component: TemplateComponent) -> bool {
        match component {
            TemplateComponent::Proposals => {
                self.covered.proposals != Some(self.desired.proposal_cut())
            }
            TemplateComponent::Transactions => {
                self.covered.transactions != Some(self.desired.transaction_cut())
            }
            TemplateComponent::Uncles => self.covered.uncles != Some(self.desired.uncle_cut()),
        }
    }

    pub(super) fn is_converged(&self) -> bool {
        self.desired_reset == self.published_reset
            && [
                TemplateComponent::Proposals,
                TemplateComponent::Transactions,
                TemplateComponent::Uncles,
            ]
            .into_iter()
            .all(|component| !self.is_pending(component))
    }

    #[cfg(test)]
    pub(super) fn force_revision_for_foundation(&mut self, revision: u64) {
        self.revision = TemplateRevision(revision);
    }

    #[cfg(test)]
    pub(super) fn force_reset_epoch_for_foundation(&mut self, epoch: u64) {
        self.desired_reset = TemplateResetEpoch(epoch);
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
