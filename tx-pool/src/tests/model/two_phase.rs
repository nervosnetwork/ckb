//! End-to-end two-phase proposal, commit and finite-progress semantics.
//!
//! The immutable `ProposalView` is the only protocol cut used here. A block
//! is legal exactly when every committed proposal id is Proposed. Template
//! selection refines that relation by additionally requiring the complete
//! in-pool causal parent closure. The liveness machine keeps only primitive
//! proposal history and derives every phase through `ProposalContext`.

use super::{
    boundaries::{CandidateUncleInput, filter_uncles_conflicting_with_proposals},
    eviction_quotient::{EvictionRefinementMetrics, transaction_weight},
    proposal::{
        ProposalBlock, ProposalContext, ProposalContextError, ProposalView, ProposalWindow,
        ProposalWindowPosition,
    },
    state::{AcceptedStatus, InputOrigin, Omega, Owner, OwnerLocation, ProposalId, TxId},
};
use std::{
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU16,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CausalCandidate {
    proposal: ProposalId,
    parents: BTreeSet<ProposalId>,
    order: u16,
}

impl CausalCandidate {
    pub(super) fn new(proposal: ProposalId, parents: BTreeSet<ProposalId>, order: u16) -> Self {
        Self {
            proposal,
            parents,
            order,
        }
    }
}

/// Primitive coordinates of production's Accepted package-order key.
///
/// The identity is the deterministic final tie-break.  It deliberately keeps
/// raw bytes rather than importing a production hash type, so production
/// refinement must project the exact identity while the finite executable
/// model can use its injective `TxId` encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AcceptedOrderInput {
    own: EvictionRefinementMetrics,
    ancestors: EvictionRefinementMetrics,
    arrival: u128,
    identity: [u8; 32],
}

impl AcceptedOrderInput {
    pub(crate) const fn new(
        own: EvictionRefinementMetrics,
        ancestors: EvictionRefinementMetrics,
        arrival: u128,
        identity: [u8; 32],
    ) -> Self {
        Self {
            own,
            ancestors,
            arrival,
            identity,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModelAncestorsScoreKey {
    own: EvictionRefinementMetrics,
    ancestors: EvictionRefinementMetrics,
}

impl ModelAncestorsScoreKey {
    fn minimum_fee_and_weight(self) -> (u64, u64) {
        let own_weight = transaction_weight(self.own);
        let ancestors_weight = transaction_weight(self.ancestors);
        let own_cross = u128::from(self.own.fee) * u128::from(ancestors_weight);
        let ancestors_cross = u128::from(self.ancestors.fee) * u128::from(own_weight);
        if own_cross < ancestors_cross {
            (self.own.fee, own_weight)
        } else {
            (self.ancestors.fee, ancestors_weight)
        }
    }
}

impl Ord for ModelAncestorsScoreKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let (fee, weight) = self.minimum_fee_and_weight();
        let (other_fee, other_weight) = other.minimum_fee_and_weight();
        let left = u128::from(fee) * u128::from(other_weight);
        let right = u128::from(other_fee) * u128::from(weight);
        left.cmp(&right).then_with(|| {
            transaction_weight(self.ancestors).cmp(&transaction_weight(other.ancestors))
        })
    }
}

impl PartialOrd for ModelAncestorsScoreKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModelAcceptedOrderKey {
    score: ModelAncestorsScoreKey,
    arrival: u128,
    identity: [u8; 32],
}

impl ModelAcceptedOrderKey {
    const fn new(input: AcceptedOrderInput) -> Self {
        Self {
            score: ModelAncestorsScoreKey {
                own: input.own,
                ancestors: input.ancestors,
            },
            arrival: input.arrival,
            identity: input.identity,
        }
    }
}

impl Ord for ModelAcceptedOrderKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.identity.cmp(&self.identity))
    }
}

impl PartialOrd for ModelAcceptedOrderKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Strongest-first Accepted order, independently reconstructed from primitive
/// package coordinates.  Duplicate identities are rejected instead of using
/// an index as a second authority.
pub(crate) fn accepted_order_refinement(inputs: &[AcceptedOrderInput]) -> Option<Vec<usize>> {
    let identities = inputs
        .iter()
        .map(|input| input.identity)
        .collect::<BTreeSet<_>>();
    if identities.len() != inputs.len() {
        return None;
    }
    let mut ordered = Vec::new();
    ordered.try_reserve(inputs.len()).ok()?;
    ordered.extend(
        inputs
            .iter()
            .copied()
            .enumerate()
            .map(|(index, input)| (ModelAcceptedOrderKey::new(input), index)),
    );
    ordered.sort_unstable_by_key(|(key, _)| Reverse(*key));
    Some(ordered.into_iter().map(|(_, index)| index).collect())
}

/// Primitive transaction coordinates consumed by the current package
/// packer. Ancestor aggregates are deliberately absent: this model derives
/// them from the causal graph so a stale production aggregate cannot make
/// both sides agree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplatePackingInput {
    own: EvictionRefinementMetrics,
    causal_parents: BTreeSet<usize>,
    proposed: bool,
    arrival: u128,
    identity: [u8; 32],
}

