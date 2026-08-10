//! Complete-candidate cost and boundary model for the M3.6 topology choice.
//!
//! This module does not predict wall time. It counts serial authority cuts and
//! explicit topology resources for one barrier-released, chain-backed,
//! independent workload. Existing composition tests own semantic equivalence;
//! these equations prevent a candidate from winning through imprecise prose.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecutionTopology {
    CurrentUak,
    SelfFusedWorkers,
    BoundedSemanticExchange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IndependentWaveInput {
    pub(super) owners: u32,
    pub(super) retained_worker_slots: u32,
    pub(super) mutation_batch_limit: u32,
    pub(super) current_ready_batch_limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CandidateApplyCost {
    pub(super) ingress: u64,
    pub(super) compute: u64,
    pub(super) ready_membership: u64,
    pub(super) effect_settlement: u64,
    pub(super) total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CandidateTopologySurface {
    pub(super) compute_mutation_callers: u32,
    pub(super) compute_tasks: u32,
    pub(super) transient_channel_slots: u32,
    pub(super) linear_capability_bound: u32,
    pub(super) added_join_edges: u32,
    pub(super) amortizes_one_available_wave: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExchangePermitState {
    FinishedCapabilityPresent,
    IdleFill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExchangePermitAcquisition {
    ImmediateOnly,
    MayQueueOne,
}

pub(super) const fn exchange_permit_acquisition(
    state: ExchangePermitState,
) -> ExchangePermitAcquisition {
    match state {
        // A finished capability must settle without waiting behind a Direct
        // holder. An immediately available grant may still fuse checkout.
        ExchangePermitState::FinishedCapabilityPresent => ExchangePermitAcquisition::ImmediateOnly,
        ExchangePermitState::IdleFill => ExchangePermitAcquisition::MayQueueOne,
    }
}

impl IndependentWaveInput {
    pub(super) fn compile(self, topology: ExecutionTopology) -> Option<CandidateApplyCost> {
        if self.owners == 0
            || self.retained_worker_slots == 0
            || self.mutation_batch_limit == 0
            || self.current_ready_batch_limit == 0
        {
            return None;
        }
        let owners = u64::from(self.owners);
        let workers = self.retained_worker_slots.min(self.owners);
        let current_ready_width = workers.min(self.current_ready_batch_limit);
        let exchange_width = workers.min(self.mutation_batch_limit);
        let current_ready_waves = ceil_div(self.owners, current_ready_width)?;
        let exchange_waves = ceil_div(self.owners, exchange_width)?;

        let (ingress, compute, ready_membership) = match topology {
            ExecutionTopology::CurrentUak => (
                owners,
                owners.checked_mul(2)?,
                u64::from(current_ready_waves),
            ),
            ExecutionTopology::SelfFusedWorkers => (
                owners,
                owners.checked_add(u64::from(workers))?,
                u64::from(current_ready_waves),
            ),
            ExecutionTopology::BoundedSemanticExchange => (
                u64::from(ceil_div(self.owners, self.mutation_batch_limit)?),
                u64::from(exchange_waves).checked_add(1)?,
                u64::from(exchange_waves),
            ),
        };
        let effect_settlement = ready_membership;
        let total = ingress
            .checked_add(compute)?
            .checked_add(ready_membership)?
            .checked_add(effect_settlement)?;
        Some(CandidateApplyCost {
            ingress,
            compute,
            ready_membership,
            effect_settlement,
            total,
        })
    }

    pub(super) fn surface(self, topology: ExecutionTopology) -> Option<CandidateTopologySurface> {
        if self.retained_worker_slots == 0 {
            return None;
        }
        let workers = self.retained_worker_slots;
        Some(match topology {
            ExecutionTopology::CurrentUak | ExecutionTopology::SelfFusedWorkers => {
                CandidateTopologySurface {
                    compute_mutation_callers: workers,
                    compute_tasks: workers,
                    transient_channel_slots: 0,
                    linear_capability_bound: workers,
                    added_join_edges: 0,
                    amortizes_one_available_wave: false,
                }
            }
            ExecutionTopology::BoundedSemanticExchange => CandidateTopologySurface {
                compute_mutation_callers: 1,
                compute_tasks: workers.checked_add(1)?,
                transient_channel_slots: workers.checked_mul(2)?,
                linear_capability_bound: workers,
                added_join_edges: 1,
                amortizes_one_available_wave: true,
            },
        })
    }
}

fn ceil_div(value: u32, divisor: u32) -> Option<u32> {
    if value == 0 || divisor == 0 {
        return None;
    }
    value.checked_sub(1)?.checked_div(divisor)?.checked_add(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryTopology {
    CurrentGuarded,
    SemaphoreOnly { permits: u32 },
    PreparedScratch { permits: u32 },
    ResidentProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryTopologyInput {
    pub(super) concurrent_requests: u32,
    pub(super) owner_rows: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryTopologyCost {
    pub(super) concurrent_guard_scans: u32,
    pub(super) authority_row_visits: u64,
    pub(super) allocates_under_guard: bool,
    pub(super) sorts_under_guard: bool,
    pub(super) duplicate_resident_rows: u32,
    pub(super) per_apply_projection_work: bool,
    pub(super) bounded_capture_admission: bool,
}

impl QueryTopologyInput {
    pub(super) fn compile(self, topology: QueryTopology) -> Option<QueryTopologyCost> {
        let scans = match topology {
            QueryTopology::CurrentGuarded | QueryTopology::ResidentProjection => {
                self.concurrent_requests
            }
            QueryTopology::SemaphoreOnly { permits }
            | QueryTopology::PreparedScratch { permits } => {
                if permits == 0 {
                    return None;
                }
                self.concurrent_requests.min(permits)
            }
        };
        let row_visits = u64::from(scans).checked_mul(u64::from(self.owner_rows))?;
        let (guard_scans, rows, allocation, sorting, duplicate, projection, bounded) =
            match topology {
                QueryTopology::CurrentGuarded => (scans, row_visits, true, true, 0, false, false),
                QueryTopology::SemaphoreOnly { .. } => {
                    (scans, row_visits, true, true, 0, false, true)
                }
                QueryTopology::PreparedScratch { .. } => {
                    (scans, row_visits, false, false, 0, false, true)
                }
                QueryTopology::ResidentProjection => {
                    (0, 0, false, false, self.owner_rows, true, true)
                }
            };
        Some(QueryTopologyCost {
            concurrent_guard_scans: guard_scans,
            authority_row_visits: rows,
            allocates_under_guard: allocation,
            sorts_under_guard: sorting,
            duplicate_resident_rows: duplicate,
            per_apply_projection_work: projection,
            bounded_capture_admission: bounded,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryScratch {
    pub(super) capacity: u32,
    pub(super) max_capacity: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryScratchStep {
    Ready,
    Grow(QueryScratch),
    OrdinaryUnavailable,
    RequestExceedsBound,
}

impl QueryScratch {
    pub(super) fn remaining_rank(self) -> Option<u32> {
        self.max_capacity.checked_sub(self.capacity)
    }

    /// Decide one lock-external allocation step. Re-observation may request a
    /// larger buffer, but every successful growth strictly decreases the
    /// finite rank and no unchanged authority cut is retried.
    pub(super) fn prepare(
        self,
        observed_rows: u32,
        allocation_available: bool,
    ) -> QueryScratchStep {
        if self.capacity > self.max_capacity || observed_rows > self.max_capacity {
            return QueryScratchStep::RequestExceedsBound;
        }
        if observed_rows <= self.capacity {
            return QueryScratchStep::Ready;
        }
        if !allocation_available {
            return QueryScratchStep::OrdinaryUnavailable;
        }
        let doubled = self
            .capacity
            .checked_mul(2)
            .map_or(self.max_capacity, |value| value.min(self.max_capacity));
        let grown = doubled.max(1).max(observed_rows).min(self.max_capacity);
        QueryScratchStep::Grow(Self {
            capacity: grown,
            max_capacity: self.max_capacity,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CachePublicationTopology {
    InlineTryWrite,
    BoundedWriter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CachePublicationInput {
    pub(super) updates: u32,
    pub(super) channel_updates: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CachePublicationCost {
    pub(super) persistent_tasks: u32,
    pub(super) resident_updates: u32,
    pub(super) write_lock_attempts: u32,
    pub(super) accepted_update_has_releaser: bool,
    pub(super) worker_waits_for_cache: bool,
}

impl CachePublicationInput {
    pub(super) fn compile(self, topology: CachePublicationTopology) -> CachePublicationCost {
        match topology {
            CachePublicationTopology::InlineTryWrite => CachePublicationCost {
                persistent_tasks: 0,
                resident_updates: 0,
                write_lock_attempts: self.updates,
                accepted_update_has_releaser: false,
                worker_waits_for_cache: false,
            },
            CachePublicationTopology::BoundedWriter => CachePublicationCost {
                persistent_tasks: 1,
                resident_updates: self.channel_updates,
                write_lock_attempts: self.updates,
                accepted_update_has_releaser: true,
                worker_waits_for_cache: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedIngressTopology {
    PerRequest,
    ExistingDispatcherDrain,
    DedicatedIngressActor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedIngressSurface {
    pub(super) batches_immediately_available_requests: bool,
    pub(super) added_tasks: u32,
    pub(super) added_channels: u32,
    pub(super) timer_or_fill_wait: bool,
    pub(super) exact_per_request_completion: bool,
}

pub(super) const fn retained_ingress_surface(
    topology: RetainedIngressTopology,
) -> RetainedIngressSurface {
    match topology {
        RetainedIngressTopology::PerRequest => RetainedIngressSurface {
            batches_immediately_available_requests: false,
            added_tasks: 0,
            added_channels: 0,
            timer_or_fill_wait: false,
            exact_per_request_completion: true,
        },
        RetainedIngressTopology::ExistingDispatcherDrain => RetainedIngressSurface {
            batches_immediately_available_requests: true,
            added_tasks: 0,
            added_channels: 0,
            timer_or_fill_wait: false,
            exact_per_request_completion: true,
        },
        RetainedIngressTopology::DedicatedIngressActor => RetainedIngressSurface {
            batches_immediately_available_requests: true,
            added_tasks: 1,
            added_channels: 1,
            timer_or_fill_wait: false,
            exact_per_request_completion: true,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrderedBoundaryTopology {
    SharedReliableSender,
    TypedReorgAndBoundedAdmin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProducerResidencyBound {
    UnboundedByProtocol,
    Bounded(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OrderedBoundaryInput {
    pub(super) trusted_reorg_publishers: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OrderedBoundaryCost {
    pub(super) waiting_payloads: ProducerResidencyBound,
    pub(super) reorg_is_lossless: bool,
    pub(super) excess_admin_is_fail_fast: bool,
    pub(super) accepted_admin_preserves_order: bool,
    pub(super) added_admin_gate: bool,
}

impl OrderedBoundaryInput {
    pub(super) fn compile(self, topology: OrderedBoundaryTopology) -> Option<OrderedBoundaryCost> {
        Some(match topology {
            OrderedBoundaryTopology::SharedReliableSender => OrderedBoundaryCost {
                waiting_payloads: ProducerResidencyBound::UnboundedByProtocol,
                reorg_is_lossless: true,
                excess_admin_is_fail_fast: false,
                accepted_admin_preserves_order: true,
                added_admin_gate: false,
            },
            OrderedBoundaryTopology::TypedReorgAndBoundedAdmin => OrderedBoundaryCost {
                waiting_payloads: ProducerResidencyBound::Bounded(
                    self.trusted_reorg_publishers.checked_add(1)?,
                ),
                reorg_is_lossless: true,
                excess_admin_is_fail_fast: true,
                accepted_admin_preserves_order: true,
                added_admin_gate: true,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CompleteTopology {
    pub(super) execution: ExecutionTopology,
    pub(super) ingress: RetainedIngressTopology,
    pub(super) query: QueryTopology,
    pub(super) cache: CachePublicationTopology,
    pub(super) ordered: OrderedBoundaryTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CompleteTopologyGap {
    PerOwnerAvailableWaveCuts,
    PerRequestRetainedIngress,
    UnboundedFullQueryAdmission,
    GuardHeldFallibleQueryWork,
    DuplicateResidentQueryProjection,
    CacheUpdateWithoutReleaser,
    UnboundedOrderedProducerResidency,
}

impl CompleteTopology {
    pub(super) fn gaps(
        self,
        execution: IndependentWaveInput,
        query: QueryTopologyInput,
        cache: CachePublicationInput,
        ordered: OrderedBoundaryInput,
    ) -> Option<Vec<CompleteTopologyGap>> {
        let mut gaps = Vec::new();
        if !execution
            .surface(self.execution)?
            .amortizes_one_available_wave
        {
            gaps.push(CompleteTopologyGap::PerOwnerAvailableWaveCuts);
        }
        if !retained_ingress_surface(self.ingress).batches_immediately_available_requests {
            gaps.push(CompleteTopologyGap::PerRequestRetainedIngress);
        }
        let query = query.compile(self.query)?;
        if !query.bounded_capture_admission {
            gaps.push(CompleteTopologyGap::UnboundedFullQueryAdmission);
        }
        if query.allocates_under_guard || query.sorts_under_guard {
            gaps.push(CompleteTopologyGap::GuardHeldFallibleQueryWork);
        }
        if query.duplicate_resident_rows != 0 || query.per_apply_projection_work {
            gaps.push(CompleteTopologyGap::DuplicateResidentQueryProjection);
        }
        if !cache.compile(self.cache).accepted_update_has_releaser {
            gaps.push(CompleteTopologyGap::CacheUpdateWithoutReleaser);
        }
        if matches!(
            ordered.compile(self.ordered)?.waiting_payloads,
            ProducerResidencyBound::UnboundedByProtocol
        ) {
            gaps.push(CompleteTopologyGap::UnboundedOrderedProducerResidency);
        }
        gaps.sort_unstable();
        gaps.dedup();
        Some(gaps)
    }
}

// The global normal-form quotient is deliberately semantic rather than a list
// of Rust implementations. Every architecture in the declared release basis
// is classified by the observation that distinguishes each axis; syntax-only
// variants normalize to the same representative.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AuthorityNormalForm {
    UniqueMinimumApply,
    UniqueOversizedApply,
    SplitLifecycleAuthorities,
    UniversalActor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum LifecycleNormalForm {
    SealedLinearExact,
    SealedLinearWithRollback,
    CopiedOrInferredEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CouplingNormalForm {
    ExactCanonicalAvailableCut,
    PerOwnerIndependentCuts,
    TimerOrApproximateBatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProgressNormalForm {
    SameApplyDerivedFiniteRank,
    SameApplyWithResidentDerivedDag,
    ResidentDecisionAuthority,
    PollingOrUnchangedRetry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ResourceNormalForm {
    UnifiedBoundedFallible,
    PartitionedBoundedCharged,
    FragmentedOrUncharged,
    InfallibleOrUnbounded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProjectionNormalForm {
    PostCommitDerivedPrepared,
    PostCommitWithResidentDerivedProjection,
    ExternalVetoOrGuardIo,
    ResidentPolicyProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TaskNormalForm {
    BoundedOwnedMinimalLanes,
    BoundedOwnedExtraActor,
    UniversalSerializedLoop,
    DetachedOrRepairTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CompatibilityNormalForm {
    ForwardLegacyMajorGeneratedLanding,
    ForwardLegacyMajorWithNonAuthoritativeFacade,
    DropsSupportedLegacy,
    ReverseMigrationOrAuthorityFacade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct GlobalNormalForm {
    pub(super) authority: AuthorityNormalForm,
    pub(super) lifecycle: LifecycleNormalForm,
    pub(super) coupling: CouplingNormalForm,
    pub(super) progress: ProgressNormalForm,
    pub(super) resources: ResourceNormalForm,
    pub(super) projections: ProjectionNormalForm,
    pub(super) tasks: TaskNormalForm,
    pub(super) compatibility: CompatibilityNormalForm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FeasibilityLaw {
    HardConstraint,
    ConcurrencyLaw,
    CouplingLaw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GlobalFeasibilityGap {
    NonUniqueAuthority,
    UnnecessaryIndependentSerialization,
    NonMinimumCouplingCut,
    InexactCapabilityOrEvidence,
    ApproximateOrTimedBatch,
    DuplicateDependencyPolicy,
    UnchangedCutRetry,
    ResourceConservationViolation,
    HostileResourceUnbounded,
    ExternalEffectVetoOrGuardIo,
    DuplicateProjectionPolicy,
    UntotalTaskOrRepairTopology,
    CompatibilityBoundaryViolation,
}

impl GlobalFeasibilityGap {
    pub(super) const fn law(self) -> FeasibilityLaw {
        match self {
            Self::UnnecessaryIndependentSerialization => FeasibilityLaw::ConcurrencyLaw,
            Self::NonMinimumCouplingCut => FeasibilityLaw::CouplingLaw,
            Self::NonUniqueAuthority
            | Self::InexactCapabilityOrEvidence
            | Self::ApproximateOrTimedBatch
            | Self::DuplicateDependencyPolicy
            | Self::UnchangedCutRetry
            | Self::ResourceConservationViolation
            | Self::HostileResourceUnbounded
            | Self::ExternalEffectVetoOrGuardIo
            | Self::DuplicateProjectionPolicy
            | Self::UntotalTaskOrRepairTopology
            | Self::CompatibilityBoundaryViolation => FeasibilityLaw::HardConstraint,
        }
    }
}

impl FeasibilityLaw {
    const fn index(self) -> usize {
        match self {
            Self::HardConstraint => 0,
            Self::ConcurrencyLaw => 1,
            Self::CouplingLaw => 2,
        }
    }
}

impl GlobalNormalForm {
    pub(super) fn feasibility_gaps(self) -> Vec<GlobalFeasibilityGap> {
        use GlobalFeasibilityGap as Gap;

        let mut gaps = Vec::new();
        match self.authority {
            AuthorityNormalForm::UniqueMinimumApply => {}
            AuthorityNormalForm::UniqueOversizedApply => gaps.push(Gap::NonMinimumCouplingCut),
            AuthorityNormalForm::SplitLifecycleAuthorities => gaps.push(Gap::NonUniqueAuthority),
            AuthorityNormalForm::UniversalActor => {
                gaps.push(Gap::UnnecessaryIndependentSerialization);
            }
        }
        if matches!(
            self.lifecycle,
            LifecycleNormalForm::CopiedOrInferredEvidence
        ) {
            gaps.push(Gap::InexactCapabilityOrEvidence);
        }
        match self.coupling {
            CouplingNormalForm::ExactCanonicalAvailableCut => {}
            CouplingNormalForm::PerOwnerIndependentCuts => {
                gaps.push(Gap::UnnecessaryIndependentSerialization);
            }
            CouplingNormalForm::TimerOrApproximateBatch => {
                gaps.push(Gap::ApproximateOrTimedBatch);
            }
        }
        match self.progress {
            ProgressNormalForm::SameApplyDerivedFiniteRank
            | ProgressNormalForm::SameApplyWithResidentDerivedDag => {}
            ProgressNormalForm::ResidentDecisionAuthority => {
                gaps.push(Gap::DuplicateDependencyPolicy);
            }
            ProgressNormalForm::PollingOrUnchangedRetry => gaps.push(Gap::UnchangedCutRetry),
        }
        match self.resources {
            ResourceNormalForm::UnifiedBoundedFallible
            | ResourceNormalForm::PartitionedBoundedCharged => {}
            ResourceNormalForm::FragmentedOrUncharged => {
                gaps.push(Gap::ResourceConservationViolation);
            }
            ResourceNormalForm::InfallibleOrUnbounded => {
                gaps.push(Gap::HostileResourceUnbounded);
            }
        }
        match self.projections {
            ProjectionNormalForm::PostCommitDerivedPrepared
            | ProjectionNormalForm::PostCommitWithResidentDerivedProjection => {}
            ProjectionNormalForm::ExternalVetoOrGuardIo => {
                gaps.push(Gap::ExternalEffectVetoOrGuardIo);
            }
            ProjectionNormalForm::ResidentPolicyProjection => {
                gaps.push(Gap::DuplicateProjectionPolicy);
            }
        }
        match self.tasks {
            TaskNormalForm::BoundedOwnedMinimalLanes | TaskNormalForm::BoundedOwnedExtraActor => {}
            TaskNormalForm::UniversalSerializedLoop => {
                gaps.push(Gap::UnnecessaryIndependentSerialization);
            }
            TaskNormalForm::DetachedOrRepairTopology => {
                gaps.push(Gap::UntotalTaskOrRepairTopology);
            }
        }
        if matches!(
            self.compatibility,
            CompatibilityNormalForm::DropsSupportedLegacy
                | CompatibilityNormalForm::ReverseMigrationOrAuthorityFacade
        ) {
            gaps.push(Gap::CompatibilityBoundaryViolation);
        }
        gaps
    }

    /// Ordered extra cost above the irreducible release-law lower bound. The
    /// coordinates follow `optimization_goal.static_objective` exactly.
    pub(super) const fn static_extra_cost(self) -> [u32; 7] {
        let mut cost = [0; 7];
        if matches!(
            self.lifecycle,
            LifecycleNormalForm::SealedLinearWithRollback
        ) {
            cost[0] += 1;
            cost[2] += 1;
            cost[3] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.progress,
            ProgressNormalForm::SameApplyWithResidentDerivedDag
        ) {
            cost[3] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.projections,
            ProjectionNormalForm::PostCommitWithResidentDerivedProjection
        ) {
            cost[3] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(self.tasks, TaskNormalForm::BoundedOwnedExtraActor) {
            cost[0] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.resources,
            ResourceNormalForm::PartitionedBoundedCharged
        ) {
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.compatibility,
            CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade
        ) {
            cost[0] += 1;
        }
        cost
    }

    const fn is_selected_core(self) -> bool {
        matches!(self.authority, AuthorityNormalForm::UniqueMinimumApply)
            && matches!(self.lifecycle, LifecycleNormalForm::SealedLinearExact)
            && matches!(
                self.coupling,
                CouplingNormalForm::ExactCanonicalAvailableCut
            )
            && matches!(
                self.progress,
                ProgressNormalForm::SameApplyDerivedFiniteRank
            )
            && matches!(self.resources, ResourceNormalForm::UnifiedBoundedFallible)
            && matches!(
                self.projections,
                ProjectionNormalForm::PostCommitDerivedPrepared
            )
            && matches!(self.tasks, TaskNormalForm::BoundedOwnedMinimalLanes)
    }

    pub(super) const fn is_selected_witness(self) -> bool {
        self.is_selected_core()
            && matches!(
                self.compatibility,
                CompatibilityNormalForm::ForwardLegacyMajorGeneratedLanding
            )
    }

    pub(super) const fn is_non_authoritative_facade_variant(self) -> bool {
        self.is_selected_core()
            && matches!(
                self.compatibility,
                CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade
            )
    }
}

const AUTHORITIES: [AuthorityNormalForm; 4] = [
    AuthorityNormalForm::UniqueMinimumApply,
    AuthorityNormalForm::UniqueOversizedApply,
    AuthorityNormalForm::SplitLifecycleAuthorities,
    AuthorityNormalForm::UniversalActor,
];
const LIFECYCLES: [LifecycleNormalForm; 3] = [
    LifecycleNormalForm::SealedLinearExact,
    LifecycleNormalForm::SealedLinearWithRollback,
    LifecycleNormalForm::CopiedOrInferredEvidence,
];
const COUPLINGS: [CouplingNormalForm; 3] = [
    CouplingNormalForm::ExactCanonicalAvailableCut,
    CouplingNormalForm::PerOwnerIndependentCuts,
    CouplingNormalForm::TimerOrApproximateBatch,
];
const PROGRESS_FORMS: [ProgressNormalForm; 4] = [
    ProgressNormalForm::SameApplyDerivedFiniteRank,
    ProgressNormalForm::SameApplyWithResidentDerivedDag,
    ProgressNormalForm::ResidentDecisionAuthority,
    ProgressNormalForm::PollingOrUnchangedRetry,
];
const RESOURCES: [ResourceNormalForm; 4] = [
    ResourceNormalForm::UnifiedBoundedFallible,
    ResourceNormalForm::PartitionedBoundedCharged,
    ResourceNormalForm::FragmentedOrUncharged,
    ResourceNormalForm::InfallibleOrUnbounded,
];
const PROJECTIONS: [ProjectionNormalForm; 4] = [
    ProjectionNormalForm::PostCommitDerivedPrepared,
    ProjectionNormalForm::PostCommitWithResidentDerivedProjection,
    ProjectionNormalForm::ExternalVetoOrGuardIo,
    ProjectionNormalForm::ResidentPolicyProjection,
];
const TASKS: [TaskNormalForm; 4] = [
    TaskNormalForm::BoundedOwnedMinimalLanes,
    TaskNormalForm::BoundedOwnedExtraActor,
    TaskNormalForm::UniversalSerializedLoop,
    TaskNormalForm::DetachedOrRepairTopology,
];
const COMPATIBILITIES: [CompatibilityNormalForm; 4] = [
    CompatibilityNormalForm::ForwardLegacyMajorGeneratedLanding,
    CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade,
    CompatibilityNormalForm::DropsSupportedLegacy,
    CompatibilityNormalForm::ReverseMigrationOrAuthorityFacade,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GlobalOptimalitySummary {
    pub(super) axis_cardinalities: [usize; 8],
    pub(super) total_normal_forms: usize,
    pub(super) feasible_normal_forms: usize,
    pub(super) rejected_normal_forms: usize,
    pub(super) rejected_by_law: [usize; 3],
    pub(super) minimum_static_extra_cost: [u32; 7],
    pub(super) static_minimizers: usize,
    pub(super) selected_static_minimizers: usize,
    pub(super) minimum_facade_static_extra_cost: [u32; 7],
    pub(super) minimum_partitioned_resource_static_extra_cost: [u32; 7],
}

pub(super) fn global_optimality_summary() -> GlobalOptimalitySummary {
    let mut summary = GlobalOptimalitySummary {
        axis_cardinalities: [
            AUTHORITIES.len(),
            LIFECYCLES.len(),
            COUPLINGS.len(),
            PROGRESS_FORMS.len(),
            RESOURCES.len(),
            PROJECTIONS.len(),
            TASKS.len(),
            COMPATIBILITIES.len(),
        ],
        total_normal_forms: 0,
        feasible_normal_forms: 0,
        rejected_normal_forms: 0,
        rejected_by_law: [0; 3],
        minimum_static_extra_cost: [u32::MAX; 7],
        static_minimizers: 0,
        selected_static_minimizers: 0,
        minimum_facade_static_extra_cost: [u32::MAX; 7],
        minimum_partitioned_resource_static_extra_cost: [u32::MAX; 7],
    };
    visit_global_normal_forms(|normal_form| {
        summary.total_normal_forms += 1;
        let gaps = normal_form.feasibility_gaps();
        if gaps.is_empty() {
            summary.feasible_normal_forms += 1;
            let cost = normal_form.static_extra_cost();
            if normal_form.is_non_authoritative_facade_variant() {
                summary.minimum_facade_static_extra_cost =
                    summary.minimum_facade_static_extra_cost.min(cost);
            }
            if matches!(
                normal_form.resources,
                ResourceNormalForm::PartitionedBoundedCharged
            ) {
                summary.minimum_partitioned_resource_static_extra_cost = summary
                    .minimum_partitioned_resource_static_extra_cost
                    .min(cost);
            }
            if cost < summary.minimum_static_extra_cost {
                summary.minimum_static_extra_cost = cost;
                summary.static_minimizers = 1;
                summary.selected_static_minimizers = usize::from(normal_form.is_selected_witness());
            } else if cost == summary.minimum_static_extra_cost {
                summary.static_minimizers += 1;
                summary.selected_static_minimizers +=
                    usize::from(normal_form.is_selected_witness());
            }
            return;
        }

        summary.rejected_normal_forms += 1;
        let mut laws = [false; 3];
        for gap in gaps {
            laws[gap.law().index()] = true;
        }
        for (count, observed) in summary.rejected_by_law.iter_mut().zip(laws) {
            *count += usize::from(observed);
        }
    });
    summary
}

pub(super) fn visit_global_normal_forms(mut visit: impl FnMut(GlobalNormalForm)) {
    for authority in AUTHORITIES {
        for lifecycle in LIFECYCLES {
            for coupling in COUPLINGS {
                for progress in PROGRESS_FORMS {
                    for resources in RESOURCES {
                        for projections in PROJECTIONS {
                            for tasks in TASKS {
                                for compatibility in COMPATIBILITIES {
                                    visit(GlobalNormalForm {
                                        authority,
                                        lifecycle,
                                        coupling,
                                        progress,
                                        resources,
                                        projections,
                                        tasks,
                                        compatibility,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