impl TemplatePackingInput {
    pub(crate) const fn new(
        own: EvictionRefinementMetrics,
        causal_parents: BTreeSet<usize>,
        proposed: bool,
        arrival: u128,
        identity: [u8; 32],
    ) -> Self {
        Self {
            own,
            causal_parents,
            proposed,
            arrival,
            identity,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelTemplatePackingLimits {
    serialized_bytes: u64,
    cycles: u64,
}

impl ModelTemplatePackingLimits {
    pub(crate) const fn new(serialized_bytes: u64, cycles: u64) -> Self {
        Self {
            serialized_bytes,
            cycles,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplatePackingObservation {
    pub(crate) selected: Vec<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplatePackingError {
    InvalidGraph,
    CausalCycle,
    DuplicateIdentity,
    Arithmetic,
}

/// Frozen current-production work bounds. Source validation binds both to the
/// independently declared production constants.
pub(crate) const TEMPLATE_PACKING_FAILURE_BOUND: usize = 4_000;
pub(crate) const TEMPLATE_DESCENDANT_CACHE_MEMBER_BOUND: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModelPackageAggregate {
    entries: usize,
    metrics: EvictionRefinementMetrics,
}

impl ModelPackageAggregate {
    const fn one(metrics: EvictionRefinementMetrics) -> Self {
        Self {
            entries: 1,
            metrics,
        }
    }

    fn checked_add(self, incoming: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(incoming.entries)?,
            metrics: EvictionRefinementMetrics::new(
                self.metrics.fee.checked_add(incoming.metrics.fee)?,
                self.metrics
                    .serialized_bytes
                    .checked_add(incoming.metrics.serialized_bytes)?,
                self.metrics.cycles.checked_add(incoming.metrics.cycles)?,
            ),
        })
    }

    fn checked_sub(self, removed: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_sub(removed.entries)?,
            metrics: EvictionRefinementMetrics::new(
                self.metrics.fee.checked_sub(removed.metrics.fee)?,
                self.metrics
                    .serialized_bytes
                    .checked_sub(removed.metrics.serialized_bytes)?,
                self.metrics.cycles.checked_sub(removed.metrics.cycles)?,
            ),
        })
    }

    fn fits(self, limits: ModelTemplatePackingLimits) -> bool {
        self.metrics.serialized_bytes <= limits.serialized_bytes
            && self.metrics.cycles <= limits.cycles
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelPackageOrderKey {
    score: ModelAncestorsScoreKey,
    arrival: u128,
    identity: [u8; 32],
    index: usize,
}

impl ModelPackageOrderKey {
    const fn new(
        index: usize,
        input: &TemplatePackingInput,
        aggregate: ModelPackageAggregate,
    ) -> Self {
        Self {
            score: ModelAncestorsScoreKey {
                own: input.own,
                ancestors: aggregate.metrics,
            },
            arrival: input.arrival,
            identity: input.identity,
            index,
        }
    }
}

impl Ord for ModelPackageOrderKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.arrival.cmp(&self.arrival))
            .then_with(|| other.identity.cmp(&self.identity))
            .then_with(|| other.index.cmp(&self.index))
    }
}

impl PartialOrd for ModelPackageOrderKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModelPackingState {
    Ineligible,
    Queued,
    Examining,
    Failed,
    Selected,
}

/// Independent finite reconstruction of production's causal CPFP packer.
///
/// The result is the selected causal package sequence before the separately
/// modelled dependency-budget and reader-before-spender compiler. The model
/// preserves the current two-dimensional fit test, dynamic descendant
/// rescoring and consecutive-failure work bound without importing production
/// queue or aggregate code.
pub(crate) fn template_packing_refinement(
    inputs: &[TemplatePackingInput],
    limits: ModelTemplatePackingLimits,
    max_consecutive_failures: usize,
) -> Result<TemplatePackingObservation, TemplatePackingError> {
    let len = inputs.len();
    let identities = inputs
        .iter()
        .map(|input| input.identity)
        .collect::<BTreeSet<_>>();
    if identities.len() != len {
        return Err(TemplatePackingError::DuplicateIdentity);
    }

    let mut children = vec![BTreeSet::new(); len];
    let mut indegree = vec![0usize; len];
    for (child, input) in inputs.iter().enumerate() {
        for parent in &input.causal_parents {
            if *parent >= len || *parent == child {
                return Err(TemplatePackingError::InvalidGraph);
            }
            if children[*parent].insert(child) {
                indegree[child] = indegree[child]
                    .checked_add(1)
                    .ok_or(TemplatePackingError::Arithmetic)?;
            }
        }
    }

    let mut ready = BTreeSet::new();
    for (index, degree) in indegree.iter().copied().enumerate() {
        if degree == 0 {
            ready.insert((
                Reverse(ModelAcceptedOrderKey::new(AcceptedOrderInput {
                    own: inputs[index].own,
                    ancestors: inputs[index].own,
                    arrival: inputs[index].arrival,
                    identity: inputs[index].identity,
                })),
                index,
            ));
        }
    }

    // Derive complete ancestor closures first. The ready-key below is replaced
    // once an aggregate exists; roots already have own == aggregate.
    let mut causal = Vec::with_capacity(len);
    let mut ancestor_sets = vec![BTreeSet::new(); len];
    let mut aggregates = vec![None; len];
    while let Some((_key, index)) = ready.pop_first() {
        let mut ancestors = BTreeSet::from([index]);
        for parent in &inputs[index].causal_parents {
            ancestors.extend(ancestor_sets[*parent].iter().copied());
        }
        let aggregate = ancestors.iter().try_fold(
            ModelPackageAggregate {
                entries: 0,
                metrics: EvictionRefinementMetrics::new(0, 0, 0),
            },
            |total, member| total.checked_add(ModelPackageAggregate::one(inputs[*member].own)),
        );
        let aggregate = aggregate.ok_or(TemplatePackingError::Arithmetic)?;
        ancestor_sets[index] = ancestors;
        aggregates[index] = Some(aggregate);
        causal.push(index);

        for child in &children[index] {
            indegree[*child] = indegree[*child]
                .checked_sub(1)
                .ok_or(TemplatePackingError::InvalidGraph)?;
            if indegree[*child] == 0 {
                // All parent aggregates now exist, so the exact accepted-order
                // preference can be reconstructed before choosing the next
                // causally-ready member.
                let mut child_ancestors = BTreeSet::from([*child]);
                for parent in &inputs[*child].causal_parents {
                    child_ancestors.extend(ancestor_sets[*parent].iter().copied());
                }
                let child_aggregate = child_ancestors.iter().try_fold(
                    ModelPackageAggregate {
                        entries: 0,
                        metrics: EvictionRefinementMetrics::new(0, 0, 0),
                    },
                    |total, member| {
                        total.checked_add(ModelPackageAggregate::one(inputs[*member].own))
                    },
                );
                let child_aggregate = child_aggregate.ok_or(TemplatePackingError::Arithmetic)?;
                ready.insert((
                    Reverse(ModelAcceptedOrderKey::new(AcceptedOrderInput {
                        own: inputs[*child].own,
                        ancestors: child_aggregate.metrics,
                        arrival: inputs[*child].arrival,
                        identity: inputs[*child].identity,
                    })),
                    *child,
                ));
            }
        }
    }
    if causal.len() != len {
        return Err(TemplatePackingError::CausalCycle);
    }

    let mut eligible = vec![false; len];
    let mut causal_rank = vec![None; len];
    for (rank, index) in causal.iter().copied().enumerate() {
        causal_rank[index] = Some(rank);
        eligible[index] = inputs[index].proposed
            && inputs[index]
                .causal_parents
                .iter()
                .all(|parent| eligible[*parent]);
    }

    let mut states = vec![ModelPackingState::Ineligible; len];
    let mut queue = BTreeSet::new();
    for index in causal.iter().copied().filter(|index| eligible[*index]) {
        let aggregate = aggregates[index].ok_or(TemplatePackingError::InvalidGraph)?;
        if aggregate.fits(limits) {
            states[index] = ModelPackingState::Queued;
            queue.insert(ModelPackageOrderKey::new(index, &inputs[index], aggregate));
        }
    }

    let mut selected = Vec::new();
    let mut selected_bytes = 0u64;
    let mut selected_cycles = 0u64;
    let mut consecutive_failures = 0usize;
    while let Some(key) = queue.pop_last() {
        let index = key.index;
        if states[index] != ModelPackingState::Queued
            || key
                != ModelPackageOrderKey::new(
                    index,
                    &inputs[index],
                    aggregates[index].ok_or(TemplatePackingError::InvalidGraph)?,
                )
        {
            return Err(TemplatePackingError::InvalidGraph);
        }
        states[index] = ModelPackingState::Examining;
        let aggregate = aggregates[index].ok_or(TemplatePackingError::InvalidGraph)?;
        let projected_bytes = selected_bytes
            .checked_add(aggregate.metrics.serialized_bytes)
            .ok_or(TemplatePackingError::Arithmetic)?;
        let projected_cycles = selected_cycles
            .checked_add(aggregate.metrics.cycles)
            .ok_or(TemplatePackingError::Arithmetic)?;
        if projected_bytes > limits.serialized_bytes || projected_cycles > limits.cycles {
            states[index] = ModelPackingState::Failed;
            consecutive_failures = consecutive_failures
                .checked_add(1)
                .ok_or(TemplatePackingError::Arithmetic)?;
            if consecutive_failures > max_consecutive_failures {
                break;
            }
            continue;
        }

        let mut package = ancestor_sets[index]
            .iter()
            .copied()
            .filter(|member| states[*member] != ModelPackingState::Selected)
            .collect::<Vec<_>>();
        package.sort_unstable_by_key(|member| causal_rank[*member]);
        let package_aggregate = package.iter().try_fold(
            ModelPackageAggregate {
                entries: 0,
                metrics: EvictionRefinementMetrics::new(0, 0, 0),
            },
            |total, member| total.checked_add(ModelPackageAggregate::one(inputs[*member].own)),
        );
        if package_aggregate != Some(aggregate) {
            return Err(TemplatePackingError::InvalidGraph);
        }

        for member in package.iter().copied() {
            match states[member] {
                ModelPackingState::Queued => {
                    let member_aggregate =
                        aggregates[member].ok_or(TemplatePackingError::InvalidGraph)?;
                    if !queue.remove(&ModelPackageOrderKey::new(
                        member,
                        &inputs[member],
                        member_aggregate,
                    )) {
                        return Err(TemplatePackingError::InvalidGraph);
                    }
                }
                ModelPackingState::Examining | ModelPackingState::Failed => {}
                ModelPackingState::Ineligible | ModelPackingState::Selected => {
                    return Err(TemplatePackingError::InvalidGraph);
                }
            }
            states[member] = ModelPackingState::Selected;
            selected.push(member);
        }

        for descendant in 0..len {
            if states[descendant] != ModelPackingState::Queued {
                continue;
            }
            let mut removed = ModelPackageAggregate {
                entries: 0,
                metrics: EvictionRefinementMetrics::new(0, 0, 0),
            };
            for member in package.iter().copied() {
                if descendant != member && ancestor_sets[descendant].contains(&member) {
                    removed = removed
                        .checked_add(ModelPackageAggregate::one(inputs[member].own))
                        .ok_or(TemplatePackingError::Arithmetic)?;
                }
            }
            if removed.entries == 0 {
                continue;
            }
            let previous = aggregates[descendant].ok_or(TemplatePackingError::InvalidGraph)?;
            if !queue.remove(&ModelPackageOrderKey::new(
                descendant,
                &inputs[descendant],
                previous,
            )) {
                return Err(TemplatePackingError::InvalidGraph);
            }
            let remaining = previous
                .checked_sub(removed)
                .ok_or(TemplatePackingError::InvalidGraph)?;
            aggregates[descendant] = Some(remaining);
            queue.insert(ModelPackageOrderKey::new(
                descendant,
                &inputs[descendant],
                remaining,
            ));
        }

        selected_bytes = projected_bytes;
        selected_cycles = projected_cycles;
        consecutive_failures = 0;
    }

    Ok(TemplatePackingObservation { selected })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DependencyScanInput {
    causal_parents: BTreeSet<usize>,
    dependency_edges: usize,
}

impl DependencyScanInput {
    pub(crate) const fn new(causal_parents: BTreeSet<usize>, dependency_edges: usize) -> Self {
        Self {
            causal_parents,
            dependency_edges,
        }
    }

    #[cfg(test)]
    pub(crate) const fn dependency_edges_for_foundation(&self) -> usize {
        self.dependency_edges
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DependencyScanObservation {
    pub(crate) retained: Vec<usize>,
    pub(crate) inspected_dependency_edges: usize,
}

/// Exact complete-scan quotient. The captured immutable receipt freezes an
/// exact upper bound over every deduplicated dependency edge. Selection scans
/// only a subset of that finite domain, so resource accounting can never
/// become a semantic rule that drops an otherwise selected transaction.
pub(crate) fn complete_dependency_scan_refinement(
    inputs: &[DependencyScanInput],
    selected: &[usize],
    captured_edge_bound: usize,
) -> Result<DependencyScanObservation, TemplatePackingError> {
    let mut seen = BTreeSet::new();
    if selected
        .iter()
        .any(|index| *index >= inputs.len() || !seen.insert(*index))
    {
        return Err(TemplatePackingError::InvalidGraph);
    }
    if inputs.iter().enumerate().any(|(child, input)| {
        input
            .causal_parents
            .iter()
            .any(|parent| *parent >= inputs.len() || *parent == child)
    }) {
        return Err(TemplatePackingError::InvalidGraph);
    }

    let positions = selected
        .iter()
        .copied()
        .enumerate()
        .map(|(position, index)| (index, position))
        .collect::<BTreeMap<_, _>>();
    if selected.iter().copied().any(|child| {
        inputs[child].causal_parents.iter().any(|parent| {
            positions
                .get(parent)
                .zip(positions.get(&child))
                .is_none_or(|(parent_position, child_position)| parent_position >= child_position)
        })
    }) {
        return Err(TemplatePackingError::InvalidGraph);
    }

    let exact_bound = inputs.iter().try_fold(0usize, |total, input| {
        total.checked_add(input.dependency_edges)
    });
    if exact_bound != Some(captured_edge_bound) {
        return Err(TemplatePackingError::InvalidGraph);
    }
    let inspected_dependency_edges = selected.iter().try_fold(0usize, |total, index| {
        total.checked_add(inputs[*index].dependency_edges)
    });
    let Some(inspected_dependency_edges) = inspected_dependency_edges else {
        return Err(TemplatePackingError::Arithmetic);
    };
    if inspected_dependency_edges > captured_edge_bound {
        return Err(TemplatePackingError::InvalidGraph);
    }
    Ok(DependencyScanObservation {
        retained: selected.to_vec(),
        inspected_dependency_edges,
    })
}

/// Parser-free observation of production's optional-content byte order.
///
/// The current assembler first fits the strongest proposal prefix, then the
/// ordered compatible-uncle prefix, and only afterwards exposes the remaining
/// bytes to transaction packing.  In particular, positive proposal capacity
/// does not imply positive commit-package capacity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CurrentTemplateCapacityObservation {
    pub(crate) proposals: usize,
    pub(crate) uncles: usize,
    pub(crate) commit_package_fits: bool,
    pub(crate) remaining_transaction_bytes: usize,
}

pub(crate) fn current_template_capacity_refinement(
    proposal_count: usize,
    max_block_proposals: usize,
    proposal_id_bytes: usize,
    compatible_uncle_bytes: &[usize],
    base_bytes: usize,
    commit_package_bytes: usize,
    max_block_bytes: usize,
) -> Option<CurrentTemplateCapacityObservation> {
    let available = max_block_bytes.checked_sub(base_bytes)?;
    if proposal_id_bytes == 0 {
        return None;
    }
    let proposals = proposal_count
        .min(max_block_proposals)
        .min(available.checked_div(proposal_id_bytes)?);
    let proposal_bytes = proposals.checked_mul(proposal_id_bytes)?;
    let mut used = base_bytes.checked_add(proposal_bytes)?;
    let mut uncles = 0usize;
    for uncle_bytes in compatible_uncle_bytes {
        let next = used.checked_add(*uncle_bytes)?;
        if next > max_block_bytes {
            break;
        }
        used = next;
        uncles = uncles.checked_add(1)?;
    }
    Some(CurrentTemplateCapacityObservation {
        proposals,
        uncles,
        commit_package_fits: used
            .checked_add(commit_package_bytes)
            .is_some_and(|total| total <= max_block_bytes),
        remaining_transaction_bytes: max_block_bytes.checked_sub(used)?,
    })
}

/// Primitive graph coordinates consumed by production's bounded
/// reader-before-spender cycle compiler.
///
/// `priority` is the position in the already packed Accepted order;
/// `eviction_order` is only the total weak-to-strong order.  The latter is an
/// intentional quotient: the independent eviction model owns how the concrete
/// package score is derived, while this relation owns only how that total order
/// breaks a conditional cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConditionalSelectionInput {
    causal_parents: BTreeSet<usize>,
    conditional_predecessors: BTreeSet<usize>,
    priority: usize,
    eviction_order: usize,
}

impl ConditionalSelectionInput {
    pub(crate) fn new(
        causal_parents: BTreeSet<usize>,
        conditional_predecessors: BTreeSet<usize>,
        priority: usize,
        eviction_order: usize,
    ) -> Self {
        Self {
            causal_parents,
            conditional_predecessors,
            priority,
            eviction_order,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConditionalSelectionObservation {
    pub(crate) ordered: Vec<usize>,
    pub(crate) shed: BTreeSet<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConditionalSelectionError {
    InvalidGraph,
    DuplicatePriority,
    DuplicateEvictionOrder,
    Arithmetic,
}

/// Frozen production bound.  The model derives the transition independently;
/// source validation binds this coordinate to the production constant.
pub(crate) const CONDITIONAL_CYCLE_ROUND_BOUND: usize = 64;

/// Exact finite relation for production's deterministic conditional-cycle
/// shedding.
///
/// Each cyclic SCC loses its weakest feedback vertex and that vertex's causal
/// descendants.  Once the round bound is exceeded, the strongest member of
/// each remaining SCC survives and every other member is shed.  This is not a
/// liveness theorem for the full Accepted cut: it is the explicit compiler
/// that produces the conditionally acyclic cohort to which the two-phase
/// liveness theorem may be applied.
pub(crate) fn conditional_template_selection_refinement(
    inputs: &[ConditionalSelectionInput],
) -> Result<ConditionalSelectionObservation, ConditionalSelectionError> {
    let len = inputs.len();
    let priorities = inputs
        .iter()
        .map(|input| input.priority)
        .collect::<BTreeSet<_>>();
    if priorities.len() != len {
        return Err(ConditionalSelectionError::DuplicatePriority);
    }
    let eviction_orders = inputs
        .iter()
        .map(|input| input.eviction_order)
        .collect::<BTreeSet<_>>();
    if eviction_orders.len() != len {
        return Err(ConditionalSelectionError::DuplicateEvictionOrder);
    }

    let mut children = vec![BTreeSet::new(); len];
    let mut causal_children = vec![BTreeSet::new(); len];
    for (child, input) in inputs.iter().enumerate() {
        for parent in &input.causal_parents {
            if *parent >= len || *parent == child {
                return Err(ConditionalSelectionError::InvalidGraph);
            }
            children[*parent].insert(child);
            causal_children[*parent].insert(child);
        }
        for predecessor in &input.conditional_predecessors {
            if *predecessor >= len || *predecessor == child {
                return Err(ConditionalSelectionError::InvalidGraph);
            }
            children[*predecessor].insert(child);
        }
    }

    let mut active = vec![true; len];
    let mut cycle_round = 0usize;
    loop {
        let ordered = conditional_topological_order(inputs, &active, &children)?;
        let active_count = active.iter().filter(|is_active| **is_active).count();
        if ordered.len() == active_count {
            return Ok(ConditionalSelectionObservation {
                ordered,
                shed: active
                    .iter()
                    .enumerate()
                    .filter_map(|(index, is_active)| (!is_active).then_some(index))
                    .collect(),
            });
        }

        let cyclic = conditional_cyclic_components(&active, &children)?;
        if cyclic.is_empty() {
            return Err(ConditionalSelectionError::InvalidGraph);
        }
        cycle_round = cycle_round
            .checked_add(1)
            .ok_or(ConditionalSelectionError::Arithmetic)?;
        let bounded_fallback = cycle_round > CONDITIONAL_CYCLE_ROUND_BOUND;
        let mut roots = BTreeSet::new();
        for component in cyclic {
            let strongest = component
                .iter()
                .copied()
                .max_by_key(|index| inputs[*index].eviction_order)
                .ok_or(ConditionalSelectionError::InvalidGraph)?;
            if bounded_fallback {
                roots.extend(component.into_iter().filter(|index| *index != strongest));
            } else {
                let weakest = component
                    .iter()
                    .copied()
                    .min_by_key(|index| inputs[*index].eviction_order)
                    .ok_or(ConditionalSelectionError::InvalidGraph)?;
                roots.insert(weakest);
            }
        }
        drop_model_causal_descendants(&mut active, roots, &causal_children)?;
        if active.iter().filter(|is_active| **is_active).count() < 2 {
            let mut ordered = active
                .iter()
                .enumerate()
                .filter_map(|(index, is_active)| is_active.then_some(index))
                .collect::<Vec<_>>();
            ordered.sort_unstable_by_key(|index| inputs[*index].priority);
            return Ok(ConditionalSelectionObservation {
                ordered,
                shed: active
                    .iter()
                    .enumerate()
                    .filter_map(|(index, is_active)| (!is_active).then_some(index))
                    .collect(),
            });
        }
    }
}

fn conditional_topological_order(
    inputs: &[ConditionalSelectionInput],
    active: &[bool],
    children: &[BTreeSet<usize>],
) -> Result<Vec<usize>, ConditionalSelectionError> {
    if inputs.len() != active.len() || inputs.len() != children.len() {
        return Err(ConditionalSelectionError::InvalidGraph);
    }
    let mut indegree = vec![0usize; inputs.len()];
    for (parent, next) in children.iter().enumerate() {
        if !active[parent] {
            continue;
        }
        for child in next {
            if !active.get(*child).copied().unwrap_or(false) {
                continue;
            }
            indegree[*child] = indegree[*child]
                .checked_add(1)
                .ok_or(ConditionalSelectionError::Arithmetic)?;
        }
    }
    let mut ready = active
        .iter()
        .enumerate()
        .filter_map(|(index, is_active)| {
            (*is_active && indegree[index] == 0).then_some((inputs[index].priority, index))
        })
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::with_capacity(active.iter().filter(|is_active| **is_active).count());
    while let Some((_priority, index)) = ready.pop_first() {
        ordered.push(index);
        for child in &children[index] {
            if !active[*child] {
                continue;
            }
            indegree[*child] = indegree[*child]
                .checked_sub(1)
                .ok_or(ConditionalSelectionError::InvalidGraph)?;
            if indegree[*child] == 0 {
                ready.insert((inputs[*child].priority, *child));
            }
        }
    }
    Ok(ordered)
}

fn conditional_cyclic_components(
    active: &[bool],
    children: &[BTreeSet<usize>],
) -> Result<Vec<Vec<usize>>, ConditionalSelectionError> {
    if active.len() != children.len() {
        return Err(ConditionalSelectionError::InvalidGraph);
    }
    let len = active.len();
    let mut reach = vec![vec![false; len]; len];
    for (parent, next) in children.iter().enumerate() {
        if !active[parent] {
            continue;
        }
        reach[parent][parent] = true;
        for child in next {
            if *child >= len || !active[*child] {
                continue;
            }
            reach[parent][*child] = true;
        }
    }
    for intermediate in 0..len {
        if !active[intermediate] {
            continue;
        }
        for source in 0..len {
            if !active[source] || !reach[source][intermediate] {
                continue;
            }
            for target in 0..len {
                if active[target] && reach[intermediate][target] {
                    reach[source][target] = true;
                }
            }
        }
    }
    let mut assigned = vec![false; len];
    let mut cyclic = Vec::new();
    for source in 0..len {
        if !active[source] || assigned[source] {
            continue;
        }
        let component = (0..len)
            .filter(|target| active[*target] && reach[source][*target] && reach[*target][source])
            .collect::<Vec<_>>();
        for member in &component {
            assigned[*member] = true;
        }
        if component.len() > 1 {
            cyclic.push(component);
        }
    }
    Ok(cyclic)
}

fn drop_model_causal_descendants(
    active: &mut [bool],
    roots: BTreeSet<usize>,
    causal_children: &[BTreeSet<usize>],
) -> Result<(), ConditionalSelectionError> {
    let mut frontier = roots.into_iter().collect::<Vec<_>>();
    while let Some(index) = frontier.pop() {
        let Some(is_active) = active.get_mut(index) else {
            return Err(ConditionalSelectionError::InvalidGraph);
        };
        if !*is_active {
            continue;
        }
        *is_active = false;
        frontier.extend(
            causal_children
                .get(index)
                .ok_or(ConditionalSelectionError::InvalidGraph)?
                .iter()
                .copied(),
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct UnproposedCommit {
    proposal: ProposalId,
}

impl UnproposedCommit {
    pub(super) const fn proposal(self) -> ProposalId {
        self.proposal
    }
}

/// The model consumes its admitted proposal projection. Equality with the
/// production consensus verifier is owned by the real-verifier/live-view
/// bridge in `verification/contextual`; this function is not a second
/// primitive-history walk or consensus oracle.
pub(super) fn verify_candidate_block(
    context: &ProposalContext,
    committed: &BTreeSet<ProposalId>,
) -> Result<(), TwoPhaseLivenessError> {
    let view = context
        .verified_view()
        .map_err(TwoPhaseLivenessError::Context)?;
    match committed
        .iter()
        .find(|proposal| view.position(**proposal) != ProposalWindowPosition::Proposed)
    {
        Some(proposal) => Err(TwoPhaseLivenessError::UnsafeCommit(UnproposedCommit {
            proposal: *proposal,
        })),
        None => Ok(()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CausalSelectionError {
    DuplicateCandidate(ProposalId),
    MissingParent {
        candidate: ProposalId,
        parent: ProposalId,
    },
    CausalCycle,
    Arithmetic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StableAcceptedCohortError {
    InvalidAuthority,
    MissingAcceptedParent { child: ProposalId, parent: TxId },
    OrderOverflow,
    Causal(CausalSelectionError),
    ConditionalTemplateCycle,
}

/// Sealed projection of one stable Accepted authority cut.
///
/// The liveness theorem is deliberately unavailable for retained Proposal
/// owners: production may expire a trusted Proposal owner or demote a remote
/// one before acceptance. Accepted membership supplies the proposal-identity
/// bijection and the complete in-pool causal parent relation used by template
/// selection. The future-stability assumption remains an explicit environment
/// premise; this type only prevents the theorem from starting from the wrong
/// lifecycle location or a structurally invalid graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct StableAcceptedCohort {
    priority: Vec<ProposalId>,
    parents: BTreeMap<ProposalId, BTreeSet<ProposalId>>,
    conditional_predecessors: BTreeMap<ProposalId, BTreeSet<ProposalId>>,
}

impl StableAcceptedCohort {
    pub(super) fn from_authority(authority: &Omega) -> Result<Self, StableAcceptedCohortError> {
        authority
            .check_invariants()
            .map_err(|_| StableAcceptedCohortError::InvalidAuthority)?;

        let proposal_by_tx = authority
            .authority
            .owners
            .iter()
            .filter_map(|(transaction, owner)| {
                matches!(owner.location, OwnerLocation::Accepted { .. })
                    .then_some((*transaction, owner.transaction.proposal))
            })
            .collect::<BTreeMap<_, _>>();
        let accepted = authority
            .authority
            .owners
            .iter()
            .filter_map(|(transaction, owner)| {
                matches!(owner.location, OwnerLocation::Accepted { .. })
                    .then_some((*transaction, owner))
            })
            .collect::<Vec<_>>();
        let accepted_by_tx = accepted.iter().copied().collect::<BTreeMap<_, _>>();
        let mut parents_by_tx = BTreeMap::new();
        let mut spenders = BTreeMap::new();
        for (transaction, owner) in &accepted {
            let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                return Err(StableAcceptedCohortError::InvalidAuthority);
            };
            let parents = evidence
                .input_origins
                .values()
                .chain(evidence.dep_origins.values())
                .filter_map(|origin| match origin {
                    InputOrigin::Pool(parent) => Some(*parent),
                    InputOrigin::Chain => None,
                })
                .collect::<BTreeSet<_>>();
            if parents
                .iter()
                .any(|parent| !accepted_by_tx.contains_key(parent))
            {
                let parent = *parents
                    .iter()
                    .find(|parent| !accepted_by_tx.contains_key(parent))
                    .ok_or(StableAcceptedCohortError::InvalidAuthority)?;
                return Err(StableAcceptedCohortError::MissingAcceptedParent {
                    child: owner.transaction.proposal,
                    parent,
                });
            }
            parents_by_tx.insert(*transaction, parents);
            for cell in &owner.transaction.inputs {
                if spenders.insert(*cell, *transaction).is_some() {
                    return Err(StableAcceptedCohortError::InvalidAuthority);
                }
            }
        }
        let mut conditional_predecessors = BTreeMap::<ProposalId, BTreeSet<ProposalId>>::new();
        for (reader, owner) in &accepted {
            let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                return Err(StableAcceptedCohortError::InvalidAuthority);
            };
            for cell in evidence.conditional_reads() {
                let Some(spender) = spenders.get(&cell) else {
                    continue;
                };
                if spender == reader {
                    continue;
                }
                let spender_proposal = proposal_by_tx
                    .get(spender)
                    .copied()
                    .ok_or(StableAcceptedCohortError::InvalidAuthority)?;
                conditional_predecessors
                    .entry(spender_proposal)
                    .or_default()
                    .insert(owner.transaction.proposal);
            }
        }

        let mut order_inputs = Vec::new();
        order_inputs
            .try_reserve(accepted.len())
            .map_err(|_| StableAcceptedCohortError::OrderOverflow)?;
        for (transaction, owner) in &accepted {
            let ancestors =
                accepted_ancestor_metrics(*transaction, &accepted_by_tx, &parents_by_tx)?;
            let mut identity = [0; 32];
            identity[31] = transaction.0;
            order_inputs.push(AcceptedOrderInput::new(
                transaction_metrics(&owner.transaction),
                ancestors,
                u128::from(owner.arrival.0),
                identity,
            ));
        }
        let priority_indices = accepted_order_refinement(&order_inputs)
            .ok_or(StableAcceptedCohortError::InvalidAuthority)?;

        let mut candidates = Vec::new();
        candidates
            .try_reserve(accepted.len())
            .map_err(|_| StableAcceptedCohortError::OrderOverflow)?;
        for (order, index) in priority_indices.into_iter().enumerate() {
            let (transaction, owner) = *accepted
                .get(index)
                .ok_or(StableAcceptedCohortError::InvalidAuthority)?;
            let parents = parents_by_tx
                .get(&transaction)
                .ok_or(StableAcceptedCohortError::InvalidAuthority)?
                .iter()
                .map(|parent| {
                    proposal_by_tx.get(parent).copied().ok_or(
                        StableAcceptedCohortError::MissingAcceptedParent {
                            child: owner.transaction.proposal,
                            parent: *parent,
                        },
                    )
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let order =
                u16::try_from(order).map_err(|_| StableAcceptedCohortError::OrderOverflow)?;
            candidates.push(CausalCandidate::new(
                owner.transaction.proposal,
                parents,
                order,
            ));
            if proposal_by_tx.get(&transaction) != Some(&owner.transaction.proposal) {
                return Err(StableAcceptedCohortError::InvalidAuthority);
            }
        }
        Self::from_candidates(candidates, conditional_predecessors)
    }

    fn from_candidates(
        mut candidates: Vec<CausalCandidate>,
        conditional_predecessors: BTreeMap<ProposalId, BTreeSet<ProposalId>>,
    ) -> Result<Self, StableAcceptedCohortError> {
        let proposed = candidates
            .iter()
            .map(|candidate| candidate.proposal)
            .collect::<BTreeSet<_>>();
        if proposed.len() != candidates.len() {
            let duplicate = candidates
                .iter()
                .map(|candidate| candidate.proposal)
                .find(|proposal| {
                    candidates
                        .iter()
                        .filter(|candidate| candidate.proposal == *proposal)
                        .count()
                        > 1
                })
                .ok_or(StableAcceptedCohortError::InvalidAuthority)?;
            return Err(StableAcceptedCohortError::Causal(
                CausalSelectionError::DuplicateCandidate(duplicate),
            ));
        }
        let view = ProposalContext::status_witness(proposed, BTreeSet::new())
            .map_err(|_| StableAcceptedCohortError::InvalidAuthority)?
            .view();
        causally_eligible(&view, &candidates).map_err(StableAcceptedCohortError::Causal)?;
        let causal_parents = candidates
            .iter()
            .map(|candidate| (candidate.proposal, candidate.parents.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut template_candidates = candidates.clone();
        for candidate in &mut template_candidates {
            candidate.parents.extend(
                conditional_predecessors
                    .get(&candidate.proposal)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }
        causally_eligible(&view, &template_candidates).map_err(|error| {
            if error == CausalSelectionError::CausalCycle {
                StableAcceptedCohortError::ConditionalTemplateCycle
            } else {
                StableAcceptedCohortError::Causal(error)
            }
        })?;
        candidates.sort_unstable_by_key(|candidate| (candidate.order, candidate.proposal));
        let priority = candidates
            .iter()
            .map(|candidate| candidate.proposal)
            .collect();
        Ok(Self {
            priority,
            parents: causal_parents,
            conditional_predecessors,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.priority.len()
    }
}

fn transaction_metrics(transaction: &super::state::Transaction) -> EvictionRefinementMetrics {
    EvictionRefinementMetrics::new(
        transaction.cost.fee(),
        u64::from(transaction.cost.serialized_bytes()),
        transaction.cost.cycles(),
    )
}

fn accepted_ancestor_metrics(
    transaction: TxId,
    accepted: &BTreeMap<TxId, &Owner>,
    parents: &BTreeMap<TxId, BTreeSet<TxId>>,
) -> Result<EvictionRefinementMetrics, StableAcceptedCohortError> {
    let mut closure = BTreeSet::new();
    let mut frontier = vec![transaction];
    while let Some(next) = frontier.pop() {
        if !closure.insert(next) {
            continue;
        }
        frontier.extend(
            parents
                .get(&next)
                .ok_or(StableAcceptedCohortError::InvalidAuthority)?
                .iter()
                .copied(),
        );
    }
    let mut fee = 0u64;
    let mut serialized_bytes = 0u64;
    let mut cycles = 0u64;
    for member in closure {
        let owner = accepted
            .get(&member)
            .ok_or(StableAcceptedCohortError::InvalidAuthority)?;
        let metrics = transaction_metrics(&owner.transaction);
        fee = fee
            .checked_add(metrics.fee)
            .ok_or(StableAcceptedCohortError::OrderOverflow)?;
        serialized_bytes = serialized_bytes
            .checked_add(metrics.serialized_bytes)
            .ok_or(StableAcceptedCohortError::OrderOverflow)?;
        cycles = cycles
            .checked_add(metrics.cycles)
            .ok_or(StableAcceptedCohortError::OrderOverflow)?;
    }
    Ok(EvictionRefinementMetrics::new(
        fee,
        serialized_bytes,
        cycles,
    ))
}

/// Compile the exact causally eligible template set in deterministic
/// parent-first order. The total `order` is an admissible abstraction of the
/// production accepted-order key; proposal id is the deterministic tie-break.
pub(super) fn causally_eligible(
    view: &ProposalView,
    candidates: &[CausalCandidate],
) -> Result<Vec<ProposalId>, CausalSelectionError> {
    let mut by_proposal = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if by_proposal.insert(candidate.proposal, index).is_some() {
            return Err(CausalSelectionError::DuplicateCandidate(candidate.proposal));
        }
    }

    let mut indegree = vec![0usize; candidates.len()];
    let mut children = vec![Vec::new(); candidates.len()];
    for (index, candidate) in candidates.iter().enumerate() {
        for parent in &candidate.parents {
            let Some(parent_index) = by_proposal.get(parent).copied() else {
                return Err(CausalSelectionError::MissingParent {
                    candidate: candidate.proposal,
                    parent: *parent,
                });
            };
            indegree[index] = indegree[index]
                .checked_add(1)
                .ok_or(CausalSelectionError::Arithmetic)?;
            children[parent_index].push(index);
        }
    }

    let mut ready = BTreeSet::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if indegree[index] == 0 {
            ready.insert((candidate.order, candidate.proposal, index));
        }
    }

    let mut eligible = vec![false; candidates.len()];
    let mut selected = Vec::new();
    selected
        .try_reserve(candidates.len())
        .map_err(|_| CausalSelectionError::Arithmetic)?;
    let mut visited = 0usize;
    while let Some((_, _, index)) = ready.pop_first() {
        visited = visited
            .checked_add(1)
            .ok_or(CausalSelectionError::Arithmetic)?;
        let candidate = &candidates[index];
        let parents_eligible = candidate.parents.iter().all(|parent| {
            by_proposal
                .get(parent)
                .and_then(|parent_index| eligible.get(*parent_index))
                .copied()
                .unwrap_or(false)
        });
        eligible[index] = view.position(candidate.proposal) == ProposalWindowPosition::Proposed
            && parents_eligible;
        if eligible[index] {
            selected.push(candidate.proposal);
        }

        for child in children[index].iter().copied() {
            indegree[child] = indegree[child]
                .checked_sub(1)
                .ok_or(CausalSelectionError::Arithmetic)?;
            if indegree[child] == 0 {
                let candidate = &candidates[child];
                ready.insert((candidate.order, candidate.proposal, child));
            }
        }
    }
    if visited != candidates.len() {
        return Err(CausalSelectionError::CausalCycle);
    }
    Ok(selected)
}

/// Test-only exact-grain bridge used by production refinement tests. Status
/// digits are Pending=0, Gap=1 and Proposed=2; candidate indices form a
/// bijection, so the bridge loses neither membership nor causal edges.
pub(crate) fn causal_membership_refinement(
    statuses: &[u8],
    parents: &[BTreeSet<usize>],
) -> Option<BTreeSet<usize>> {
    if statuses.len() != parents.len() || statuses.len() > usize::from(u8::MAX) + 1 {
        return None;
    }
    let mut proposed = BTreeSet::new();
    let mut gap = BTreeSet::new();
    for (index, status) in statuses.iter().copied().enumerate() {
        let proposal = ProposalId(index as u8);
        match status {
            0 => {}
            1 => {
                gap.insert(proposal);
            }
            2 => {
                proposed.insert(proposal);
            }
            _ => return None,
        }
    }
    let view = ProposalContext::status_witness(proposed, gap).ok()?.view();
    let mut candidates = Vec::new();
    candidates.try_reserve(parents.len()).ok()?;
    for (index, parent_indices) in parents.iter().enumerate() {
        if parent_indices.iter().any(|parent| *parent >= parents.len()) {
            return None;
        }
        candidates.push(CausalCandidate::new(
            ProposalId(index as u8),
            parent_indices
                .iter()
                .map(|parent| ProposalId(*parent as u8))
                .collect(),
            index as u16,
        ));
    }
    causally_eligible(&view, &candidates).ok().map(|selected| {
        selected
            .into_iter()
            .map(|proposal| usize::from(proposal.0))
            .collect()
    })
}

/// Primitive candidate coordinates captured by one template-service source
/// cut.  No aggregate, selected prefix or liveness capacity is accepted from a
/// caller: all of those are derived below from this one value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateServiceCandidate {
    proposal: ProposalId,
    status: AcceptedStatus,
    own: EvictionRefinementMetrics,
    causal_parents: BTreeSet<usize>,
    conditional_predecessors: BTreeSet<usize>,
    dependency_edges: usize,
    eviction_order: usize,
    arrival: u128,
    identity: [u8; 32],
}

impl TemplateServiceCandidate {
    #[allow(clippy::too_many_arguments)]
    pub(super) const fn new(
        proposal: ProposalId,
        status: AcceptedStatus,
        own: EvictionRefinementMetrics,
        causal_parents: BTreeSet<usize>,
        conditional_predecessors: BTreeSet<usize>,
        dependency_edges: usize,
        eviction_order: usize,
        arrival: u128,
        identity: [u8; 32],
    ) -> Self {
        Self {
            proposal,
            status,
            own,
            causal_parents,
            conditional_predecessors,
            dependency_edges,
            eviction_order,
            arrival,
            identity,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn from_primitive(
        proposal: u8,
        status: AcceptedStatus,
        own: EvictionRefinementMetrics,
        causal_parents: BTreeSet<usize>,
        conditional_predecessors: BTreeSet<usize>,
        dependency_edges: usize,
        eviction_order: usize,
        arrival: u128,
        identity: [u8; 32],
    ) -> Self {
        Self::new(
            ProposalId(proposal),
            status,
            own,
            causal_parents,
            conditional_predecessors,
            dependency_edges,
            eviction_order,
            arrival,
            identity,
        )
    }
}

/// Exact optional-content and block-resource coordinates used by one template
/// compilation.  Compatible uncles are already in production order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CurrentTemplateComposition {
    max_block_proposals: usize,
    proposal_id_bytes: usize,
    candidate_uncles: Vec<CandidateUncleInput>,
    base_bytes: usize,
    max_block_bytes: usize,
    max_block_cycles: u64,
}

impl CurrentTemplateComposition {
    pub(crate) fn new(
        max_block_proposals: usize,
        proposal_id_bytes: usize,
        candidate_uncles: Vec<CandidateUncleInput>,
        base_bytes: usize,
        max_block_bytes: usize,
        max_block_cycles: u64,
    ) -> Self {
        Self {
            max_block_proposals,
            proposal_id_bytes,
            candidate_uncles,
            base_bytes,
            max_block_bytes,
            max_block_cycles,
        }
    }
}

/// One immutable input cut for the complete local-service composition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateServiceSourceCut {
    candidates: Vec<TemplateServiceCandidate>,
    composition: CurrentTemplateComposition,
    captured_dependency_edge_bound: usize,
}

impl TemplateServiceSourceCut {
    pub(crate) fn new(
        candidates: Vec<TemplateServiceCandidate>,
        composition: CurrentTemplateComposition,
        captured_dependency_edge_bound: usize,
    ) -> Self {
        Self {
            candidates,
            composition,
            captured_dependency_edge_bound,
        }
    }

    /// Project one conditionally acyclic Accepted model-authority cut into the
    /// primitive local-service coordinates.  Cyclic cuts are rejected by
    /// `StableAcceptedCohort::from_authority`; callers that exercise cycle
    /// shedding must instead provide the full primitive cut to `new`, so the
    /// compiler output, not a preselected cohort, owns the theorem boundary.
    pub(super) fn from_accepted_authority(
        authority: &Omega,
        composition: CurrentTemplateComposition,
    ) -> Result<Self, TemplateServicePremiseError> {
        let cohort = StableAcceptedCohort::from_authority(authority)
            .map_err(TemplateServicePremiseError::Cohort)?;
        let proposal_positions = cohort
            .priority
            .iter()
            .copied()
            .enumerate()
            .map(|(position, proposal)| (proposal, position))
            .collect::<BTreeMap<_, _>>();
        let mut accepted = BTreeMap::new();
        for (transaction, owner) in &authority.authority.owners {
            if !matches!(owner.location, OwnerLocation::Accepted { .. }) {
                continue;
            }
            if accepted
                .insert(owner.transaction.proposal, (*transaction, owner))
                .is_some()
            {
                return Err(TemplateServicePremiseError::InvalidSourceCut);
            }
        }
        if accepted.len() != cohort.len() {
            return Err(TemplateServicePremiseError::InvalidSourceCut);
        }

        let mut captured_dependency_edge_bound = 0usize;
        let mut candidates = Vec::new();
        candidates
            .try_reserve(cohort.len())
            .map_err(|_| TemplateServicePremiseError::InvalidSourceCut)?;
        for (source_order, proposal) in cohort.priority.iter().copied().enumerate() {
            let (transaction, owner) = accepted
                .get(&proposal)
                .copied()
                .ok_or(TemplateServicePremiseError::InvalidSourceCut)?;
            let OwnerLocation::Accepted {
                evidence,
                proposal: status,
                ..
            } = &owner.location
            else {
                return Err(TemplateServicePremiseError::InvalidSourceCut);
            };
            let causal_parents = cohort
                .parents
                .get(&proposal)
                .ok_or(TemplateServicePremiseError::InvalidSourceCut)?
                .iter()
                .map(|parent| {
                    proposal_positions
                        .get(parent)
                        .copied()
                        .ok_or(TemplateServicePremiseError::InvalidSourceCut)
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let conditional_predecessors = cohort
                .conditional_predecessors
                .get(&proposal)
                .into_iter()
                .flatten()
                .map(|predecessor| {
                    proposal_positions
                        .get(predecessor)
                        .copied()
                        .ok_or(TemplateServicePremiseError::InvalidSourceCut)
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let dependency_edges = evidence.dep_origins.len();
            captured_dependency_edge_bound = captured_dependency_edge_bound
                .checked_add(dependency_edges)
                .ok_or(TemplateServicePremiseError::InvalidSourceCut)?;
            let mut identity = [0; 32];
            identity[31] = transaction.0;
            candidates.push(TemplateServiceCandidate::new(
                proposal,
                status.value(),
                transaction_metrics(&owner.transaction),
                causal_parents,
                conditional_predecessors,
                dependency_edges,
                // The exact eviction relation is observationally irrelevant
                // after the authority cut has independently proved acyclic.
                source_order,
                u128::from(owner.arrival.0),
                identity,
            ));
        }
        Ok(Self::new(
            candidates,
            composition,
            captured_dependency_edge_bound,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateServicePremiseError {
    InvalidSourceCut,
    NoProposalCapacity,
    NoCommitCapacity,
    Packing(TemplatePackingError),
    Conditional(ConditionalSelectionError),
    Cohort(StableAcceptedCohortError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TemplateServiceCompilation {
    proposal_source_indices: Vec<usize>,
    compatible_uncles: Vec<CandidateUncleInput>,
    packed_source_indices: Vec<usize>,
    retained_source_indices: Vec<usize>,
    shed_source_indices: BTreeSet<usize>,
}

fn compile_template_service_cut(
    source: &TemplateServiceSourceCut,
    positions: &[ProposalWindowPosition],
    satisfied: &[bool],
) -> Result<TemplateServiceCompilation, TemplateServicePremiseError> {
    if positions.len() != source.candidates.len() || satisfied.len() != source.candidates.len() {
        return Err(TemplateServicePremiseError::InvalidSourceCut);
    }
    let pending_source_indices = positions
        .iter()
        .zip(satisfied)
        .enumerate()
        .filter_map(|(index, (position, satisfied))| {
            (!*satisfied && *position == ProposalWindowPosition::Outside).then_some(index)
        })
        .collect::<Vec<_>>();
    let proposal_capacity = current_template_capacity_refinement(
        pending_source_indices.len(),
        source.composition.max_block_proposals,
        source.composition.proposal_id_bytes,
        &[],
        source.composition.base_bytes,
        0,
        source.composition.max_block_bytes,
    )
    .ok_or(TemplateServicePremiseError::NoProposalCapacity)?;
    let proposal_source_indices = pending_source_indices
        .into_iter()
        .take(proposal_capacity.proposals)
        .collect::<Vec<_>>();
    let proposal_ids = proposal_source_indices
        .iter()
        .map(|index| source.candidates[*index].proposal)
        .collect::<BTreeSet<_>>();
    let compatible_uncles = filter_uncles_conflicting_with_proposals(
        source.composition.candidate_uncles.iter().cloned(),
        &proposal_ids,
    );
    let compatible_uncle_bytes = compatible_uncles
        .iter()
        .map(|uncle| uncle.serialized_bytes)
        .collect::<Vec<_>>();
    let capacity = current_template_capacity_refinement(
        proposal_source_indices.len(),
        source.composition.max_block_proposals,
        source.composition.proposal_id_bytes,
        &compatible_uncle_bytes,
        source.composition.base_bytes,
        0,
        source.composition.max_block_bytes,
    )
    .ok_or(TemplateServicePremiseError::NoProposalCapacity)?;
    if capacity.proposals != proposal_source_indices.len() {
        return Err(TemplateServicePremiseError::InvalidSourceCut);
    }
    let compatible_uncles = compatible_uncles
        .into_iter()
        .take(capacity.uncles)
        .collect::<Vec<_>>();
    let serialized_bytes = u64::try_from(capacity.remaining_transaction_bytes)
        .map_err(|_| TemplateServicePremiseError::InvalidSourceCut)?;
    let active_parents = source
        .candidates
        .iter()
        .map(|candidate| {
            candidate
                .causal_parents
                .iter()
                .copied()
                .filter(|parent| !satisfied.get(*parent).copied().unwrap_or(false))
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let packing_inputs = source
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            TemplatePackingInput::new(
                candidate.own,
                active_parents[index].clone(),
                !satisfied[index] && positions[index] == ProposalWindowPosition::Proposed,
                candidate.arrival,
                candidate.identity,
            )
        })
        .collect::<Vec<_>>();
    let packing = template_packing_refinement(
        &packing_inputs,
        ModelTemplatePackingLimits::new(serialized_bytes, source.composition.max_block_cycles),
        TEMPLATE_PACKING_FAILURE_BOUND,
    )
    .map_err(TemplateServicePremiseError::Packing)?;
    let dependency_inputs = source
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            DependencyScanInput::new(active_parents[index].clone(), candidate.dependency_edges)
        })
        .collect::<Vec<_>>();
    let dependency = complete_dependency_scan_refinement(
        &dependency_inputs,
        &packing.selected,
        source.captured_dependency_edge_bound,
    )
    .map_err(TemplateServicePremiseError::Packing)?;
    if dependency.retained != packing.selected {
        return Err(TemplateServicePremiseError::InvalidSourceCut);
    }

    let selected_positions = packing
        .selected
        .iter()
        .copied()
        .enumerate()
        .map(|(position, index)| (index, position))
        .collect::<BTreeMap<_, _>>();
    let mut conditional_inputs = Vec::new();
    conditional_inputs
        .try_reserve(packing.selected.len())
        .map_err(|_| TemplateServicePremiseError::InvalidSourceCut)?;
    for (priority, source_index) in packing.selected.iter().copied().enumerate() {
        let candidate = source
            .candidates
            .get(source_index)
            .ok_or(TemplateServicePremiseError::InvalidSourceCut)?;
        let causal_parents = active_parents[source_index]
            .iter()
            .map(|parent| {
                selected_positions
                    .get(parent)
                    .copied()
                    .ok_or(TemplateServicePremiseError::InvalidSourceCut)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let conditional_predecessors = candidate
            .conditional_predecessors
            .iter()
            .filter_map(|predecessor| selected_positions.get(predecessor).copied())
            .collect();
        conditional_inputs.push(ConditionalSelectionInput::new(
            causal_parents,
            conditional_predecessors,
            priority,
            candidate.eviction_order,
        ));
    }
    let conditional = conditional_template_selection_refinement(&conditional_inputs)
        .map_err(TemplateServicePremiseError::Conditional)?;
    let retained_source_indices = conditional
        .ordered
        .iter()
        .map(|index| {
            packing
                .selected
                .get(*index)
                .copied()
                .ok_or(TemplateServicePremiseError::InvalidSourceCut)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let shed_source_indices = conditional
        .shed
        .iter()
        .map(|index| {
            packing
                .selected
                .get(*index)
                .copied()
                .ok_or(TemplateServicePremiseError::InvalidSourceCut)
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(TemplateServiceCompilation {
        proposal_source_indices,
        compatible_uncles,
        packed_source_indices: packing.selected,
        retained_source_indices,
        shed_source_indices,
    })
}

/// Sealed liveness premise produced by one exact local-template compilation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TemplateServicePremise {
    source: TemplateServiceSourceCut,
    cohort: StableAcceptedCohort,
    packed_source_indices: Vec<usize>,
    retained_source_indices: Vec<usize>,
    shed_source_indices: BTreeSet<usize>,
}

impl TemplateServicePremise {
    pub(super) fn compile(
        source: TemplateServiceSourceCut,
    ) -> Result<Self, TemplateServicePremiseError> {
        if source.candidates.is_empty() {
            return Err(TemplateServicePremiseError::NoProposalCapacity);
        }
        let proposals = source
            .candidates
            .iter()
            .map(|candidate| candidate.proposal)
            .collect::<BTreeSet<_>>();
        let identities = source
            .candidates
            .iter()
            .map(|candidate| candidate.identity)
            .collect::<BTreeSet<_>>();
        let eviction_orders = source
            .candidates
            .iter()
            .map(|candidate| candidate.eviction_order)
            .collect::<BTreeSet<_>>();
        if proposals.len() != source.candidates.len()
            || identities.len() != source.candidates.len()
            || eviction_orders.len() != source.candidates.len()
            || source
                .composition
                .candidate_uncles
                .iter()
                .map(|uncle| uncle.id)
                .collect::<BTreeSet<_>>()
                .len()
                != source.composition.candidate_uncles.len()
        {
            return Err(TemplateServicePremiseError::InvalidSourceCut);
        }

        let proposal_capacity = current_template_capacity_refinement(
            1,
            source.composition.max_block_proposals,
            source.composition.proposal_id_bytes,
            &[],
            source.composition.base_bytes,
            0,
            source.composition.max_block_bytes,
        )
        .ok_or(TemplateServicePremiseError::NoProposalCapacity)?;
        if proposal_capacity.proposals == 0 {
            return Err(TemplateServicePremiseError::NoProposalCapacity);
        }

        // Stable eligibility is a prospective theorem premise, not the
        // current pack result. Compile the exact candidate cut in its legal
        // all-Proposed continuation; `current_compilation` separately maps
        // the captured current statuses and `TwoPhaseLiveness` seals them to
        // the primitive proposal-history context.
        let initial = compile_template_service_cut(
            &source,
            &vec![ProposalWindowPosition::Proposed; source.candidates.len()],
            &vec![false; source.candidates.len()],
        )?;
        if initial.retained_source_indices.is_empty() {
            return Err(TemplateServicePremiseError::NoCommitCapacity);
        }
        let retained_set = initial
            .retained_source_indices
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut cohort_candidates = Vec::new();
        let mut conditional_predecessors = BTreeMap::new();
        cohort_candidates
            .try_reserve(initial.retained_source_indices.len())
            .map_err(|_| TemplateServicePremiseError::InvalidSourceCut)?;
        for (retained_order, source_index) in
            initial.retained_source_indices.iter().copied().enumerate()
        {
            let candidate = source
                .candidates
                .get(source_index)
                .ok_or(TemplateServicePremiseError::InvalidSourceCut)?;
            let parents = candidate
                .causal_parents
                .iter()
                .map(|parent| {
                    if !retained_set.contains(parent) {
                        return Err(TemplateServicePremiseError::InvalidSourceCut);
                    }
                    source
                        .candidates
                        .get(*parent)
                        .map(|parent| parent.proposal)
                        .ok_or(TemplateServicePremiseError::InvalidSourceCut)
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            let order = u16::try_from(retained_order)
                .map_err(|_| TemplateServicePremiseError::InvalidSourceCut)?;
            cohort_candidates.push(CausalCandidate::new(candidate.proposal, parents, order));
            conditional_predecessors.insert(
                candidate.proposal,
                candidate
                    .conditional_predecessors
                    .iter()
                    .filter(|predecessor| retained_set.contains(predecessor))
                    .map(|predecessor| {
                        source
                            .candidates
                            .get(*predecessor)
                            .map(|predecessor| predecessor.proposal)
                            .ok_or(TemplateServicePremiseError::InvalidSourceCut)
                    })
                    .collect::<Result<BTreeSet<_>, _>>()?,
            );
        }
        let cohort =
            StableAcceptedCohort::from_candidates(cohort_candidates, conditional_predecessors)
                .map_err(TemplateServicePremiseError::Cohort)?;
        Ok(Self {
            source,
            cohort,
            packed_source_indices: initial.packed_source_indices,
            retained_source_indices: initial.retained_source_indices,
            shed_source_indices: initial.shed_source_indices,
        })
    }

    pub(crate) fn compile_for_foundation(source: TemplateServiceSourceCut) -> Option<Self> {
        Self::compile(source).ok()
    }

    fn current_compilation(
        &self,
    ) -> Result<TemplateServiceCompilation, TemplateServicePremiseError> {
        let positions = self
            .source
            .candidates
            .iter()
            .map(|candidate| match candidate.status {
                AcceptedStatus::Pending => ProposalWindowPosition::Outside,
                AcceptedStatus::Gap => ProposalWindowPosition::Gap,
                AcceptedStatus::Proposed => ProposalWindowPosition::Proposed,
            })
            .collect::<Vec<_>>();
        let satisfied = vec![false; self.source.candidates.len()];
        compile_template_service_cut(&self.source, &positions, &satisfied)
    }

    pub(crate) fn current_retained_source_indices(&self) -> Option<Vec<usize>> {
        self.current_compilation()
            .ok()
            .map(|compilation| compilation.retained_source_indices)
    }

    pub(crate) fn current_proposal_source_indices(&self) -> Option<Vec<usize>> {
        self.current_compilation()
            .ok()
            .map(|compilation| compilation.proposal_source_indices)
    }

    fn admits_context(&self, context: &ProposalContext) -> bool {
        self.source
            .candidates
            .iter()
            .all(|candidate| context.status(candidate.proposal).value() == candidate.status)
    }

    fn local_offer(
        &self,
        view: &ProposalView,
        committed: &BTreeSet<ProposalId>,
    ) -> Result<LocalTemplateOffer, TemplateServicePremiseError> {
        let retained = self
            .retained_source_indices
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let mut positions = vec![ProposalWindowPosition::Outside; self.source.candidates.len()];
        let mut satisfied = vec![false; self.source.candidates.len()];
        for (index, candidate) in self.source.candidates.iter().enumerate() {
            if committed.contains(&candidate.proposal) {
                satisfied[index] = true;
            } else {
                positions[index] = view.position(candidate.proposal);
            }
        }
        let compilation = compile_template_service_cut(&self.source, &positions, &satisfied)?;
        let proposals = compilation
            .proposal_source_indices
            .iter()
            .map(|index| self.source.candidates[*index].proposal)
            .collect::<BTreeSet<_>>();
        let commits = compilation
            .retained_source_indices
            .iter()
            .filter(|index| retained.contains(index))
            .filter_map(|index| self.source.candidates.get(*index))
            .map(|candidate| candidate.proposal)
            .collect();
        Ok(LocalTemplateOffer {
            proposals,
            commits,
            compatible_uncles: compilation.compatible_uncles,
        })
    }

    pub(crate) fn packed_source_indices(&self) -> &[usize] {
        &self.packed_source_indices
    }

    pub(crate) fn retained_source_indices(&self) -> &[usize] {
        &self.retained_source_indices
    }

    pub(crate) fn shed_source_indices(&self) -> &BTreeSet<usize> {
        &self.shed_source_indices
    }
}

/// Typed external premise. For a stable finite cohort, a current proposal
/// offer is realized within `proposal_service_bound` canonical heights and,
/// after realization at height `p`, a current transaction offer is realized
/// at least once in `[p + closest, p + farthest]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CanonicalServicePremise {
    proposal_service_bound: NonZeroU16,
    window: ProposalWindow,
}

impl CanonicalServicePremise {
    pub(super) const fn for_window(
        proposal_service_bound: u16,
        window: ProposalWindow,
    ) -> Option<Self> {
        let Some(proposal_service_bound) = NonZeroU16::new(proposal_service_bound) else {
            return None;
        };
        Some(Self {
            proposal_service_bound,
            window,
        })
    }

    const fn proposal_service_bound(self) -> u16 {
        self.proposal_service_bound.get()
    }

    const fn admits(self, window: ProposalWindow) -> bool {
        self.window.closest() == window.closest() && self.window.farthest() == window.farthest()
    }
}

/// Stronger operational corollary. At most `max_missed_blocks` consecutive
/// canonical heights fail to realize a current offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BoundedCanonicalOutage {
    max_missed_blocks: u16,
}

impl BoundedCanonicalOutage {
    pub(super) const fn new(max_missed_blocks: u16) -> Self {
        Self { max_missed_blocks }
    }

    pub(super) const fn implies_window_hit(
        self,
        window: ProposalWindow,
    ) -> Option<CanonicalServicePremise> {
        let Some(distance) = window.farthest().checked_sub(window.closest()) else {
            return None;
        };
        let Some(width) = distance.checked_add(1) else {
            return None;
        };
        if self.max_missed_blocks >= width {
            return None;
        }
        let Some(proposal_service_bound) = self.max_missed_blocks.checked_add(1) else {
            return None;
        };
        CanonicalServicePremise::for_window(proposal_service_bound, window)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CanonicalBlockService {
    Independent,
    CurrentProposalOfferWithoutOptionalUncles,
    CurrentProposalOfferWithCompatibleUncles,
    CurrentOfferWithoutOptionalUncles,
    CurrentOfferWithCompatibleUncles,
}

impl CanonicalBlockService {
    const fn realizes_proposal_offer(self) -> bool {
        !matches!(self, Self::Independent)
    }

    const fn realizes_commit_offer(self) -> bool {
        matches!(
            self,
            Self::CurrentOfferWithoutOptionalUncles | Self::CurrentOfferWithCompatibleUncles
        )
    }

    const fn includes_compatible_uncles(self) -> bool {
        matches!(
            self,
            Self::CurrentProposalOfferWithCompatibleUncles | Self::CurrentOfferWithCompatibleUncles
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalTemplateOffer {
    proposals: BTreeSet<ProposalId>,
    commits: BTreeSet<ProposalId>,
    compatible_uncles: Vec<CandidateUncleInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TwoPhaseBlockObservation {
    height: u16,
    offered_proposals: BTreeSet<ProposalId>,
    offered_commits: BTreeSet<ProposalId>,
    proposals: BTreeSet<ProposalId>,
    uncle_proposals: BTreeSet<ProposalId>,
    committed: BTreeSet<ProposalId>,
}

impl TwoPhaseBlockObservation {
    pub(super) const fn height(&self) -> u16 {
        self.height
    }

    pub(super) fn offered_proposals(&self) -> &BTreeSet<ProposalId> {
        &self.offered_proposals
    }

    pub(super) fn offered_commits(&self) -> &BTreeSet<ProposalId> {
        &self.offered_commits
    }

    pub(super) fn proposals(&self) -> &BTreeSet<ProposalId> {
        &self.proposals
    }

    pub(super) fn uncle_proposals(&self) -> &BTreeSet<ProposalId> {
        &self.uncle_proposals
    }

    pub(super) fn committed(&self) -> &BTreeSet<ProposalId> {
        &self.committed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TwoPhaseLivenessError {
    Context(ProposalContextError),
    UnsafeCommit(UnproposedCommit),
    Template(TemplateServicePremiseError),
    ServicePremise,
    TipHeightOverflow,
    Allocation,
    NondecreasingServiceRank,
}

/// A finite causally closed Accepted cohort under a stable total priority.
/// Local template offers and external canonical realization are distinct:
/// an independent block advances history but cannot fabricate adoption of the
/// current tx-pool template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TwoPhaseLiveness {
    context: ProposalContext,
    premise: TemplateServicePremise,
    committed: BTreeSet<ProposalId>,
}

impl TwoPhaseLiveness {
    pub(super) fn new(
        window: ProposalWindow,
        premise: TemplateServicePremise,
    ) -> Result<Self, TwoPhaseLivenessError> {
        Self::from_context(ProposalContext::initial(window), premise)
    }

    pub(super) fn from_context(
        context: ProposalContext,
        premise: TemplateServicePremise,
    ) -> Result<Self, TwoPhaseLivenessError> {
        if !premise.admits_context(&context) {
            return Err(TwoPhaseLivenessError::ServicePremise);
        }
        Ok(Self {
            context,
            premise,
            committed: BTreeSet::new(),
        })
    }

    pub(super) fn rank(&self) -> usize {
        self.premise.cohort.len() - self.committed.len()
    }

    pub(super) fn is_committed(&self, proposal: ProposalId) -> bool {
        self.committed.contains(&proposal)
    }

    pub(super) fn position(
        &self,
        proposal: ProposalId,
    ) -> Result<ProposalWindowPosition, TwoPhaseLivenessError> {
        Ok(self.context.position(proposal))
    }

    pub(super) fn step(
        &mut self,
        service: CanonicalBlockService,
    ) -> Result<TwoPhaseBlockObservation, TwoPhaseLivenessError> {
        let offer = self.local_offer()?;
        self.apply_offer(offer, service)
    }

    fn local_offer(&self) -> Result<LocalTemplateOffer, TwoPhaseLivenessError> {
        self.premise
            .local_offer(&self.context.view(), &self.committed)
            .map_err(TwoPhaseLivenessError::Template)
    }

    fn apply_offer(
        &mut self,
        offer: LocalTemplateOffer,
        service: CanonicalBlockService,
    ) -> Result<TwoPhaseBlockObservation, TwoPhaseLivenessError> {
        let offered_uncle_proposals = offer
            .compatible_uncles
            .iter()
            .flat_map(|uncle| uncle.proposals.iter().copied())
            .collect::<BTreeSet<_>>();
        let proposals = if service.realizes_proposal_offer() {
            offer.proposals.clone()
        } else {
            BTreeSet::new()
        };
        let committed = if service.realizes_commit_offer() {
            offer.commits.clone()
        } else {
            BTreeSet::new()
        };
        let uncle_proposals = if service.includes_compatible_uncles() {
            offered_uncle_proposals
        } else {
            BTreeSet::new()
        };
        verify_candidate_block(&self.context, &committed)?;
        self.committed.extend(committed.iter().copied());
        self.context = self
            .context
            .advance(proposals.clone(), uncle_proposals.clone())
            .map_err(TwoPhaseLivenessError::Context)?;
        Ok(TwoPhaseBlockObservation {
            height: self.context.tip_height(),
            offered_proposals: offer.proposals,
            offered_commits: offer.commits,
            proposals,
            uncle_proposals,
            committed,
        })
    }

    /// Exercise the latest service schedule admitted by the typed premise.
    /// Proposal service occurs only at its bound and commit service only on
    /// the final legal height of the first offered commit. Any earlier current
    /// template realization can only preserve or decrease the remaining rank;
    /// TLC explores all such earlier choices independently.
    pub(super) fn run_window_serviced(
        &mut self,
        premise: CanonicalServicePremise,
    ) -> Result<Vec<TwoPhaseBlockObservation>, TwoPhaseLivenessError> {
        if !premise.admits(self.context.window()) {
            return Err(TwoPhaseLivenessError::ServicePremise);
        }
        let proposal_rounds = self
            .premise
            .source
            .candidates
            .len()
            .div_ceil(self.premise.source.composition.max_block_proposals);
        let proposal_span = proposal_rounds
            .saturating_sub(1)
            .checked_mul(usize::from(premise.proposal_service_bound()))
            .ok_or(TwoPhaseLivenessError::TipHeightOverflow)?;
        let residence_span = usize::from(
            self.context
                .window()
                .farthest()
                .saturating_sub(self.context.window().closest()),
        );
        if proposal_span > residence_span {
            return Err(TwoPhaseLivenessError::ServicePremise);
        }
        let per_rank_bound = proposal_rounds
            .checked_mul(usize::from(premise.proposal_service_bound()))
            .and_then(|bound| bound.checked_add(usize::from(self.context.window().farthest())))
            .ok_or(TwoPhaseLivenessError::TipHeightOverflow)?;
        let total_bound = self
            .rank()
            .checked_mul(per_rank_bound)
            .ok_or(TwoPhaseLivenessError::TipHeightOverflow)?;
        if total_bound > usize::from(u16::MAX - self.context.tip_height()) {
            return Err(TwoPhaseLivenessError::TipHeightOverflow);
        }
        let mut observations = Vec::new();
        observations
            .try_reserve(total_bound)
            .map_err(|_| TwoPhaseLivenessError::Allocation)?;
        let mut proposal_misses = 0u16;

        while self.rank() != 0 {
            let before = self.rank();
            for _ in 0..per_rank_bound {
                let offer = self.local_offer()?;
                let empty_next = self
                    .context
                    .advance(BTreeSet::new(), BTreeSet::new())
                    .map_err(TwoPhaseLivenessError::Context)?;
                let commit_window_deadline = offer.commits.iter().any(|proposal| {
                    empty_next.position(*proposal) == ProposalWindowPosition::Outside
                });
                let next_proposal_miss = if offer.proposals.is_empty() {
                    0
                } else {
                    proposal_misses
                        .checked_add(1)
                        .ok_or(TwoPhaseLivenessError::TipHeightOverflow)?
                };
                let proposal_deadline = !offer.proposals.is_empty()
                    && next_proposal_miss >= premise.proposal_service_bound();
                let service = if commit_window_deadline {
                    CanonicalBlockService::CurrentOfferWithoutOptionalUncles
                } else if proposal_deadline {
                    CanonicalBlockService::CurrentProposalOfferWithoutOptionalUncles
                } else {
                    CanonicalBlockService::Independent
                };
                let observation = self.apply_offer(offer, service)?;
                proposal_misses = if service.realizes_proposal_offer()
                    || observation.offered_proposals().is_empty()
                {
                    0
                } else {
                    next_proposal_miss
                };
                observations.push(observation);
                if self.rank() < before {
                    proposal_misses = 0;
                    break;
                }
            }
            if self.rank() >= before {
                return Err(TwoPhaseLivenessError::NondecreasingServiceRank);
            }
        }
        Ok(observations)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HomogeneousProposalDrain {
    remaining: u64,
    limit: u64,
}

impl HomogeneousProposalDrain {
    /// This count quotient is admissible only for a finite independent cohort
    /// with stable ordering, no stronger arrivals and equal proposal cost.
    pub(super) const fn new(remaining: u64, limit: u64) -> Option<Self> {
        if limit == 0 {
            None
        } else {
            Some(Self { remaining, limit })
        }
    }

    pub(super) const fn rank(self) -> u64 {
        self.remaining.div_ceil(self.limit)
    }

    pub(super) fn propose_next(&mut self) -> u64 {
        let proposed = self.remaining.min(self.limit);
        self.remaining -= proposed;
        proposed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(closest: u16, farthest: u16) -> ProposalWindow {
        ProposalWindow::new(closest, farthest).expect("test window is valid")
    }

    fn independent_service_premise(
        proposals: BTreeSet<ProposalId>,
        proposal_limit: usize,
        commit_limit: usize,
    ) -> TemplateServicePremise {
        assert!(!proposals.is_empty());
        assert!(proposal_limit > 0);
        assert!(commit_limit > 0);
        const PROPOSAL_BYTES: usize = 1_024;
        let fitting_proposals = proposal_limit.min(proposals.len());
        let max_block_bytes = fitting_proposals
            .checked_mul(PROPOSAL_BYTES)
            .and_then(|bytes| bytes.checked_add(commit_limit))
            .expect("the finite fixture capacity fits usize");
        let candidates = proposals
            .into_iter()
            .enumerate()
            .map(|(index, proposal)| {
                let mut identity = [0; 32];
                identity[31] = proposal.0;
                TemplateServiceCandidate::new(
                    proposal,
                    AcceptedStatus::Pending,
                    EvictionRefinementMetrics::new(1, 1, 1),
                    BTreeSet::new(),
                    BTreeSet::new(),
                    0,
                    index,
                    index as u128,
                    identity,
                )
            })
            .collect();
        TemplateServicePremise::compile(TemplateServiceSourceCut::new(
            candidates,
            CurrentTemplateComposition::new(
                proposal_limit,
                PROPOSAL_BYTES,
                Vec::new(),
                0,
                max_block_bytes,
                u64::try_from(commit_limit).expect("the fixture cycle limit fits u64"),
            ),
            0,
        ))
        .expect("the independent exact source cut has positive compiled service")
    }

    fn status_context(encoding: u8) -> ProposalContext {
        let mut digits = encoding;
        let mut proposed = BTreeSet::new();
        let mut gap = BTreeSet::new();
        for raw in 0..3 {
            let proposal = ProposalId(raw);
            match digits % 3 {
                0 => {}
                1 => {
                    gap.insert(proposal);
                }
                2 => {
                    proposed.insert(proposal);
                }
                _ => unreachable!("base-three digit is total"),
            }
            digits /= 3;
        }
        ProposalContext::status_witness(proposed, gap).expect("the status partition is disjoint")
    }

    #[test]
    fn model_candidate_block_relation_is_exact_for_every_status_partition() {
        for encoding in 0..27 {
            let context = status_context(encoding);
            for mask in 0u8..8 {
                let committed = (0..3)
                    .filter(|raw| mask & (1 << raw) != 0)
                    .map(ProposalId)
                    .collect::<BTreeSet<_>>();
                let expected = committed
                    .iter()
                    .find(|proposal| {
                        context.position(**proposal) != ProposalWindowPosition::Proposed
                    })
                    .copied();
                assert_eq!(
                    verify_candidate_block(&context, &committed)
                        .err()
                        .and_then(|error| {
                            match error {
                                TwoPhaseLivenessError::UnsafeCommit(error) => {
                                    Some(error.proposal())
                                }
                                _ => None,
                            }
                        }),
                    expected,
                );
            }
        }
    }

    #[test]
    fn model_txpool_projection_excludes_genesis_history() {
        let proposal = ProposalId(7);
        let context = ProposalContext::new(
            2,
            window(2, 4),
            [ProposalBlock::new(
                0,
                BTreeSet::from([proposal]),
                BTreeSet::new(),
            )],
        )
        .expect("a custom legal genesis proposal is primitive history");
        assert_eq!(
            context.position(proposal),
            ProposalWindowPosition::Outside,
            "the tx-pool projection excludes the same genesis occurrence as the verifier"
        );
        assert_eq!(
            context
                .verified_view()
                .expect("normal history carries verification admission")
                .position(proposal),
            ProposalWindowPosition::Outside,
            "the admitted model projection excludes genesis proposals"
        );
        assert!(matches!(
            verify_candidate_block(&context, &BTreeSet::from([proposal])),
            Err(TwoPhaseLivenessError::UnsafeCommit(_))
        ));
    }

    #[test]
    fn model_causal_relation_rejects_missing_duplicate_and_cyclic_ownership() {
        let view = status_context(26).view();
        assert_eq!(
            causally_eligible(
                &view,
                &[
                    CausalCandidate::new(ProposalId(0), BTreeSet::new(), 0),
                    CausalCandidate::new(ProposalId(0), BTreeSet::new(), 1),
                ],
            ),
            Err(CausalSelectionError::DuplicateCandidate(ProposalId(0)))
        );
        assert_eq!(
            causally_eligible(
                &view,
                &[CausalCandidate::new(
                    ProposalId(0),
                    BTreeSet::from([ProposalId(1)]),
                    0,
                )],
            ),
            Err(CausalSelectionError::MissingParent {
                candidate: ProposalId(0),
                parent: ProposalId(1),
            })
        );
        assert_eq!(
            causally_eligible(
                &view,
                &[
                    CausalCandidate::new(ProposalId(0), BTreeSet::from([ProposalId(1)]), 0,),
                    CausalCandidate::new(ProposalId(1), BTreeSet::from([ProposalId(0)]), 1,),
                ],
            ),
            Err(CausalSelectionError::CausalCycle)
        );
    }

    #[test]
    fn model_liveness_premise_rejects_a_nonfitting_commit_package() {
        let candidate = TemplateServiceCandidate::new(
            ProposalId(0),
            AcceptedStatus::Pending,
            EvictionRefinementMetrics::new(10, 1, 2),
            BTreeSet::new(),
            BTreeSet::new(),
            0,
            0,
            0,
            [1; 32],
        );
        let source = TemplateServiceSourceCut::new(
            vec![candidate],
            CurrentTemplateComposition::new(1, 16, Vec::new(), 0, 17, 1),
            0,
        );
        assert_eq!(
            TemplateServicePremise::compile(source),
            Err(TemplateServicePremiseError::NoCommitCapacity),
            "a positive proposal prefix cannot fabricate commit capacity for a package that fails the exact cycle limit"
        );
    }

    #[test]
    fn model_liveness_premise_consumes_conditional_compiler_output() {
        let candidates = vec![
            TemplateServiceCandidate::new(
                ProposalId(1),
                AcceptedStatus::Pending,
                EvictionRefinementMetrics::new(20, 1, 1),
                BTreeSet::new(),
                BTreeSet::from([1]),
                1,
                2,
                0,
                [1; 32],
            ),
            TemplateServiceCandidate::new(
                ProposalId(0),
                AcceptedStatus::Pending,
                EvictionRefinementMetrics::new(10, 1, 1),
                BTreeSet::new(),
                BTreeSet::from([0]),
                1,
                0,
                1,
                [2; 32],
            ),
            TemplateServiceCandidate::new(
                ProposalId(2),
                AcceptedStatus::Pending,
                EvictionRefinementMetrics::new(1, 1, 1),
                BTreeSet::from([1]),
                BTreeSet::new(),
                0,
                1,
                2,
                [3; 32],
            ),
        ];
        let premise = TemplateServicePremise::compile(TemplateServiceSourceCut::new(
            candidates,
            CurrentTemplateComposition::new(3, 16, Vec::new(), 0, 51, 3),
            2,
        ))
        .expect("the exact compiler retains the strongest cycle representative");
        assert_eq!(premise.packed_source_indices(), &[0, 1, 2]);
        assert_eq!(premise.retained_source_indices(), &[0]);
        assert_eq!(premise.shed_source_indices(), &BTreeSet::from([1, 2]));
        let mut liveness = TwoPhaseLiveness::new(window(1, 2), premise)
            .expect("the captured Pending source cut matches the initial context");
        assert_eq!(liveness.rank(), 1, "shed work is not silently admitted");
        liveness
            .run_window_serviced(
                CanonicalServicePremise::for_window(1, window(1, 2))
                    .expect("service is bounded within the proposal window"),
            )
            .expect("the compiled nonempty cohort has finite service");
        assert!(liveness.is_committed(ProposalId(1)));
        assert!(!liveness.is_committed(ProposalId(0)));
        assert!(!liveness.is_committed(ProposalId(2)));
    }

    #[test]
    fn model_liveness_premise_rejects_a_spliced_dependency_bound() {
        let candidate = TemplateServiceCandidate::new(
            ProposalId(0),
            AcceptedStatus::Pending,
            EvictionRefinementMetrics::new(10, 1, 1),
            BTreeSet::new(),
            BTreeSet::new(),
            1,
            0,
            0,
            [1; 32],
        );
        let source = TemplateServiceSourceCut::new(
            vec![candidate],
            CurrentTemplateComposition::new(1, 16, Vec::new(), 0, 17, 1),
            0,
        );
        assert_eq!(
            TemplateServicePremise::compile(source),
            Err(TemplateServicePremiseError::Packing(
                TemplatePackingError::InvalidGraph
            )),
            "the complete scan bound must come from the same captured candidate cut"
        );
    }

    #[test]
    fn model_local_proposal_offer_preserves_a_nonfitting_priority_prefix() {
        let nonfitting = TemplateServiceCandidate::new(
            ProposalId(0),
            AcceptedStatus::Pending,
            EvictionRefinementMetrics::new(100, 1, 2),
            BTreeSet::new(),
            BTreeSet::new(),
            0,
            0,
            0,
            [1; 32],
        );
        let fitting = TemplateServiceCandidate::new(
            ProposalId(1),
            AcceptedStatus::Pending,
            EvictionRefinementMetrics::new(1, 1, 1),
            BTreeSet::new(),
            BTreeSet::new(),
            0,
            1,
            1,
            [2; 32],
        );
        let premise = TemplateServicePremise::compile(TemplateServiceSourceCut::new(
            vec![nonfitting, fitting],
            CurrentTemplateComposition::new(1, 16, Vec::new(), 0, 17, 1),
            0,
        ))
        .expect("the fitting tail is a valid commit cohort");
        assert_eq!(premise.retained_source_indices(), &[1]);
        let offer = premise
            .local_offer(&ProposalView::empty(), &BTreeSet::new())
            .expect("the exact source cut has one proposal slot");
        assert_eq!(
            offer.proposals,
            BTreeSet::from([ProposalId(0)]),
            "transaction packing cannot erase the first Pending production proposal"
        );
        assert!(offer.commits.is_empty());
    }

    #[test]
    fn model_template_capacity_obeys_consensus_proposal_count_before_bytes() {
        let observation = current_template_capacity_refinement(2, 1, 16, &[], 0, 0, 32)
            .expect("the finite byte cut is valid");
        assert_eq!(
            observation.proposals, 1,
            "free bytes cannot authorize more proposal ids than the consensus count limit"
        );
        assert_eq!(observation.remaining_transaction_bytes, 16);
    }

    #[test]
    fn model_liveness_premise_seals_uncle_capacity_into_the_source_cut() {
        let candidate = TemplateServiceCandidate::new(
            ProposalId(0),
            AcceptedStatus::Pending,
            EvictionRefinementMetrics::new(1, 1, 1),
            BTreeSet::new(),
            BTreeSet::new(),
            0,
            0,
            0,
            [1; 32],
        );
        let source = TemplateServiceSourceCut::new(
            vec![candidate],
            CurrentTemplateComposition::new(
                1,
                16,
                vec![CandidateUncleInput::new(
                    1,
                    BTreeSet::from([ProposalId(9)]),
                    17,
                )],
                0,
                17,
                1,
            ),
            0,
        );
        assert_eq!(
            TemplateServicePremise::compile(source),
            Err(TemplateServicePremiseError::NoCommitCapacity),
            "an uncle that consumes the sealed block cut cannot be spliced out to fabricate commit capacity"
        );
    }

    #[test]
    fn model_liveness_rejects_a_proposal_prefix_longer_than_the_residence_window() {
        let candidates = [2, 2, 1]
            .into_iter()
            .enumerate()
            .map(|(index, cycles)| {
                let mut identity = [0; 32];
                identity[31] = index as u8;
                TemplateServiceCandidate::new(
                    ProposalId(index as u8),
                    AcceptedStatus::Pending,
                    EvictionRefinementMetrics::new(10 - index as u64, 1, cycles),
                    BTreeSet::new(),
                    BTreeSet::new(),
                    0,
                    index,
                    index as u128,
                    identity,
                )
            })
            .collect();
        let premise = TemplateServicePremise::compile(TemplateServiceSourceCut::new(
            candidates,
            CurrentTemplateComposition::new(1, 16, Vec::new(), 0, 17, 1),
            0,
        ))
        .expect("the fitting tail remains an exact commit candidate");
        assert_eq!(premise.retained_source_indices(), &[2]);
        let window = window(1, 1);
        let mut liveness = TwoPhaseLiveness::new(window, premise)
            .expect("the captured Pending source cut matches the initial context");
        assert_eq!(
            liveness.run_window_serviced(
                CanonicalServicePremise::for_window(1, window)
                    .expect("each current proposal offer is realized immediately"),
            ),
            Err(TwoPhaseLivenessError::ServicePremise),
            "the first proposal can expire and repeat before the third Pending candidate is reached"
        );
    }

    #[test]
    fn model_liveness_premise_services_a_causal_package_under_one_proposal_slot() {
        let candidates = vec![
            TemplateServiceCandidate::new(
                ProposalId(0),
                AcceptedStatus::Pending,
                EvictionRefinementMetrics::new(1, 1, 1),
                BTreeSet::new(),
                BTreeSet::new(),
                0,
                0,
                1,
                [1; 32],
            ),
            TemplateServiceCandidate::new(
                ProposalId(1),
                AcceptedStatus::Pending,
                EvictionRefinementMetrics::new(100, 1, 1),
                BTreeSet::from([0]),
                BTreeSet::new(),
                0,
                1,
                0,
                [2; 32],
            ),
        ];
        let premise = TemplateServicePremise::compile(TemplateServiceSourceCut::new(
            candidates,
            CurrentTemplateComposition::new(1, 1_024, Vec::new(), 0, 1_026, 2),
            0,
        ))
        .expect("the parent-child package fits after one proposal slot");
        assert_eq!(
            premise.retained_source_indices(),
            &[0, 1],
            "the exact compiler seals a parent-first package order"
        );
        let window = window(2, 4);
        let mismatched_window = ProposalWindow::new(1, 2).expect("the test window is valid");
        let mut mismatched = TwoPhaseLiveness::new(window, premise.clone())
            .expect("the captured Pending source cut matches the initial context");
        assert_eq!(
            mismatched.run_window_serviced(
                CanonicalServicePremise::for_window(3, mismatched_window)
                    .expect("the distinct window has a finite service premise"),
            ),
            Err(TwoPhaseLivenessError::ServicePremise),
            "a service witness from another proposal window cannot be spliced into this cut"
        );
        let mut liveness = TwoPhaseLiveness::new(window, premise)
            .expect("the captured Pending source cut matches the initial context");
        liveness
            .run_window_serviced(
                CanonicalServicePremise::for_window(2, window)
                    .expect("proposal service is finite within the commit window"),
            )
            .expect("proposal status removes the child while its parent is offered");
        assert!(liveness.is_committed(ProposalId(0)));
        assert!(liveness.is_committed(ProposalId(1)));
    }

    #[test]
    fn model_window_serviced_two_phase_machine_is_safe_and_finitely_ranked_exhaustively() {
        for closest in 1..=4 {
            for farthest in closest..=6 {
                let window = window(closest, farthest);
                let width = farthest - closest + 1;
                assert!(
                    BoundedCanonicalOutage::new(width - 1)
                        .implies_window_hit(window)
                        .is_some()
                );
                assert_eq!(
                    BoundedCanonicalOutage::new(width).implies_window_hit(window),
                    None
                );
                for count in 1..=6 {
                    let admitted = (0..count).map(ProposalId).collect::<BTreeSet<_>>();
                    for proposals in 1..=3 {
                        for commits in 1..=3 {
                            for proposal_service_bound in 1..=3 {
                                let premise = independent_service_premise(
                                    admitted.clone(),
                                    proposals,
                                    commits,
                                );
                                let mut model = TwoPhaseLiveness::new(window, premise).expect(
                                    "the captured Pending source cut matches the initial context",
                                );
                                let initial_rank = model.rank();
                                let result = model.run_window_serviced(
                                    CanonicalServicePremise::for_window(
                                        proposal_service_bound,
                                        window,
                                    )
                                    .expect(
                                        "external proposal service is bounded within the window",
                                    ),
                                );
                                let proposal_rounds = usize::from(count).div_ceil(proposals);
                                let proposal_span = proposal_rounds.saturating_sub(1)
                                    * usize::from(proposal_service_bound);
                                if proposal_span + usize::from(closest) > usize::from(farthest) {
                                    assert_eq!(
                                        result,
                                        Err(TwoPhaseLivenessError::ServicePremise),
                                        "a prefix that cannot coexist inside the residence window has no finite production rank"
                                    );
                                    continue;
                                }
                                let observations =
                                    result.expect("the finite window-serviced cohort terminates");
                                assert_eq!(model.rank(), 0);
                                assert!(
                                    observations.len()
                                        <= initial_rank
                                            * (proposal_rounds
                                                * usize::from(proposal_service_bound)
                                                + usize::from(farthest))
                                );
                                assert!(observations.iter().all(|block| {
                                    block.offered_proposals().len() <= proposals
                                        && block.offered_commits().len() <= commits
                                        && block.proposals().len() <= proposals
                                        && block.committed().len() <= commits
                                }));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn model_qualitative_service_can_phase_miss_but_window_hit_restores_progress() {
        let proposal = ProposalId(0);
        let premise = independent_service_premise(BTreeSet::from([proposal]), 1, 1);
        let window = window(2, 4);
        let mut model = TwoPhaseLiveness::new(window, premise)
            .expect("the captured Pending source cut matches the initial context");
        let first = model
            .step(CanonicalBlockService::CurrentOfferWithoutOptionalUncles)
            .expect("initial mandatory-offer service is legal");
        assert_eq!(first.height(), 1);
        assert_eq!(first.offered_proposals(), &BTreeSet::from([proposal]));
        assert_eq!(first.proposals(), &BTreeSet::from([proposal]));
        assert_eq!(model.position(proposal), Ok(ProposalWindowPosition::Gap));

        for _ in 0..4 {
            model
                .step(CanonicalBlockService::Independent)
                .expect("a canonical block independent of this template is legal");
        }
        assert_eq!(
            model.position(proposal),
            Ok(ProposalWindowPosition::Outside)
        );
        assert!(!model.is_committed(proposal));

        let reproposal = model
            .step(CanonicalBlockService::CurrentOfferWithoutOptionalUncles)
            .expect("a later service event can re-propose after missing the commit phase");
        assert_eq!(reproposal.proposals(), &BTreeSet::from([proposal]));
        assert!(reproposal.committed().is_empty());
        model
            .run_window_serviced(
                CanonicalServicePremise::for_window(2, window)
                    .expect("the exact external window-hit premise is typed"),
            )
            .expect("window-hit service restores finite commit progress");
        assert!(model.is_committed(proposal));
    }

    #[test]
    fn model_proposal_service_does_not_fabricate_commit_capacity() {
        let proposal = ProposalId(0);
        let premise = independent_service_premise(BTreeSet::from([proposal]), 1, 1);
        let mut model = TwoPhaseLiveness::new(window(1, 2), premise)
            .expect("the captured Pending source cut matches the initial context");

        let proposed = model
            .step(CanonicalBlockService::CurrentProposalOfferWithCompatibleUncles)
            .expect("proposal-only realization is a legal template observation");
        assert_eq!(proposed.proposals(), &BTreeSet::from([proposal]));
        assert!(proposed.committed().is_empty());

        let proposal_only = model
            .step(CanonicalBlockService::CurrentProposalOfferWithCompatibleUncles)
            .expect("an uncle-constrained template can realize proposals but no commit");
        assert!(proposal_only.offered_commits().contains(&proposal));
        assert!(proposal_only.committed().is_empty());
        assert!(!model.is_committed(proposal));
    }

    #[test]
    fn model_4500_cohort_has_exactly_three_proposal_blocks() {
        let mut drain = HomogeneousProposalDrain::new(4_500, 1_500)
            .expect("the integration proposal limit is positive");
        assert_eq!(drain.rank(), 3);
        assert_eq!(drain.propose_next(), 1_500);
        assert_eq!(drain.rank(), 2);
        assert_eq!(drain.propose_next(), 1_500);
        assert_eq!(drain.rank(), 1);
        assert_eq!(drain.propose_next(), 1_500);
        assert_eq!(drain.rank(), 0);
        assert_eq!(drain.propose_next(), 0);
        assert_eq!(HomogeneousProposalDrain::new(1, 0), None);
    }
}
