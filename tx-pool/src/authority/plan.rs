mod apply_seal;
mod chain_transition;
mod compute_exchange;
mod ingress;
mod membership;
mod settlement;

pub(in crate::authority) use self::apply_seal::ApplyToken;
#[cfg(test)]
pub(in crate::authority) use self::compute_exchange::PreparedSharedComputeExchange;
pub(in crate::authority) use self::compute_exchange::{
    CommittedComputeExchange, ComputeExchangeAssignment, ComputeExchangeCompletion,
    ComputeExchangeDeferred, ComputeExchangeDeferredRoute, ComputeExchangePlanFailure,
    ComputeExchangeRecoveries, ComputeExchangeRecoverySink, ComputeExchangeSettled,
    RecoveredComputeExchange, SharedComputeExchangeOutcome,
};
pub(in crate::authority) use self::ingress::{
    CommittedRetainedAdmissionBatch, ConcurrentRetainedIngressError, SharedRetainedIngressHead,
};

#[cfg(test)]
#[path = "tests/support/plan.rs"]
pub(in crate::authority) mod test_support;
#[cfg(test)]
use self::test_support::DependencyLossWork;

#[cfg(test)]
use super::ban::PeerBanDelta;
use super::ban::{PeerBanError, PeerBanSlotBank};
use super::chain::{
    DirectAdmissionReceipt, DirectAdmissionRejection, FinalAdmissionPreparation,
    FinalAdmissionReceipt, FinalAdmissionSubject, FinalAdmissionWork,
};
#[cfg(test)]
use super::chain::{FinalAdmissionRejection, FinalAdmissionRetry};
use super::dependency::{
    DependencyBatchDelta, DependencyControlDelta, DependencyEntryControlDelta, DependencyError,
    DependencyFinalization, DependencyFrontier, DependencyMaintenanceAction,
    DependencyMaintenancePlan, DependencyStageError, RowsActivatedDependencyBatch,
    SealedReadyPhaseDependency, SettlementDependencyEvidence, StagedDependencyBatch,
};
#[cfg(test)]
use super::effect::CommittedPeerCohortRevocation;
use super::effect::{
    CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
    CommittedRejection, CommittedRemoteIngressRelease, EffectBatch, EffectBuildError,
    EffectClosePlanError, EffectConfigError, EffectDelta, EffectError, EffectLimits, EffectLog,
    EffectPolicy, EffectPublication, EffectPublicationObservation, EffectSettlement,
    EffectSettlementPlan, EffectSettlementPlanError, EffectWakeProjection,
    GenerationResetPlanError, ParentTransactionRequest, PendingRecentReject, RejectionAudience,
};
use super::indexes::{
    AcceptedExpiryHead, AuthorityIndexes, IndexDelta, IndexError, RemoteExpiryWitness,
};
use super::ingress::DirectCommand;
use super::read::AuthorityReadView;
pub(in crate::authority) use super::rejection::MembershipReject;
use super::rejection::{
    CommittedPublicReject, ComponentLimitKind, DirectRejectionValidity, DirectTransactionRejection,
};
use super::resources::{
    ChargeProjection, ChargeRecord, ChargedAdmission, ComputeGrant, DirectAcceptedInsertionError,
    OwnerRemovalResourcePlan, ResourceBatchPlan, ResourceCapacityWaitIdentity,
    ResourceCommitHealth, ResourceError, ResourceLedger, ResourceLimits, ResourcePlan,
    ResourceVector,
};
use super::scheduler::{
    FairFrontier, QueueLane, ReadyApplyReservation, ReadyReservation, ReadySlotReservation,
    SchedulerBatchDelta, SchedulerDelta, SchedulerError, SchedulerWakeProjection,
    StagedIngressVisibility, VerifyOrder,
};
use super::shard::{ShardApplySupport, ShardOwnerSourcePlan, ShardReadSupport, ShardWriteSupport};
use super::shard::{
    ShardProposedCountPlanError, ShardedOwnerMap, ShardedOwnerReadGuard, ShardedOwnerWriteCut,
};
#[cfg(test)]
use super::source::AuthoritySourceVersionSnapshot;
use super::source::{AuthoritySourceVersions, PoolTemplateVersions, SourceVersionDelta};
use super::state::{
    AcceptedAtMillis, AcceptedEntry, AcceptedProvenance, AdmissionBasis,
    ApplyOwnerBatchReservationError, ApplySequence, Arrival, AsyncProcessStart, AuthorityClockBank,
    AuthorityClocks, ChainRevision, ChainViewId, DependencyCut, DependencyKey, DependencyOrigin,
    EntryVersion, KnownDependencies, MissingDependencies, OwnedTx, PayloadPolicy,
    PayloadPolicyEvolution, PoolGeneration, PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource,
    ProposalBase, QueuedWork, RawTxHash, RemoteDeadline, ReplacementHistoryEntry,
    ReplacementHistoryError, ResolvedFacts, TxRecord, ValidatedAdmission, VerifiedFacts,
};
use super::validation::FinalAdmissionValidationOutcome;
use super::work::{
    ComputeSettlement, MissingResolution, SettlementNext, SettlementRejection, SettlementToken,
};
use crate::error::Reject;
use ckb_types::{
    core::{EntryCompleted, error::OutPointError, tx_pool::get_transaction_weight},
    packed::OutPoint,
    prelude::Unpack,
};
use ckb_util::parking_lot::Mutex;
pub(in crate::authority) use membership::MembershipConfig;
pub(in crate::authority) use membership::RemovalCause;
#[cfg(test)]
pub(in crate::authority) use membership::StatusCounts;
pub(in crate::authority) use membership::{
    AcceptedOrderKey, AncestorAggregate, DescendantAggregate, EvictionOrderKey,
    MembershipProjection,
};
use membership::{
    AcceptedRemovalSet, AdministrativeClosureWitness, MembershipPolicyOutcome,
    MembershipPolicyWitness, MembershipRemoval, PreparedMembership, ProjectionDelta,
};
#[cfg(test)]
pub(in crate::authority) use settlement::SettlementPlan;
pub(in crate::authority) use settlement::{
    CoupledSettlementContinuation, IndependentCandidate, SettlementBatch,
    SharedIndependentSettlementCompilation, SharedReadyWaveCompilation,
};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;
#[cfg(test)]
use std::time::Instant;

pub(in crate::authority) use apply_seal::TxPoolAuthority;
use apply_seal::{OwnerResourceUpdate, PreparedOwnerResourceDelta};

impl TxPoolAuthority {
    pub(super) fn entry_guard(&self, hash: &RawTxHash) -> Option<ShardedOwnerReadGuard<'_>> {
        self.entries.get(hash)
    }

    #[cfg(test)]
    pub(super) fn entry(&self, hash: &RawTxHash) -> Option<OwnedTx> {
        self.entries.get(hash).as_deref().cloned()
    }

    pub(super) fn operational_metrics(&self) -> crate::metrics::OperationalMetrics {
        // Metrics are not policy authority, but their counters still promise
        // one complete projection.  Hold the existing fixed-layout read cut so
        // a concurrent sharded Apply cannot splice per-shard totals from two
        // different moments.
        let owners = self.entries.read_all();
        let (resources, _accepted) = self.resources.coherent_totals(&owners);
        let total = resources.preaccepted;
        let remote = resources.remote;
        let conflict = resources.replacement_history;
        crate::metrics::OperationalMetrics {
            kernel: crate::metrics::KernelUsage {
                total_entries: total.entries,
                total_bytes: total.total_bytes().map_or(usize::MAX, |bytes| bytes),
                remote_entries: remote.entries,
                remote_bytes: remote.total_bytes().map_or(usize::MAX, |bytes| bytes),
                conflict_entries: conflict.entries,
                conflict_bytes: conflict.total_bytes().map_or(usize::MAX, |bytes| bytes),
                active_work: total.active_work,
            },
            effects: self.effects.lock().operational_usage(),
        }
    }

    pub(super) fn accepted_spender(
        &self,
        input: &ckb_types::packed::OutPoint,
    ) -> Option<RawTxHash> {
        self.membership.spender(input)
    }

    pub(super) fn chain_revision(&self) -> ChainRevision {
        self.chain_view.revision()
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        &self.chain_view
    }

    /// Dependency evidence captured outside an Apply observes every sequence
    /// strictly before `next_sequence`. A concurrent Apply consumes
    /// `next_sequence`, so its loss event is newer than this cut and makes the
    /// direct receipt stale instead of being mistaken for already observed.
    pub(super) fn dependency_observation_cut(&self) -> DependencyCut {
        DependencyCut(ApplySequence(
            self.clocks.snapshot().next_sequence.0.saturating_sub(1),
        ))
    }

    /// Borrow one immutable authority cut for every query projection. The view
    /// exposes neither the primary owner enum nor independently captured
    /// accepted/preaccepted collections.
    pub(super) fn read_view(&self) -> AuthorityReadView<'_> {
        AuthorityReadView::new(
            self.generation,
            self.chain_view.clone(),
            &self.entries,
            &self.membership,
            self.membership_config,
            &self.source_versions,
        )
    }

    /// Bounded strongest-first Ready identities for the runtime's sealed
    /// validation capture. Raw identities never cross the authority module.
    #[cfg(test)]
    pub(in crate::authority) fn ready_candidates(
        &self,
    ) -> Result<Vec<(RawTxHash, EntryVersion)>, PlanError> {
        self.scheduler.lock().ready().map_err(PlanError::from)
    }

    pub(in crate::authority) fn reserve_ready_candidates(
        &self,
    ) -> Result<Option<ReadyReservation>, PlanError> {
        ReadyReservation::capture(&self.scheduler).map_err(PlanError::from)
    }

    pub(in crate::authority) fn reserved_ready_common_prefix_len<'a>(
        &self,
        reservation: &ReadyReservation,
        captured: impl IntoIterator<Item = (&'a RawTxHash, EntryVersion)>,
    ) -> usize {
        reservation.current_prefix_len(&self.scheduler, captured)
    }

    pub(in crate::authority) fn template_source_versions(&self) -> PoolTemplateVersions {
        let owners = self.entries.read_all();
        owners.template_sources(self.source_versions.template())
    }

    pub(in crate::authority) fn wake_projection(&self) -> AuthorityWakeProjection {
        let scheduler = self.scheduler.lock().wake_projection();
        self.wake_projection_with_scheduler(scheduler)
    }

    fn wake_projection_without_scheduler(&self) -> AuthorityWakeProjection {
        AuthorityWakeProjection {
            scheduler: SchedulerWakeProjection {
                resolve: None,
                verify_small: None,
                verify_any: None,
                ready: None,
            },
            active_work: 0,
            effects: self.effects.lock().wake_projection(),
            // Shared owner Apply advances exact per-shard template sources.
            // Its move-owned retirement receipt carries the net change bit,
            // so this wake edge needs no read of the unrelated global source
            // barrier merely to place equal before/after sentinels here.
            template: [ApplySequence(0); 3],
        }
    }

    fn wake_projection_with_scheduler(
        &self,
        scheduler: SchedulerWakeProjection,
    ) -> AuthorityWakeProjection {
        let template = self.source_versions.template();
        AuthorityWakeProjection {
            scheduler,
            // Ordinary Apply supplies the exact net release bit from its
            // already-reserved resource delta. Keeping a neutral sentinel in
            // this O(1) projection avoids two fixed 64-shard aggregate scans
            // while preserving the existing before/after wake relation.
            active_work: 0,
            effects: self.effects.lock().wake_projection(),
            template: [
                template.proposals.barrier(),
                template.transactions.barrier(),
                template.chain,
            ],
        }
    }

    /// Effect settlement owns no transaction, scheduler, dependency or
    /// template transition. Carry neutral sentinels for those unrelated
    /// domains so a shared owner Apply cannot be mistaken for a settlement
    /// wake merely because both commits overlap under the outer read guard.
    fn wake_projection_for_effect_settlement(
        effects: EffectWakeProjection,
    ) -> AuthorityWakeProjection {
        AuthorityWakeProjection {
            scheduler: SchedulerWakeProjection {
                resolve: None,
                verify_small: None,
                verify_any: None,
                ready: None,
            },
            active_work: 0,
            effects,
            template: [ApplySequence(0); 3],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AuthorityConfigError {
    Effect(EffectConfigError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Backpressure {
    ProposalCollision,
    TotalResources,
    RemoteResources,
    PeerResources,
    AcceptedResources,
    ComputeResources,
    GenerationReplacement,
    EffectCapacity,
    DependencyStageCapacity,
    Allocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StalePlan {
    Missing,
    Version,
    Phase,
    ChainRevision,
    Dependency,
    AcceptedObservation,
    Generation,
    EffectSequence,
    ClockBase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityFault {
    CounterExhausted,
    ResourceProjection,
    MembershipProjection,
    IndexProjection,
    SchedulerProjection,
    DependencyProjection,
    EffectProjection,
}

/// Closed Plan surface for the allocation-pressure generation terminal.
/// Every carrier and reset batch is already built before Apply; no ordinary
/// backpressure or stale retry can be represented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum GenerationReplacementPlanError {
    LifecycleClosed,
    Fault(AuthorityFault),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlanError {
    Duplicate,
    PayloadVariant,
    Membership(MembershipReject),
    Backpressure(Backpressure),
    ResourceContended(ResourceCapacityWaitIdentity),
    Stale(StalePlan),
    Fault(AuthorityFault),
    EffectClosed,
}

/// A compute settlement that could not be committed still owns the exact
/// work capability. Callers may retain it until a selected generation
/// replacement makes it stale, or discard it only after proving the reported
/// error already makes the work stale.
#[derive(Debug)]
#[must_use = "a failed compute settlement still owns the active work capability"]
pub(super) struct ComputeSettlementFailure {
    recovery: ComputeSettlementRecovery,
    token: SettlementToken,
    next: SettlementNext,
}

/// Closed progress contract for returning the sole compute capability.
///
/// Settlement may wait only for effect capacity released by the independent
/// publisher. Allocation has no authority-owned progress level and therefore
/// retains the exact capability until the generation terminal makes it stale.
/// Every other planning outcome is structural in this context. Keeping that
/// distinction at the producer prevents a future `PlanError` variant from
/// silently becoming an unbounded worker retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ComputeSettlementRecovery {
    Obsolete(StalePlan),
    RetryExact(SettlementChangedCut),
    CancelAfterAllocation,
    WaitEffectCapacity,
    Structural(PlanError),
}

/// Proof that an exact settlement lost only a final coherent-cut comparison
/// while its owner capability is still current and Computing. The private
/// constructor prevents structural carrier faults from entering the bounded
/// changed-cut retry lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SettlementChangedCut {
    domain: SettlementChangedDomain,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SettlementChangedDomain {
    Planning(StalePlan),
    GenerationOrChain,
    OwnerOrProjection,
    Scheduler,
    ResourceCapacity,
    DependencyStageCapacity,
}

impl SettlementChangedCut {
    fn planning(stale: StalePlan) -> Self {
        Self {
            domain: SettlementChangedDomain::Planning(stale),
        }
    }

    fn generation_or_chain() -> Self {
        Self {
            domain: SettlementChangedDomain::GenerationOrChain,
        }
    }

    fn owner_or_projection() -> Self {
        Self {
            domain: SettlementChangedDomain::OwnerOrProjection,
        }
    }

    fn scheduler() -> Self {
        Self {
            domain: SettlementChangedDomain::Scheduler,
        }
    }

    fn resource_capacity() -> Self {
        Self {
            domain: SettlementChangedDomain::ResourceCapacity,
        }
    }

    fn dependency_stage_capacity() -> Self {
        Self {
            domain: SettlementChangedDomain::DependencyStageCapacity,
        }
    }
}

impl ComputeSettlementRecovery {
    fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(stale) => Self::Obsolete(stale),
            PlanError::Backpressure(Backpressure::Allocation) => Self::CancelAfterAllocation,
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::WaitEffectCapacity,
            PlanError::Backpressure(Backpressure::DependencyStageCapacity) => {
                Self::RetryExact(SettlementChangedCut::dependency_stage_capacity())
            }
            PlanError::ResourceContended(identity) => {
                Self::Structural(PlanError::ResourceContended(identity))
            }
            PlanError::Backpressure(
                pressure @ (Backpressure::ProposalCollision
                | Backpressure::TotalResources
                | Backpressure::RemoteResources
                | Backpressure::PeerResources
                | Backpressure::AcceptedResources
                | Backpressure::ComputeResources
                | Backpressure::GenerationReplacement),
            ) => Self::Structural(PlanError::Backpressure(pressure)),
            PlanError::Duplicate => Self::Structural(PlanError::Duplicate),
            PlanError::PayloadVariant => Self::Structural(PlanError::PayloadVariant),
            PlanError::Membership(rejection) => Self::Structural(PlanError::Membership(rejection)),
            PlanError::Fault(fault) => Self::Structural(PlanError::Fault(fault)),
            PlanError::EffectClosed => Self::Structural(PlanError::EffectClosed),
        }
    }
}

impl ComputeSettlementFailure {
    fn new(error: PlanError, token: SettlementToken, next: SettlementNext) -> Self {
        Self {
            recovery: ComputeSettlementRecovery::from_plan(error),
            token,
            next,
        }
    }

    fn retry_exact(
        changed_cut: SettlementChangedCut,
        token: SettlementToken,
        next: SettlementNext,
    ) -> Self {
        Self {
            recovery: ComputeSettlementRecovery::RetryExact(changed_cut),
            token,
            next,
        }
    }

    pub(super) fn recovery(&self) -> &ComputeSettlementRecovery {
        &self.recovery
    }

    pub(super) fn into_settlement(self) -> ComputeSettlement {
        let Self {
            recovery: _,
            token,
            next,
        } = self;
        ComputeSettlement { token, next }
    }

    /// Discard an expensive result while retaining the exact owner capability
    /// until the already-selected generation replacement makes it obsolete.
    /// This matches exchange-side allocation recovery and avoids a transient
    /// `Computing -> Queued` Apply immediately before the whole generation is
    /// replaced.
    pub(super) fn discard_result_for_generation_replacement(self) -> ComputeSettlement {
        let Self {
            token,
            next,
            recovery: _,
        } = self;
        drop(next);
        ComputeSettlement {
            token,
            next: SettlementNext::Retry,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectSettlementError {
    StaleLease,
    Projection,
    CounterExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectCloseError {
    ActiveWork,
    AlreadyClosed,
    CounterExhausted,
}

/// Effect settlement has the same linear handoff rule as compute: planning
/// failure must return the publisher receipt instead of silently losing the
/// only tentative endpoint cursor for the resident record.
#[derive(Debug)]
#[must_use = "a failed effect settlement still owns the exact publication receipt"]
pub(super) struct EffectSettlementFailure {
    error: EffectSettlementError,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "this move-only capability is retained until the publisher fault is disposed; tests also recover it to prove linearity"
        )
    )]
    settlement: EffectSettlement,
}

impl EffectSettlementFailure {
    pub(super) fn error(&self) -> EffectSettlementError {
        self.error
    }
}

#[must_use = "a superseded reset receipt must retire outside the authority guard"]
#[expect(
    clippy::large_enum_variant,
    reason = "Applied is the ordinary hot path and already has to carry CommittedDelta across the authority guard; boxing it would add one allocation to every effect settlement solely to shrink the rare superseded-reset disposition"
)]
pub(super) enum EffectSettlementCommit {
    Applied(CommittedDelta),
    Superseded(EffectSettlement),
}

impl From<ResourceError> for PlanError {
    fn from(error: ResourceError) -> Self {
        match error {
            ResourceError::PreAcceptedLimit => Self::Backpressure(Backpressure::TotalResources),
            ResourceError::RemoteLimit => Self::Backpressure(Backpressure::RemoteResources),
            ResourceError::PeerLimit(_) => Self::Backpressure(Backpressure::PeerResources),
            ResourceError::ReplacementHistoryLimit => {
                Self::Fault(AuthorityFault::ResourceProjection)
            }
            ResourceError::AcceptedLimit => Self::Backpressure(Backpressure::AcceptedResources),
            ResourceError::ComputeEnvelope => Self::Backpressure(Backpressure::ComputeResources),
            ResourceError::Allocation => Self::Backpressure(Backpressure::Allocation),
            ResourceError::Arithmetic
            | ResourceError::ExistingChargeMismatch
            | ResourceError::AttributionMismatch
            | ResourceError::CapacityBankFault => Self::Fault(AuthorityFault::ResourceProjection),
            ResourceError::DuplicateChange => Self::Fault(AuthorityFault::ResourceProjection),
        }
    }
}

impl From<IndexError> for PlanError {
    fn from(error: IndexError) -> Self {
        match error {
            IndexError::ProposalCollision => Self::Backpressure(Backpressure::ProposalCollision),
            IndexError::Allocation => Self::Backpressure(Backpressure::Allocation),
            IndexError::Arithmetic => Self::Fault(AuthorityFault::CounterExhausted),
            IndexError::Projection => Self::Fault(AuthorityFault::IndexProjection),
        }
    }
}

impl From<SchedulerError> for PlanError {
    fn from(error: SchedulerError) -> Self {
        match error {
            SchedulerError::Allocation => Self::Backpressure(Backpressure::Allocation),
            SchedulerError::Stale => Self::Stale(StalePlan::Version),
            SchedulerError::Projection | SchedulerError::Arithmetic => {
                Self::Fault(AuthorityFault::SchedulerProjection)
            }
        }
    }
}

impl From<DependencyError> for PlanError {
    fn from(error: DependencyError) -> Self {
        match error {
            DependencyError::Projection => Self::Fault(AuthorityFault::DependencyProjection),
            DependencyError::Allocation => Self::Backpressure(Backpressure::Allocation),
            DependencyError::Stale => Self::Stale(StalePlan::Dependency),
            DependencyError::Fanout => Self::Membership(MembershipReject::ComponentLimit {
                kind: ComponentLimitKind::Mutation,
                limit: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            }),
            DependencyError::SurvivingAcceptedConsumer => {
                Self::Fault(AuthorityFault::DependencyProjection)
            }
        }
    }
}

impl From<DependencyStageError> for PlanError {
    fn from(error: DependencyStageError) -> Self {
        match error {
            DependencyStageError::Stale => Self::Stale(StalePlan::Dependency),
            DependencyStageError::Projection => Self::Fault(AuthorityFault::DependencyProjection),
            DependencyStageError::Capacity => {
                Self::Backpressure(Backpressure::DependencyStageCapacity)
            }
            DependencyStageError::Allocation => Self::Backpressure(Backpressure::Allocation),
        }
    }
}

impl From<EffectError> for PlanError {
    fn from(error: EffectError) -> Self {
        match error {
            EffectError::Full => Self::Backpressure(Backpressure::EffectCapacity),
            EffectError::Allocation => Self::Backpressure(Backpressure::Allocation),
            EffectError::Closed => Self::EffectClosed,
            EffectError::SequenceOvertaken => Self::Stale(StalePlan::EffectSequence),
            EffectError::Projection => Self::Fault(AuthorityFault::EffectProjection),
        }
    }
}

impl From<GenerationResetPlanError> for PlanError {
    fn from(error: GenerationResetPlanError) -> Self {
        match error {
            GenerationResetPlanError::Closed => Self::EffectClosed,
            GenerationResetPlanError::SequenceOvertaken => Self::Stale(StalePlan::EffectSequence),
        }
    }
}

impl From<PeerBanError> for PlanError {
    fn from(error: PeerBanError) -> Self {
        match error {
            PeerBanError::Contention => Self::Stale(StalePlan::Version),
            PeerBanError::CounterExhausted => Self::Fault(AuthorityFault::CounterExhausted),
        }
    }
}

impl From<ShardProposedCountPlanError> for PlanError {
    fn from(error: ShardProposedCountPlanError) -> Self {
        match error {
            ShardProposedCountPlanError::Projection => {
                Self::Fault(AuthorityFault::MembershipProjection)
            }
            ShardProposedCountPlanError::Arithmetic => {
                Self::Fault(AuthorityFault::CounterExhausted)
            }
            ShardProposedCountPlanError::Allocation => Self::Backpressure(Backpressure::Allocation),
        }
    }
}

/// Post-commit timing evidence consumed by the production metrics boundary.
///
/// This deliberately contains no owner hash, transition cause, chain view or
/// Apply sequence. Those facts already live in the authority/effect state and
/// retaining a second receipt solely for tests would create a shadow
/// projection that every transition had to keep synchronized.
#[derive(Debug)]
enum AsyncProcessObservations {
    None,
    #[cfg(any(test, feature = "internal"))]
    One(AsyncProcessStart),
    Batch(Vec<AsyncProcessStart>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct AuthorityWakeProjection {
    scheduler: SchedulerWakeProjection,
    active_work: usize,
    effects: EffectWakeProjection,
    template: [ApplySequence; 3],
}

impl AuthorityWakeProjection {
    fn with_effects(mut self, effects: super::effect::EffectWakeProjection) -> Self {
        self.effects = effects;
        self
    }
}

/// Exact before/after runnable projection produced by one committed Apply.
///
/// It carries no authority state and cannot select work. The runtime consumes
/// it only after the store guard and retirement payloads have been released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuthorityWakeTransition {
    before: AuthorityWakeProjection,
    after: AuthorityWakeProjection,
    dependency_maintenance_activated: bool,
    dependency_poisoned: bool,
    template_source_changed: bool,
}

impl AuthorityWakeTransition {
    pub(in crate::authority) fn between(
        before: AuthorityWakeProjection,
        after: AuthorityWakeProjection,
    ) -> Self {
        let template_source_changed = before.template != after.template;
        Self {
            before,
            after,
            dependency_maintenance_activated: false,
            dependency_poisoned: false,
            template_source_changed,
        }
    }

    fn with_effect_wake(mut self, effect: super::effect::EffectWakeTransition) -> Self {
        let (before, after) = effect.projections();
        self.before.effects = before;
        self.after.effects = after;
        self
    }

    fn head_advanced(before: Option<EntryVersion>, after: Option<EntryVersion>) -> bool {
        after.is_some() && before != after
    }

    fn resolve_advanced(self) -> bool {
        Self::head_advanced(self.before.scheduler.resolve, self.after.scheduler.resolve)
    }

    fn verify_small_advanced(self) -> bool {
        Self::head_advanced(
            self.before.scheduler.verify_small,
            self.after.scheduler.verify_small,
        )
    }

    fn verify_any_advanced(self) -> bool {
        Self::head_advanced(
            self.before.scheduler.verify_any,
            self.after.scheduler.verify_any,
        )
    }

    /// Publish one compute level when any compatible scheduler head changes or
    /// an active-work release may make a stable head newly eligible. The
    /// coordinator derives exact role assignments from the authoritative
    /// scheduler; this boolean is deliberately not a second routing policy.
    pub(super) fn compute_advanced(self) -> bool {
        let compute_slot_released = self.after.active_work < self.before.active_work;
        self.resolve_advanced()
            || self.verify_small_advanced()
            || self.verify_any_advanced()
            || (compute_slot_released
                && (self.after.scheduler.resolve.is_some()
                    || self.after.scheduler.verify_small.is_some()
                    || self.after.scheduler.verify_any.is_some()))
    }

    pub(super) fn ready_advanced(self) -> bool {
        Self::head_advanced(self.before.scheduler.ready, self.after.scheduler.ready)
    }

    pub(super) fn dependency_maintenance_activated(self) -> bool {
        self.dependency_maintenance_activated || self.dependency_poisoned
    }

    pub(super) fn effect_publisher_advanced(self) -> bool {
        self.after
            .effects
            .publisher_advanced_from(self.before.effects)
    }

    pub(super) fn effect_capacity_released(self) -> bool {
        self.after
            .effects
            .capacity_released_from(self.before.effects)
    }

    pub(super) fn owner_source_advanced(self) -> bool {
        self.template_source_changed
    }
}

#[derive(Debug)]
#[must_use = "a committed delta owns post-Apply retirement and timing evidence"]
pub(super) struct CommittedDelta {
    async_process_observations: AsyncProcessObservations,
    /// Removal observations remain available to production property tests and are destroyed
    /// with the retirement carrier before post-commit publication.
    pub(super) removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
    retired_effect: Option<Arc<EffectBatch>>,
    retired_generation: Option<RetiredGeneration>,
    post_commit_fault: Option<AuthorityFault>,
    wake: AuthorityWakeTransition,
}

struct ApplyRetirement {
    async_process_observations: AsyncProcessObservations,
    removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
    retired_effect: Option<Arc<EffectBatch>>,
    retired_generation: Option<RetiredGeneration>,
    dependency: Option<DependencyFinalization>,
    template_source_changed: bool,
}

/// Dependency publication produced by the canonical owner-removal compiler.
/// Keeping it paired with retirement makes it impossible for administration
/// or pipeline reset to keep the owner mutation while discarding the only
/// maintenance/poison receipt for the same Apply.
struct OwnerRemovalCommit {
    retired: RetiredOwners,
    dependency: DependencyFinalization,
}

fn finish_apply(
    authority: &TxPoolAuthority,
    before: AuthorityWakeProjection,
    compute_slot_released: bool,
    quiescent_projection: bool,
    retirement: ApplyRetirement,
) -> CommittedDelta {
    let after = authority.wake_projection();
    finish_apply_between(
        authority,
        before,
        after,
        compute_slot_released,
        quiescent_projection,
        retirement,
    )
}

fn finish_apply_without_scheduler_change(
    authority: &TxPoolAuthority,
    before: AuthorityWakeProjection,
    compute_slot_released: bool,
    quiescent_projection: bool,
    retirement: ApplyRetirement,
) -> CommittedDelta {
    let after = authority.wake_projection_without_scheduler();
    finish_apply_between(
        authority,
        before,
        after,
        compute_slot_released,
        quiescent_projection,
        retirement,
    )
}

fn finish_effect_only_apply(
    authority: &TxPoolAuthority,
    effect: super::effect::EffectWakeTransition,
    retirement: ApplyRetirement,
) -> CommittedDelta {
    let projection = authority.wake_projection_without_scheduler();
    let (effect_before, effect_after) = effect.projections();
    finish_apply_between(
        authority,
        projection.with_effects(effect_before),
        projection.with_effects(effect_after),
        false,
        false,
        retirement,
    )
}

fn finish_apply_between(
    _authority: &TxPoolAuthority,
    mut before: AuthorityWakeProjection,
    after: AuthorityWakeProjection,
    compute_slot_released: bool,
    quiescent_projection: bool,
    retirement: ApplyRetirement,
) -> CommittedDelta {
    if quiescent_projection {
        #[cfg(test)]
        {
            let inconsistencies = _authority.primary_projection_inconsistencies();
            assert!(
                inconsistencies.is_empty(),
                "every exclusively committed Apply must preserve the production owner/projection relation: {inconsistencies:?}"
            );
        }
    }
    before.active_work = usize::from(compute_slot_released);
    let ApplyRetirement {
        async_process_observations,
        removals,
        retired,
        retired_effect,
        retired_generation,
        dependency,
        template_source_changed,
    } = retirement;
    let (dependency_maintenance_activated, dependency_poisoned) = match dependency {
        None | Some(DependencyFinalization::Quiet) => (false, false),
        Some(DependencyFinalization::Activated) => (true, false),
        Some(DependencyFinalization::Poisoned) => (false, true),
    };
    CommittedDelta {
        async_process_observations,
        removals,
        retired,
        retired_effect,
        retired_generation,
        post_commit_fault: dependency_poisoned.then_some(AuthorityFault::DependencyProjection),
        wake: AuthorityWakeTransition {
            before,
            after,
            dependency_maintenance_activated,
            dependency_poisoned,
            template_source_changed: template_source_changed || before.template != after.template,
        },
    }
}

/// Lock-external capability produced only after every retired authority value
/// has been destroyed. The runtime must consume it to publish derived wake
/// hints and asynchronous timing evidence.
#[must_use = "a post-commit receipt must be published after retirement"]
pub(super) struct AuthorityPostCommit {
    async_process_observations: AsyncProcessObservations,
    post_commit_fault: Option<AuthorityFault>,
    wake: AuthorityWakeTransition,
}

#[derive(Debug)]
struct RetiredGeneration {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the map is an ownership carrier whose delayed Drop is the production behavior"
        )
    )]
    entries: ShardedOwnerMap,
    _resources: ResourceLedger,
    _scheduler: Arc<Mutex<FairFrontier>>,
    _dependencies: DependencyFrontier,
}

/// Owners removed by Apply are destroyed only after every authority guard is
/// released. The first slot is inline, so the common single-owner path adds no
/// heap allocation; Plan reserves every additional slot before acquisition.
#[derive(Debug, Default)]
struct RetiredOwners {
    first: Option<OwnedTx>,
    rest: Vec<OwnedTx>,
}

impl RetiredOwners {
    fn is_empty(&self) -> bool {
        self.first.is_none() && self.rest.is_empty()
    }

    fn try_with_capacity(capacity: usize) -> Result<Self, PlanError> {
        let mut rest = Vec::new();
        rest.try_reserve(capacity.saturating_sub(1))
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        Ok(Self { first: None, rest })
    }

    fn push(&mut self, owner: OwnedTx) {
        if self.first.is_none() {
            self.first = Some(owner);
        } else {
            self.rest.push(owner);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        usize::from(self.first.is_some()).saturating_add(self.rest.len())
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        1usize.saturating_add(self.rest.capacity())
    }
}

impl CommittedDelta {
    fn with_effect_wake(mut self, effect: super::effect::EffectWakeTransition) -> Self {
        self.wake = self.wake.with_effect_wake(effect);
        self
    }

    /// Destroy all potentially large retired values after the authority guard
    /// opens, then return the only capability that may publish this Apply's
    /// derived wake and timing observations.
    pub(in crate::authority) fn into_post_commit(self) -> AuthorityPostCommit {
        let Self {
            async_process_observations,
            removals,
            retired,
            retired_effect,
            retired_generation,
            post_commit_fault,
            wake,
        } = self;
        drop(removals);
        drop(retired);
        drop(retired_effect);
        drop(retired_generation);
        AuthorityPostCommit {
            async_process_observations,
            post_commit_fault,
            wake,
        }
    }

    /// Scratch-generation Applies are not externally published. Consume their
    /// retirement locally while retaining the one dependency fact that must
    /// become visible when the completed generation is swapped into service.
    fn into_scratch_dependency_finalization(self) -> DependencyFinalization {
        let Self {
            async_process_observations: _,
            removals,
            retired,
            retired_effect,
            retired_generation,
            post_commit_fault: _,
            wake,
        } = self;
        drop(removals);
        drop(retired);
        drop(retired_effect);
        drop(retired_generation);
        if wake.dependency_poisoned {
            DependencyFinalization::Poisoned
        } else if wake.dependency_maintenance_activated {
            DependencyFinalization::Activated
        } else {
            DependencyFinalization::Quiet
        }
    }
}

impl AuthorityPostCommit {
    /// Publish the legacy asynchronous processing histogram only from the
    /// closed receipt of a successful membership Apply. Timing evidence is
    /// removed from Accepted ownership before this receipt is built, so a
    /// stale plan, retry, cancellation or journal replay cannot double-count.
    pub(in crate::authority) fn publish_metrics_and_take_wake(
        self,
    ) -> (AuthorityWakeTransition, Option<AuthorityFault>) {
        let Some(metrics) = ckb_metrics::handle() else {
            return (self.wake, self.post_commit_fault);
        };
        let mut observe = |started_at: &AsyncProcessStart| {
            metrics
                .ckb_tx_pool_async_process
                .observe(started_at.elapsed_seconds());
        };
        match &self.async_process_observations {
            AsyncProcessObservations::None => {}
            #[cfg(any(test, feature = "internal"))]
            AsyncProcessObservations::One(started_at) => observe(started_at),
            AsyncProcessObservations::Batch(started_at) => {
                started_at.iter().for_each(&mut observe);
            }
        }
        (self.wake, self.post_commit_fault)
    }
}

struct EntryDelta {
    key: RawTxHash,
    expected: OwnerPrestate,
    after: Option<OwnedTx>,
    owners: DerivedOwnerDelta,
    retired: RetiredOwners,
    resource: ResourcePlan,
    scheduler: SchedulerDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

impl EntryDelta {
    fn into_shared_settlement(
        self,
        authority: &TxPoolAuthority,
        settlement_evidence: SettlementDependencyEvidence,
    ) -> Result<(IndependentDelta, ShardApplySupport), PlanError> {
        self.into_shared_entry(authority, Some(settlement_evidence))
    }

    fn into_shared_maintenance(
        self,
        authority: &TxPoolAuthority,
        maintenance: DependencyMaintenancePlan,
        maintenance_evidence: Option<SettlementDependencyEvidence>,
    ) -> Result<(IndependentDelta, ShardApplySupport), PlanError> {
        self.into_shared_entry_with_maintenance(authority, Some(maintenance), maintenance_evidence)
    }

    fn into_shared_entry(
        self,
        authority: &TxPoolAuthority,
        dependency_evidence: Option<SettlementDependencyEvidence>,
    ) -> Result<(IndependentDelta, ShardApplySupport), PlanError> {
        self.into_shared_entry_with_maintenance(authority, None, dependency_evidence)
    }

    fn into_shared_entry_with_maintenance(
        self,
        authority: &TxPoolAuthority,
        maintenance: Option<DependencyMaintenancePlan>,
        dependency_evidence: Option<SettlementDependencyEvidence>,
    ) -> Result<(IndependentDelta, ShardApplySupport), PlanError> {
        self.into_shared_entry_with_controls(
            authority,
            maintenance,
            dependency_evidence,
            ProjectionDelta::empty(),
            false,
        )
    }

    fn into_shared_terminalization(
        self,
        authority: &TxPoolAuthority,
        dependency_evidence: Option<SettlementDependencyEvidence>,
        policy_witness: MembershipPolicyWitness,
    ) -> Result<IndependentDelta, PlanError> {
        let (delta, _support) = self.into_shared_entry_with_controls(
            authority,
            None,
            dependency_evidence,
            ProjectionDelta::empty().with_read_witness(policy_witness),
            true,
        )?;
        Ok(delta)
    }

    fn into_shared_entry_with_controls(
        self,
        authority: &TxPoolAuthority,
        maintenance: Option<DependencyMaintenancePlan>,
        dependency_evidence: Option<SettlementDependencyEvidence>,
        projection: ProjectionDelta,
        stage_effect: bool,
    ) -> Result<(IndependentDelta, ShardApplySupport), PlanError> {
        let Self {
            key,
            expected,
            after,
            owners,
            retired,
            resource,
            scheduler,
            dependency,
            effect,
            clocks,
        } = self;
        if !stage_effect && !effect.is_empty() {
            return Err(PlanError::Fault(AuthorityFault::EffectProjection));
        }
        let effect = if stage_effect {
            effect
        } else {
            EffectDelta::default()
        };
        let mut owner_cuts = Vec::new();
        owner_cuts
            .try_reserve_exact(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        owner_cuts.push(IndependentOwnerCut {
            key,
            expected,
            action: IndependentOwnerAction::Replace(after),
        });
        let scheduler = scheduler.into_shared_batch().map_err(PlanError::from)?;
        let dependency = match (maintenance.as_ref(), dependency_evidence) {
            (Some(maintenance), Some(evidence)) => {
                dependency.with_history_maintenance_evidence(evidence, maintenance)
            }
            (_, evidence) => dependency.with_settlement_evidence(evidence, &authority.dependencies),
        }
        .map_err(PlanError::from)?;
        let dependency = match maintenance {
            Some(maintenance) => dependency.with_control(
                DependencyControlDelta::Maintenance(maintenance),
                &authority.dependencies,
            ),
            None => Ok(dependency),
        }
        .map_err(PlanError::from)?;
        let delta = IndependentDelta {
            owner_cuts,
            owners,
            resource: Some(resource.into_batch()),
            projection,
            scheduler,
            dependency,
            effect,
            clocks,
            async_process_starts: Vec::new(),
            removals: Vec::new(),
            retired,
        };
        let support = delta.physical_support(authority);
        Ok((delta, support))
    }
}

#[cfg(test)]
impl EntryDelta {
    fn shard_support(
        &self,
    ) -> (
        super::shard_support::AuthorityShardSupport,
        super::shard_support::ExclusiveSupport,
    ) {
        let mut support = super::shard_support::AuthorityShardSupport::default();
        let mut exclusive = super::shard_support::ExclusiveSupport::default();
        support.insert(b"owner-resource/owner", &self.key);
        self.owners.indexes.extend_shard_support(&mut support);
        self.resource
            .extend_shard_support(&mut support, &mut exclusive);
        self.scheduler
            .extend_shard_support(&mut support, &mut exclusive);
        self.dependency
            .extend_shard_support(&mut support, &mut exclusive);
        exclusive.effect_log = self.effect.has_exclusive_write();
        (support, exclusive)
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "this Plan-only value replaces an equally wide before/after option pair; boxing would add fallible allocation to every owner transition"
)]
enum EntryTransition {
    Insert {
        key: RawTxHash,
        after: OwnedTx,
    },
    Replace {
        key: RawTxHash,
        before: OwnedTx,
        after: OwnedTx,
    },
    Remove {
        key: RawTxHash,
        before: OwnedTx,
    },
}

struct DerivedOwnerDelta {
    indexes: IndexDelta,
    sources: SourceVersionDelta,
    template_sources: ShardOwnerSourcePlan,
}

struct TransitionControls {
    dependency: DependencyEntryControlDelta,
    effect: EffectDelta,
}

impl TransitionControls {
    fn none() -> Self {
        Self {
            dependency: DependencyEntryControlDelta::default(),
            effect: EffectDelta::default(),
        }
    }

    fn dependency(dependency: DependencyEntryControlDelta) -> Self {
        Self {
            dependency,
            ..Self::none()
        }
    }

    #[cfg(test)]
    fn effect(effect: EffectDelta) -> Self {
        Self {
            effect,
            ..Self::none()
        }
    }

    fn dependency_and_effect(dependency: DependencyEntryControlDelta, effect: EffectDelta) -> Self {
        Self { dependency, effect }
    }
}

enum CheckoutEligibility {
    Ready {
        grant: ComputeGrant,
        after_charge: ResourceVector,
    },
    StaleDependency,
}

struct DependencyLossKeys {
    keys: Vec<DependencyKey>,
    #[cfg(test)]
    work: DependencyLossWork,
}

struct MembershipDelta {
    changed_key: RawTxHash,
    changed_expected: OwnerPrestate,
    changed_after: OwnedTx,
    retired: RetiredOwners,
    removals: Vec<MembershipRemoval>,
    owners: DerivedOwnerDelta,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    scheduler: SchedulerDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    async_process_start: Option<AsyncProcessStart>,
}

/// Source-specific validation produces this immutable input; the shared
/// membership compiler then handles RBF, capacity, projections and effects
/// identically for asynchronous and direct admission.
struct MembershipCompilation {
    key: RawTxHash,
    existing: Option<OwnedTx>,
    accepted: AcceptedEntry,
    prepared: PreparedMembership,
    clocks: ApplyClockReservation,
    effects: MembershipEffects,
    async_process_start: Option<AsyncProcessStart>,
    resource: Option<ResourceBatchPlan>,
    dependency: Option<DependencyBatchDelta>,
    scheduler: Option<SchedulerDelta>,
    owners: Option<DerivedOwnerDelta>,
    sparse_resource: bool,
}

struct PreacceptedCandidateEvaluation {
    key: RawTxHash,
    existing: OwnedTx,
    accepted: AcceptedEntry,
    async_process_start: Option<AsyncProcessStart>,
    outcome: MembershipPolicyOutcome,
}

impl MembershipDelta {
    /// Lift one canonical Ready result whose evaluator never entered the
    /// population-wide capacity frontier. The policy witness supplies exact
    /// candidate and victim incarnations; every continuation owner, scheduler,
    /// dependency, projection and resource receipt remains in the existing
    /// IndependentDelta and is revalidated before the first owner mutation.
    fn into_shared_non_capacity(self) -> Result<IndependentDelta, PlanError> {
        if !self.projection.has_sparse_non_capacity_policy_witness()
            || self
                .removals
                .iter()
                .any(|removal| removal.cause == RemovalCause::Capacity)
            || !matches!(&self.changed_after, OwnedTx::Accepted(_))
        {
            return Err(PlanError::Stale(StalePlan::AcceptedObservation));
        }
        self.into_shared_exact_policy()
    }

    fn into_shared_capacity(self) -> Result<IndependentDelta, PlanError> {
        if !self.projection.has_capacity_frontier_policy_witness()
            || !self
                .removals
                .iter()
                .any(|removal| removal.cause == RemovalCause::Capacity)
            || !matches!(&self.changed_after, OwnedTx::Accepted(_))
        {
            return Err(PlanError::Stale(StalePlan::AcceptedObservation));
        }
        self.into_shared_exact_policy()
    }

    fn into_shared_exact(self) -> Result<IndependentDelta, PlanError> {
        if self.projection.has_capacity_frontier_policy_witness() {
            self.into_shared_capacity()
        } else {
            self.into_shared_non_capacity()
        }
    }

    fn into_shared_exact_policy(mut self) -> Result<IndependentDelta, PlanError> {
        let owner_count = self
            .removals
            .len()
            .checked_add(1)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut owner_cuts = Vec::new();
        owner_cuts
            .try_reserve_exact(owner_count)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for removal in &mut self.removals {
            let expected = self
                .projection
                .expected_accepted_version(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            owner_cuts.push(IndependentOwnerCut {
                key: removal.hash.clone(),
                expected: OwnerPrestate::Accepted(expected),
                action: IndependentOwnerAction::Replace(removal.take_after()),
            });
        }
        let expected = self.changed_expected;
        let expected_is_witnessed = match expected {
            OwnerPrestate::Vacant => self.projection.expected_owner_vacant(&self.changed_key),
            OwnerPrestate::PreAccepted(version) => {
                self.projection
                    .expected_preaccepted_version(&self.changed_key)
                    == Some(version)
            }
            OwnerPrestate::Accepted(version) => {
                self.projection.expected_accepted_version(&self.changed_key) == Some(version)
            }
            OwnerPrestate::ReplacementHistory(version) => {
                self.projection
                    .expected_replacement_history_version(&self.changed_key)
                    == Some(version)
            }
        };
        if !expected_is_witnessed {
            return Err(PlanError::Stale(StalePlan::AcceptedObservation));
        }
        owner_cuts.push(IndependentOwnerCut {
            key: self.changed_key,
            expected,
            action: IndependentOwnerAction::Replace(Some(self.changed_after)),
        });
        owner_cuts.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        let scheduler = self.scheduler.into_shared_batch()?;
        let mut async_process_starts = Vec::new();
        if let Some(start) = self.async_process_start {
            async_process_starts
                .try_reserve_exact(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            async_process_starts.push(start);
        }
        Ok(IndependentDelta {
            owner_cuts,
            owners: self.owners,
            resource: Some(self.resource),
            projection: self.projection,
            scheduler,
            dependency: self.dependency,
            effect: self.effect,
            clocks: self.clocks,
            async_process_starts,
            removals: self.removals,
            retired: self.retired,
        })
    }
}

/// Admission publication is a closed capability choice. Normal production
/// paths must publish their committed outcome; only the sealed feature-
/// internal `PlugEntry` fixture may choose the silent branch that preserves
/// its historical no-callback/no-relay contract.
enum MembershipEffects {
    Publish(EffectPolicy),
    #[cfg(any(test, feature = "internal"))]
    SilentInternal,
}

struct IndependentUpdate {
    key: RawTxHash,
    after: Option<OwnedTx>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "this Plan-only owner action keeps the move-owned replacement allocation-free through Apply; boxing would add one fallible allocation per owner cut"
)]
enum IndependentOwnerAction {
    Observe,
    Replace(Option<OwnedTx>),
}

struct IndependentOwnerCut {
    key: RawTxHash,
    expected: OwnerPrestate,
    action: IndependentOwnerAction,
}

#[derive(Clone, Copy)]
enum OwnerPrestate {
    Vacant,
    PreAccepted(EntryVersion),
    Accepted(EntryVersion),
    ReplacementHistory(EntryVersion),
}

impl OwnerPrestate {
    fn from_owner(owner: &OwnedTx) -> Self {
        match owner {
            OwnedTx::PreAccepted(entry) => Self::PreAccepted(entry.record.version),
            OwnedTx::Accepted(entry) => Self::Accepted(entry.record.version),
            OwnedTx::ReplacementHistory(entry) => Self::ReplacementHistory(entry.record().version),
        }
    }

    fn is_fresh(self, current: Option<&OwnedTx>) -> bool {
        match self {
            Self::Vacant => current.is_none(),
            Self::PreAccepted(expected) => matches!(
                current,
                Some(OwnedTx::PreAccepted(entry)) if entry.record.version == expected
            ),
            Self::Accepted(expected) => matches!(
                current,
                Some(OwnedTx::Accepted(entry)) if entry.record.version == expected
            ),
            Self::ReplacementHistory(expected) => matches!(
                current,
                Some(OwnedTx::ReplacementHistory(entry)) if entry.record().version == expected
            ),
        }
    }
}

/// One mechanically commuting membership batch. The ordinary disjoint
/// admission run and the strictly proven leaf-RBF cohort share this exact
/// delta and Apply; policy remains in the canonical membership evaluator.
pub(in crate::authority) struct IndependentDelta {
    owner_cuts: Vec<IndependentOwnerCut>,
    owners: DerivedOwnerDelta,
    resource: Option<ResourceBatchPlan>,
    projection: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    async_process_starts: Vec<AsyncProcessStart>,
    removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
}

impl IndependentDelta {
    fn is_pure_accepted(&self) -> bool {
        self.removals.is_empty()
            && self.scheduler.is_shared_acceptance_removal_only()
            && self.owner_cuts.iter().all(|owner| {
                matches!(
                    owner.action,
                    IndependentOwnerAction::Replace(Some(OwnedTx::Accepted(_)))
                )
            })
    }

    fn is_ready_phase_only(&self) -> bool {
        self.is_pure_accepted() && self.dependency.is_ready_phase_only_shape()
    }

    fn physical_write_support(&self, authority: &TxPoolAuthority) -> ShardWriteSupport {
        let mut support = authority.entries.owner_resource_write_support(
            self.owner_cuts.iter().filter_map(|owner| {
                matches!(owner.action, IndependentOwnerAction::Replace(_)).then_some(&owner.key)
            }),
            self.projection.proposed_count_plan(),
            self.resource.as_ref().map_or(
                &super::shard::ShardResourcePlan::empty(),
                ResourceBatchPlan::shard_plan,
            ),
        );
        support.include(
            self.owners
                .indexes
                .sharded_write_support(&authority.entries),
        );
        support.include(self.projection.sharded_write_support(&authority.entries));
        support.include(
            self.dependency
                .sharded_owner_commit_write_support(&authority.entries),
        );
        support
    }

    fn physical_support(&self, authority: &TxPoolAuthority) -> ShardApplySupport {
        let mut reads = if self.is_ready_phase_only() {
            self.dependency
                .ready_phase_final_read_support(&authority.entries)
        } else {
            self.dependency.sharded_read_support(&authority.entries)
        };
        reads.include(self.projection.sharded_read_support(&authority.entries));
        for owner in &self.owner_cuts {
            if matches!(owner.action, IndependentOwnerAction::Observe) {
                reads.insert(authority.entries.owner_shard(&owner.key));
            }
        }
        ShardApplySupport::new(reads, self.physical_write_support(authority))
    }
}

struct EffectOnlyDelta {
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

#[derive(Default)]
struct FreshDependencyPublication {
    maintenance_activated: bool,
}

impl FreshDependencyPublication {
    fn absorb(&mut self, outcome: DependencyFinalization) -> Result<(), PlanError> {
        match outcome {
            DependencyFinalization::Quiet => Ok(()),
            DependencyFinalization::Activated => {
                self.maintenance_activated = true;
                Ok(())
            }
            DependencyFinalization::Poisoned => {
                Err(PlanError::Fault(AuthorityFault::DependencyProjection))
            }
        }
    }

    fn into_finalization(self) -> DependencyFinalization {
        if self.maintenance_activated {
            DependencyFinalization::Activated
        } else {
            DependencyFinalization::Quiet
        }
    }
}

struct FreshGeneration {
    entries: ShardedOwnerMap,
    resources: ResourceLedger,
    scheduler: Arc<Mutex<FairFrontier>>,
    dependencies: DependencyFrontier,
    dependency_publication: FreshDependencyPublication,
}

impl FreshGeneration {
    fn empty(
        resources: &ResourceLedger,
        scheduler: &Arc<Mutex<FairFrontier>>,
        entries: &ShardedOwnerMap,
    ) -> Self {
        // This private map is only the prebuilt carrier for generation-owned
        // shard payloads. Apply swaps those payloads into the persistent live
        // routed locks; peer fences never leave the live layout and therefore
        // require no copy, reservation, or owner-population scan.
        let entries = ShardedOwnerMap::new(entries.router());
        let dependencies = DependencyFrontier::for_entries(
            &entries,
            resources.limits().max_dependency_stage_units(),
        );
        Self {
            entries,
            resources: ResourceLedger::new(resources.limits()),
            scheduler: Arc::new(Mutex::new(FairFrontier::new(
                scheduler.lock().verify_order(),
            ))),
            dependencies,
            dependency_publication: FreshDependencyPublication::default(),
        }
    }

    fn preaccepted_active_work(&self) -> usize {
        self.resources.read(&self.entries).preaccepted().active_work
    }
}

struct ClearPoolDelta {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    fresh: FreshGeneration,
    sources: SourceVersionDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    compute_slot_released: bool,
}

struct ClearPipelineDelta {
    generation: PoolGeneration,
    removal: OwnerRemovalBatch,
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

#[cfg(test)]
struct AdminDelta {
    marker: PeerBanDelta,
    removal: OwnerRemovalBatch,
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

/// Unique owner-removal input whose caller-selected order is preserved.
/// Duplicate rejection happens once before cause-specific effects or derived
/// deltas are compiled; truncation of a prefix preserves the proof.
struct OwnerRemovalKeys(Vec<RawTxHash>);

impl OwnerRemovalKeys {
    fn new(hashes: Vec<RawTxHash>) -> Result<Self, PlanError> {
        let mut unique = HashSet::new();
        unique
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in &hashes {
            if !unique.insert(hash) {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        }
        Ok(Self(hashes))
    }

    fn into_inner(self) -> Vec<RawTxHash> {
        self.0
    }
}

impl std::ops::Deref for OwnerRemovalKeys {
    type Target = [RawTxHash];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Complete authoritative and derived transition for a set of owners moving
/// to Nowhere. Administrative causes and generation control reuse this one
/// compiler, so resource release, membership removal, dependency publication,
/// source versions, and retirement cannot acquire separate manual maps.
struct OwnerRemovalBatch {
    hashes: Vec<RawTxHash>,
    expected_versions: Vec<EntryVersion>,
    owners: DerivedOwnerDelta,
    resources: OwnerRemovalResourcePlan,
    membership: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    retired: RetiredOwners,
}

/// Owner snapshots and the optional administrative-closure seal are one
/// evidence carrier: a caller cannot accidentally pair the closure witness
/// with a different owner population at the canonical removal compiler.
struct OwnerRemovalSnapshots {
    owners: Vec<OwnedTx>,
    closure: Option<AdministrativeClosureWitness>,
}

/// Fully compiled owner-removal transition whose fallible live projection
/// staging has not begun. Compile owns only bounded deltas and exact clocks;
/// Bind installs hidden scheduler/dependency rows and an optional effect row;
/// Apply consumes the final physical cut without allocation.
struct CompiledSharedOwnerRemoval {
    removal: OwnerRemovalBatch,
    publication: Option<EffectPublication>,
    sequence: ApplySequence,
    clocks: AuthorityClocks,
}

#[must_use = "a bound shared owner removal must commit its exact cut or roll back every hidden row"]
struct PreparedSharedOwnerRemoval<'authority> {
    authority: &'authority TxPoolAuthority,
    removal: OwnerRemovalBatch,
    projections: ingress::StagedRetainedIngress<'authority>,
    staged_effect: Option<super::effect::StagedEffect>,
    clocks: AuthorityClocks,
}

/// Remote expiry Plan output. It deliberately owns no live scheduler gate so
/// an earlier due insertion can commit before Bind and make the witness stale.
#[must_use = "compiled Remote expiry must bind to the live generation or be discarded"]
pub(super) struct CompiledSharedRemoteExpiry {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    removal: CompiledSharedOwnerRemoval,
    witness: RemoteExpiryWitness,
}

#[must_use = "prepared Remote expiry must apply its exact prefix or roll back every staged row"]
pub(super) struct PreparedSharedRemoteExpiry<'authority> {
    removal: PreparedSharedOwnerRemoval<'authority>,
    witness: RemoteExpiryWitness,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedBackingParent {
    hash: RawTxHash,
    version: EntryVersion,
}

/// Cause-neutral evidence for removing one Accepted administrative closure.
/// Every caller shares the same closure, owner, backing-parent and projection
/// freshness proof; a cause may add evidence but cannot replace this carrier.
struct AdministrativeRemovalControl {
    parents: Vec<AcceptedBackingParent>,
}

/// Complete Accepted administrative capture consumed by the canonical owner
/// removal compiler. Keeping these fields together prevents a caller from
/// pairing one closure witness with another owner or released-input set.
struct AcceptedAdministrativeRemoval {
    hashes: OwnerRemovalKeys,
    accepted_removals: AcceptedRemovalSet,
    released: Vec<DependencyKey>,
    snapshots: OwnerRemovalSnapshots,
    control: AdministrativeRemovalControl,
}

/// Accepted-expiry evidence that is not already owned by OwnerRemovalBatch.
/// The selected root binds the cause to its immutable deadline, while every
/// pool-backed released input names the exact surviving parent incarnation.
struct AcceptedExpiryControl {
    head: AcceptedExpiryHead,
    administrative: AdministrativeRemovalControl,
}

#[must_use = "compiled Accepted expiry must bind to the live generation or be discarded"]
pub(super) struct CompiledSharedAcceptedExpiry {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    removal: CompiledSharedOwnerRemoval,
    control: AcceptedExpiryControl,
}

#[must_use = "prepared Accepted expiry must apply its exact closure or roll back every staged row"]
pub(super) struct PreparedSharedAcceptedExpiry<'authority> {
    removal: PreparedSharedOwnerRemoval<'authority>,
    control: AcceptedExpiryControl,
}

#[must_use = "compiled local removal must bind to the captured generation or be discarded"]
pub(super) struct CompiledSharedLocalRemoval {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    removal: CompiledSharedOwnerRemoval,
    control: AdministrativeRemovalControl,
}

#[must_use = "prepared local removal must apply its exact cut or roll back every staged row"]
pub(super) struct PreparedSharedLocalRemoval<'authority> {
    removal: PreparedSharedOwnerRemoval<'authority>,
    control: AdministrativeRemovalControl,
}

struct ChainOwnerUpdate {
    key: RawTxHash,
    after: Option<OwnedTx>,
}

struct ChainDelta {
    view: ChainViewId,
    updates: Vec<ChainOwnerUpdate>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    membership: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    retired: RetiredOwners,
    clocks: AuthorityClocks,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the prepared authority transition stays allocation-free after Plan; boxing a large arm would move a fixed semantic delta behind an infallible allocation"
)]
enum DependencyAuthorityDelta {
    Entry(EntryDelta),
    #[cfg(any(test, feature = "internal"))]
    Membership(MembershipDelta),
    ClearPipeline(ClearPipelineDelta),
    #[cfg(test)]
    Admin(AdminDelta),
    Chain(ChainDelta),
}

impl DependencyAuthorityDelta {
    fn releases_preaccepted_active_work(&self) -> bool {
        match self {
            Self::Entry(delta) => delta.resource.releases_preaccepted_active_work(),
            #[cfg(any(test, feature = "internal"))]
            Self::Membership(delta) => delta.resource.releases_preaccepted_active_work(),
            Self::ClearPipeline(delta) => {
                delta.removal.resources.releases_preaccepted_active_work()
            }
            #[cfg(test)]
            Self::Admin(delta) => delta.removal.resources.releases_preaccepted_active_work(),
            Self::Chain(delta) => delta.resources.releases_preaccepted_active_work(),
        }
    }

    fn take_dependency(&mut self) -> Option<DependencyBatchDelta> {
        match self {
            Self::Entry(delta) => Some(std::mem::take(&mut delta.dependency)),
            #[cfg(any(test, feature = "internal"))]
            Self::Membership(delta) => Some(std::mem::take(&mut delta.dependency)),
            Self::ClearPipeline(delta) => Some(std::mem::take(&mut delta.removal.dependency)),
            #[cfg(test)]
            Self::Admin(delta) => Some(std::mem::take(&mut delta.removal.dependency)),
            Self::Chain(delta) => Some(std::mem::take(&mut delta.dependency)),
        }
    }
}

enum PlainAuthorityDelta {
    Effect(EffectOnlyDelta),
    ClearPool(Box<ClearPoolDelta>),
}

impl PlainAuthorityDelta {
    fn releases_preaccepted_active_work(&self) -> bool {
        match self {
            Self::Effect(_) => false,
            Self::ClearPool(delta) => delta.compute_slot_released,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MissingResolutionDisposition {
    Wait,
    Reject(SettlementRejection),
}

#[derive(Clone, Copy)]
enum ProjectedRemovalSet<'set> {
    Replacement(&'set HashSet<RawTxHash>),
    Administrative(&'set AcceptedRemovalSet),
}

impl ProjectedRemovalSet<'_> {
    fn contains(self, hash: &RawTxHash) -> bool {
        match self {
            Self::Replacement(removed) => removed.contains(hash),
            Self::Administrative(removed) => removed.contains(hash),
        }
    }
}

#[derive(Clone, Copy)]
struct ProjectedFinalOwnerSet<'set> {
    removed: ProjectedRemovalSet<'set>,
}

impl ProjectedFinalOwnerSet<'_> {
    fn contains_removed(self, hash: &RawTxHash) -> bool {
        self.removed.contains(hash)
    }
}

#[derive(Clone, Copy)]
enum ReleasedInputContext<'input> {
    Replacement {
        candidate_inputs: &'input HashSet<OutPoint>,
    },
    Administrative {
        victim: &'input RawTxHash,
    },
}

enum ReleasedInputBacking {
    Unavailable,
    Chain,
    Pool(AcceptedBackingParent),
}

impl ReleasedInputBacking {
    fn is_available(&self) -> bool {
        !matches!(self, Self::Unavailable)
    }
}

struct AdministrativeReleasedInputs {
    keys: Vec<DependencyKey>,
    parents: Vec<AcceptedBackingParent>,
}

enum SettlementDisposition {
    Retain {
        phase: PreAcceptedPhase,
        charge: ResourceVector,
    },
    RetainAndPublish {
        phase: PreAcceptedPhase,
        charge: ResourceVector,
        publication: EffectPublication,
    },
    Terminal(SettlementRejection),
}

struct SettlementPolicy {
    existing: OwnedTx,
    dependency: SettlementDependencyEvidence,
    disposition: SettlementDisposition,
    recovery_next: SettlementNext,
}

struct OwnerLocalSettlement {
    phase: OwnerLocalPhase,
    charge: ResourceVector,
}

enum OwnerLocalPhase {
    Resolve,
    Verify(ResolvedFacts),
    Ready(VerifiedFacts),
}

impl OwnerLocalPhase {
    fn into_preaccepted(self) -> PreAcceptedPhase {
        match self {
            Self::Resolve => PreAcceptedPhase::Queued(QueuedWork::Resolve),
            Self::Verify(resolved) => PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
            Self::Ready(verified) => PreAcceptedPhase::Ready(verified),
        }
    }

    fn into_exact_next(self) -> SettlementNext {
        match self {
            Self::Resolve => SettlementNext::Retry,
            Self::Verify(resolved) => SettlementNext::QueuedVerify(resolved),
            Self::Ready(verified) => SettlementNext::Ready(verified),
        }
    }
}

impl OwnerLocalSettlement {
    /// Recover the exact worker result from the owner-local normal form when
    /// shared retained capacity requires the single-settlement planner. That
    /// planner already owns the typed accept-or-resource-reject decision.
    fn into_exact_next(self) -> SettlementNext {
        self.phase.into_exact_next()
    }
}

fn settlement_dependency_inputs(
    next: &SettlementNext,
) -> (Option<&KnownDependencies>, Option<&MissingDependencies>) {
    match next {
        SettlementNext::QueuedVerify(resolved) => (Some(resolved.payload().dependencies()), None),
        SettlementNext::Waiting(missing) => (Some(missing.dependencies()), Some(missing.missing())),
        SettlementNext::Ready(verified) => (Some(verified.payload().dependencies()), None),
        SettlementNext::VerificationRejected { resolved, .. } => {
            (Some(resolved.payload().dependencies()), None)
        }
        SettlementNext::Rejected(_) | SettlementNext::Retry => (None, None),
    }
}

enum SettlementClassification {
    OwnerLocal(OwnerLocalSettlement),
    NonLocal(NonLocalSettlement),
}

enum NonLocalSettlement {
    Waiting(MissingResolution),
    Rejected(SettlementRejection),
    VerificationRejected {
        rejection: CommittedPublicReject,
        resolved: ResolvedFacts,
    },
}

enum PrepareSettlementError {
    Recompute(PlanError),
    Preserve {
        error: PlanError,
        next: SettlementNext,
    },
}

impl From<PlanError> for PrepareSettlementError {
    fn from(error: PlanError) -> Self {
        Self::Recompute(error)
    }
}

struct PreparedDependencyApply {
    delta: DependencyAuthorityDelta,
    staged: StagedDependencyBatch,
}

enum PreparedApplyKind {
    Dependency(Box<PreparedDependencyApply>),
    Plain(PlainAuthorityDelta),
}

#[must_use = "a prepared authority transition has no effect until explicitly applied"]
pub(super) struct PreparedApply<'authority> {
    authority: &'authority mut TxPoolAuthority,
    kind: PreparedApplyKind,
}

/// The only ordinary membership batch eligible for the future shared Apply
/// cut. Keeping its carrier distinct from [`PreparedApply`] makes it
/// impossible for chain, generation, administration, or coupled membership
/// deltas to acquire shared-commit authority by changing a runtime tag.
///
#[must_use = "a prepared independent authority transition has no effect until explicitly applied"]
pub(in crate::authority) enum PreparedIndependentApply<'authority> {
    Shared {
        authority: &'authority TxPoolAuthority,
        delta: IndependentDelta,
        support: ShardApplySupport,
        staged_effect: Option<super::effect::StagedEffect>,
    },
    #[cfg(test)]
    Exclusive {
        authority: &'authority mut TxPoolAuthority,
        delta: IndependentDelta,
    },
}

/// Move-owned result of coherent exclusive planning, not yet authorized to
/// mutate a live generation. Runtime releases the planning write guard, then
/// binds this value under the shared generation barrier before exact-shard
/// OCC. A chain/generation writer in between makes binding stale and dropping
/// this value returns every resource/effect reservation.
#[must_use = "compiled shared work must bind to its exact generation or be discarded stale"]
pub(in crate::authority) struct CompiledSharedIndependent {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    delta: IndependentDelta,
    support: ShardApplySupport,
    staged_effect: super::effect::StagedEffect,
}

/// The only effect-free Ready-head transition: a validation-rules mismatch
/// returns one exact owner to Resolve. Keeping it distinct from
/// [`CompiledSharedIndependent`] makes a missing acceptance/rejection effect
/// unrepresentable for every other Ready disposition.
#[must_use = "compiled Ready re-resolution must consume its exact reservation or be discarded stale"]
pub(in crate::authority) struct CompiledSharedReadyReresolution {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    delta: IndependentDelta,
    support: ShardApplySupport,
}

/// Closed shared implementation of a non-candidate Ready head. Every variant
/// was produced by the same final validator as the legacy exclusive command;
/// runtime can only commit it with the captured Ready reservation.
#[must_use = "a shared Ready-head disposition must commit or return its exact reservation"]
#[expect(
    clippy::large_enum_variant,
    reason = "each arm owns its already-preallocated semantic delta through Apply; boxing would add a fallible allocation after final validation"
)]
pub(in crate::authority) enum PreparedSharedReadyHeadDisposition<'authority> {
    Effectful {
        authority: &'authority TxPoolAuthority,
        compiled: CompiledSharedIndependent,
    },
    Reresolve {
        authority: &'authority TxPoolAuthority,
        compiled: CompiledSharedReadyReresolution,
    },
    PeerRevocation(ingress::PreparedSharedPeerRevocationCore<'authority>),
}

#[must_use = "committed Ready rows must receive their staged effect terminal and wake receipt"]
pub(in crate::authority) struct ReadyCommittedRows {
    before: AuthorityWakeProjection,
    compute_slot_released: bool,
    resource_health: ResourceCommitHealth,
    retirement: ApplyRetirement,
    scheduler_unchanged: bool,
}

/// An independent Apply whose owner rows are already irreversible. Callers
/// must publish `committed` before forwarding `post_commit_fault` to
/// supervision; representing those two facts together prevents a sibling
/// capacity fault from cancelling an effect for rows that did commit.
#[must_use = "publish the committed delta before forwarding any post-commit fault"]
pub(in crate::authority) struct CommittedSharedApply {
    committed: CommittedDelta,
    post_commit_fault: Option<AuthorityFault>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns preallocated retirement storage after irreversible owner mutation; boxing here would add a fallible post-commit allocation"
)]
pub(in crate::authority) enum ReadyJobCommitOutcome {
    Committed(CommittedSharedApply),
    Stale(super::effect::EffectWakeTransition),
    Fault {
        fault: AuthorityFault,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
}

/// Complete terminal of the non-candidate Ready-head route. Effect wake is
/// optional only for the effect-free re-resolution arm; validation rejection
/// and peer revocation still carry their exact staged-effect wake on failure.
#[must_use = "publish a committed Ready head or its exact rollback wake"]
#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns preallocated retirement after irreversible owner mutation; boxing would add a post-commit allocation"
)]
pub(in crate::authority) enum ReadyHeadCommitOutcome {
    Committed(CommittedSharedApply),
    Stale {
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
    Backpressure {
        pressure: Backpressure,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
    Fault {
        fault: AuthorityFault,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
}

#[must_use = "a shared Direct commit must publish its committed rows or exact staged-effect rollback wake"]
#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns preallocated retirement after irreversible owner mutation; boxing would add a post-commit allocation"
)]
pub(super) enum SharedDirectCommitOutcome {
    Committed(CommittedSharedApply),
    Stale(super::effect::EffectWakeTransition),
    Fault {
        fault: AuthorityFault,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
}

enum DirectMembershipEffectResult {
    Duplicate(RawTxHash),
    Rejected(MembershipReject),
}

/// Complete shared Local candidate disposition. Accepted candidates reuse the
/// canonical membership `IndependentDelta`; duplicate and policy rejection
/// hold their exact membership read fence through the sole effect activation.
#[must_use = "a prepared Direct disposition must commit or roll back its exact effect"]
#[expect(
    clippy::large_enum_variant,
    reason = "the accepted arm owns one already-preallocated canonical membership delta; boxing would add a fallible allocation after validation"
)]
pub(super) enum PreparedSharedDirectAdmissionDisposition<'authority> {
    Accepted {
        authority: &'authority TxPoolAuthority,
        compiled: CompiledSharedIndependent,
    },
    EffectOnly(PreparedSharedDirectMembershipEffect<'authority>),
}

#[must_use = "publish the committed Direct disposition or its exact rollback wake"]
pub(super) enum SharedDirectAdmissionCommitOutcome {
    Accepted(CommittedSharedApply),
    Duplicate {
        key: RawTxHash,
        committed: CommittedSharedApply,
    },
    Rejected {
        reason: MembershipReject,
        committed: CommittedSharedApply,
    },
    Stale {
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
    Fault {
        fault: AuthorityFault,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
}

/// Effect-only Direct membership terminal. Its exact policy witness stays
/// read-locked through activation because no owner mutation exists to
/// linearize a duplicate or rejection decision first.
#[must_use = "a Direct membership effect must activate under its read fence or roll back"]
pub(super) struct PreparedSharedDirectMembershipEffect<'authority> {
    authority: &'authority TxPoolAuthority,
    result: DirectMembershipEffectResult,
    read_witness: MembershipPolicyWitness,
    staged_effect: super::effect::StagedEffect,
    clocks: AuthorityClocks,
}

/// A fully staged owner-free Direct rejection bound to the authority borrowed
/// through the runtime's shared generation/chain guard. The only remaining
/// operation is the exact Accepted read-fence check followed by activation in
/// the sole EffectLog, or an explicit staged rollback.
#[must_use = "a shared Direct rejection terminal must activate or roll back its staged effect"]
pub(super) struct PreparedSharedDirectRejectionTerminal<'authority> {
    authority: &'authority TxPoolAuthority,
    reason: CommittedPublicReject,
    read_witness: DirectRejectionReadWitness,
    staged_effect: super::effect::StagedEffect,
    clocks: AuthorityClocks,
}

/// One immutable description of the complete read fence shared by both the
/// pre-stage semantic check and the final OCC check. The support is derived
/// once from the sealed overlay; both checks therefore use the same chain and
/// producer/spender relation instead of duplicating a route decision.
struct DirectRejectionReadWitness {
    validity: DirectRejectionValidity,
    support: ShardReadSupport,
}

/// A successful witness binding. `accepted` is `None` only for stable ingress
/// facts; otherwise it keeps every exact Accepted premise locked through
/// staged-effect activation.
struct BoundDirectRejectionReadFence<'authority> {
    _accepted: Option<ShardedOwnerWriteCut<'authority>>,
}

impl DirectRejectionReadWitness {
    fn capture(
        authority: &TxPoolAuthority,
        validity: DirectRejectionValidity,
    ) -> Result<Self, PlanError> {
        let support = match &validity {
            DirectRejectionValidity::Stable => ShardReadSupport::default(),
            DirectRejectionValidity::AcceptedReads { reads, .. } => {
                reads.sharded_read_support(&authority.entries)
            }
        };
        let witness = Self { validity, support };
        let fence = witness.bind(authority).map_err(PlanError::Stale)?;
        drop(fence);
        Ok(witness)
    }

    fn bind<'authority>(
        &self,
        authority: &'authority TxPoolAuthority,
    ) -> Result<BoundDirectRejectionReadFence<'authority>, StalePlan> {
        match &self.validity {
            DirectRejectionValidity::Stable => {
                Ok(BoundDirectRejectionReadFence { _accepted: None })
            }
            DirectRejectionValidity::AcceptedReads { view, reads } => {
                if view != &authority.chain_view {
                    return Err(StalePlan::ChainRevision);
                }
                let read_cut = authority
                    .entries
                    .mixed_cut(self.support, ShardWriteSupport::default());
                if !reads.is_current_in_cut(&authority.entries, &read_cut) {
                    return Err(StalePlan::AcceptedObservation);
                }
                Ok(BoundDirectRejectionReadFence {
                    _accepted: Some(read_cut),
                })
            }
        }
    }
}

/// Terminal result returned while the shared outer guard is still live. The
/// runtime releases that guard before publishing either the committed Apply
/// or the exact rollback/capacity wake.
#[must_use = "publish the committed rejection or its staged-effect rollback wake"]
#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm carries move-owned post-Apply retirement; boxing would add a fallible allocation after the sole journal record is already irreversible"
)]
pub(super) enum SharedDirectRejectionTerminalOutcome {
    Committed {
        reason: CommittedPublicReject,
        committed: CommittedSharedApply,
    },
    Failed {
        error: PlanError,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
}

impl PreparedSharedDirectAdmissionDisposition<'_> {
    pub(super) fn commit(self) -> SharedDirectAdmissionCommitOutcome {
        match self {
            Self::Accepted {
                authority,
                compiled,
            } => {
                #[cfg(test)]
                authority.entries.enter_shared_ingress_probe(
                    crate::authority::shard::SharedIngressProbePhase::DirectMembershipPreparedBeforeFinalCut,
                );
                match compiled.commit_direct(authority) {
                    SharedDirectCommitOutcome::Committed(committed) => {
                        SharedDirectAdmissionCommitOutcome::Accepted(committed)
                    }
                    SharedDirectCommitOutcome::Stale(effect_wake) => {
                        SharedDirectAdmissionCommitOutcome::Stale {
                            effect_wake: Some(effect_wake),
                        }
                    }
                    SharedDirectCommitOutcome::Fault { fault, effect_wake } => {
                        SharedDirectAdmissionCommitOutcome::Fault { fault, effect_wake }
                    }
                }
            }
            Self::EffectOnly(effect) => effect.commit(),
        }
    }
}

impl PreparedSharedDirectMembershipEffect<'_> {
    fn commit(self) -> SharedDirectAdmissionCommitOutcome {
        let Self {
            authority,
            result,
            read_witness,
            staged_effect,
            clocks,
        } = self;
        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
        );
        let read_fence = match read_witness.bind(authority) {
            Ok(fence) => fence,
            Err(_) => {
                let _reserved_clock_high_water = clocks;
                return Self::rollback(staged_effect);
            }
        };
        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionReadCutBeforeActivation,
        );
        let effect_wake = staged_effect.activate_with_wake();
        drop(read_fence);
        let _reserved_clock_high_water = clocks;
        let retirement = ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect: None,
            retired_generation: None,
            dependency: None,
            template_source_changed: false,
        };
        let committed = CommittedSharedApply::clean(finish_effect_only_apply(
            authority,
            effect_wake,
            retirement,
        ));
        match result {
            DirectMembershipEffectResult::Duplicate(key) => {
                SharedDirectAdmissionCommitOutcome::Duplicate { key, committed }
            }
            DirectMembershipEffectResult::Rejected(reason) => {
                SharedDirectAdmissionCommitOutcome::Rejected { reason, committed }
            }
        }
    }

    fn rollback(staged_effect: super::effect::StagedEffect) -> SharedDirectAdmissionCommitOutcome {
        match staged_effect.rollback_with_wake() {
            Ok(effect_wake) => SharedDirectAdmissionCommitOutcome::Stale {
                effect_wake: Some(effect_wake),
            },
            Err(_) => SharedDirectAdmissionCommitOutcome::Fault {
                fault: AuthorityFault::EffectProjection,
                effect_wake: None,
            },
        }
    }
}

impl PreparedSharedDirectRejectionTerminal<'_> {
    pub(super) fn apply(self) -> SharedDirectRejectionTerminalOutcome {
        let Self {
            authority,
            reason,
            read_witness,
            staged_effect,
            clocks,
        } = self;
        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
        );

        let read_fence = match read_witness.bind(authority) {
            Ok(fence) => fence,
            Err(stale) => {
                let _reserved_clock_high_water = clocks;
                return Self::rollback_failure(staged_effect, PlanError::Stale(stale));
            }
        };

        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionReadCutBeforeActivation,
        );
        // Activation occurs while every producer/spender premise remains
        // read-locked. The outer runtime read guard simultaneously prevents a
        // chain or generation replacement from changing the journal identity.
        let effect_wake = staged_effect.activate_with_wake();
        drop(read_fence);
        let _reserved_clock_high_water = clocks;
        let retirement = ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect: None,
            retired_generation: None,
            dependency: None,
            template_source_changed: false,
        };
        let committed = CommittedSharedApply::clean(finish_effect_only_apply(
            authority,
            effect_wake,
            retirement,
        ));
        SharedDirectRejectionTerminalOutcome::Committed { reason, committed }
    }

    fn rollback_failure(
        staged_effect: super::effect::StagedEffect,
        error: PlanError,
    ) -> SharedDirectRejectionTerminalOutcome {
        match staged_effect.rollback_with_wake() {
            Ok(effect_wake) => SharedDirectRejectionTerminalOutcome::Failed {
                error,
                effect_wake: Some(effect_wake),
            },
            Err(_) => SharedDirectRejectionTerminalOutcome::Failed {
                error: PlanError::Fault(AuthorityFault::EffectProjection),
                effect_wake: None,
            },
        }
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "both arms own bounded preallocated shared-Apply state; boxing would add a fallible allocation to every compute settlement"
)]
pub(in crate::authority) enum SharedComputeSettlementPreparation<'authority> {
    Prepared(PreparedSharedComputeSettlement<'authority>),
    PeerRevocation(PreparedSharedComputePeerRevocation<'authority>),
}

impl SharedComputeSettlementPreparation<'_> {
    pub(in crate::authority) fn apply(self) -> SharedComputeSettlementOutcome {
        match self {
            Self::Prepared(prepared) => prepared.apply(),
            Self::PeerRevocation(prepared) => prepared.apply(),
        }
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "both compiler arms carry bounded pre-owner state directly so a successful hot-path classification performs no extra heap allocation"
)]
enum SharedSettlementEntryCompilation<'authority> {
    Entry {
        delta: EntryDelta,
        dependency: SettlementDependencyEvidence,
        publication: Option<EffectPublication>,
        recovery_next: SettlementNext,
        sequence: ApplySequence,
    },
    PeerRevocation {
        core: ingress::PreparedSharedPeerRevocationCore<'authority>,
        recovery_next: SettlementNext,
    },
}

#[must_use = "a malformed compute result must commit its exact peer cohort or return its settlement recovery"]
pub(in crate::authority) struct PreparedSharedComputePeerRevocation<'authority> {
    core: ingress::PreparedSharedPeerRevocationCore<'authority>,
    recovery: ComputeSettlement,
}

#[must_use = "a shared compute settlement must commit or return its exact linear recovery"]
pub(in crate::authority) struct PreparedSharedComputeSettlement<'authority> {
    authority: &'authority TxPoolAuthority,
    delta: IndependentDelta,
    support: ShardApplySupport,
    staged_effect: Option<super::effect::StagedEffect>,
    recovery: ComputeSettlement,
}

#[must_use = "the runtime must publish a committed settlement or its exact rollback wake"]
#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns preallocated retirement after irreversible owner mutation; boxing would add a fallible post-commit allocation"
)]
pub(in crate::authority) enum SharedComputeSettlementOutcome {
    Committed(CommittedSharedApply),
    Failed {
        failure: ComputeSettlementFailure,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    },
}

#[derive(Debug)]
pub(super) enum ConcurrentIndependentError {
    ChangedCut(SettlementChangedCut),
    Fault(AuthorityFault),
}

impl CompiledSharedIndependent {
    pub(in crate::authority) fn is_compatible_with(&self, other: &Self) -> bool {
        self.support.is_compatible(other.support)
    }

    pub(in crate::authority) fn bind(
        self,
        authority: &TxPoolAuthority,
    ) -> Result<PreparedIndependentApply<'_>, ConcurrentIndependentError> {
        if authority.generation != self.generation || authority.chain_view != self.chain_view {
            return Err(ConcurrentIndependentError::ChangedCut(
                SettlementChangedCut::generation_or_chain(),
            ));
        }
        Ok(PreparedIndependentApply::Shared {
            authority,
            delta: self.delta,
            support: self.support,
            staged_effect: Some(self.staged_effect),
        })
    }

    pub(in crate::authority) fn commit_ready_job(
        self,
        authority: &TxPoolAuthority,
        reservation: ReadySlotReservation,
    ) -> ReadyJobCommitOutcome {
        let Self {
            generation,
            chain_view,
            delta,
            support,
            staged_effect,
        } = self;
        if authority.generation != generation || authority.chain_view != chain_view {
            drop(delta);
            drop(reservation);
            return match staged_effect.rollback_with_wake() {
                Ok(wake) => ReadyJobCommitOutcome::Stale(wake),
                Err(_) => ReadyJobCommitOutcome::Fault {
                    fault: AuthorityFault::EffectProjection,
                    effect_wake: None,
                },
            };
        }
        match apply_seal::commit_ready_job_rows(authority, delta, support, reservation) {
            Ok(committed) => {
                let effect_wake = staged_effect.activate_with_wake();
                ReadyJobCommitOutcome::Committed(committed.finish(authority, Some(effect_wake)))
            }
            Err(error) => {
                let effect_wake = staged_effect.rollback_with_wake().ok();
                match error {
                    ConcurrentIndependentError::ChangedCut(_) => effect_wake.map_or(
                        ReadyJobCommitOutcome::Fault {
                            fault: AuthorityFault::EffectProjection,
                            effect_wake: None,
                        },
                        ReadyJobCommitOutcome::Stale,
                    ),
                    ConcurrentIndependentError::Fault(fault) => {
                        ReadyJobCommitOutcome::Fault { fault, effect_wake }
                    }
                }
            }
        }
    }

    fn commit_ready_head(
        self,
        authority: &TxPoolAuthority,
        reservation: ReadyReservation,
    ) -> ReadyHeadCommitOutcome {
        let Self {
            generation,
            chain_view,
            delta,
            support,
            staged_effect,
        } = self;
        if authority.generation != generation || authority.chain_view != chain_view {
            drop(delta);
            drop(reservation);
            return match staged_effect.rollback_with_wake() {
                Ok(effect_wake) => ReadyHeadCommitOutcome::Stale {
                    effect_wake: Some(effect_wake),
                },
                Err(_) => ReadyHeadCommitOutcome::Fault {
                    fault: AuthorityFault::EffectProjection,
                    effect_wake: None,
                },
            };
        }
        match apply_seal::commit_reserved_ready_head_rows(authority, delta, support, reservation) {
            Ok(committed) => {
                let effect_wake = staged_effect.activate_with_wake();
                ReadyHeadCommitOutcome::Committed(committed.finish(authority, Some(effect_wake)))
            }
            Err(error) => {
                let effect_wake = staged_effect.rollback_with_wake().ok();
                match error {
                    ConcurrentIndependentError::ChangedCut(_) => effect_wake.map_or(
                        ReadyHeadCommitOutcome::Fault {
                            fault: AuthorityFault::EffectProjection,
                            effect_wake: None,
                        },
                        |effect_wake| ReadyHeadCommitOutcome::Stale {
                            effect_wake: Some(effect_wake),
                        },
                    ),
                    ConcurrentIndependentError::Fault(fault) => {
                        ReadyHeadCommitOutcome::Fault { fault, effect_wake }
                    }
                }
            }
        }
    }

    pub(super) fn commit_direct(self, authority: &TxPoolAuthority) -> SharedDirectCommitOutcome {
        let Self {
            generation,
            chain_view,
            delta,
            support,
            staged_effect,
        } = self;
        if authority.generation != generation || authority.chain_view != chain_view {
            drop(delta);
            return match staged_effect.rollback_with_wake() {
                Ok(wake) => SharedDirectCommitOutcome::Stale(wake),
                Err(_) => SharedDirectCommitOutcome::Fault {
                    fault: AuthorityFault::EffectProjection,
                    effect_wake: None,
                },
            };
        }
        match apply_seal::commit_unreserved_shared_rows(authority, delta, support) {
            Ok(committed) => {
                let effect_wake = staged_effect.activate_with_wake();
                SharedDirectCommitOutcome::Committed(committed.finish(authority, Some(effect_wake)))
            }
            Err(error) => {
                let effect_wake = staged_effect.rollback_with_wake().ok();
                match error {
                    ConcurrentIndependentError::ChangedCut(_) => effect_wake.map_or(
                        SharedDirectCommitOutcome::Fault {
                            fault: AuthorityFault::EffectProjection,
                            effect_wake: None,
                        },
                        SharedDirectCommitOutcome::Stale,
                    ),
                    ConcurrentIndependentError::Fault(fault) => {
                        SharedDirectCommitOutcome::Fault { fault, effect_wake }
                    }
                }
            }
        }
    }

    /// Cancel one compiled Ready job before owner mutation. The reservation
    /// and delta are released before the staged effect is terminalized, and
    /// the exact wake edge is returned to the runtime for publication. This
    /// is the sole transport-failure cleanup path; dropping a compiled job is
    /// not allowed to substitute for semantic terminalization.
    pub(in crate::authority) fn cancel_ready_job(
        self,
        reservation: ReadySlotReservation,
    ) -> Result<super::effect::EffectWakeTransition, AuthorityFault> {
        self.cancel_ready_job_inner(Some(reservation))
    }

    /// Cancel a compiled job before a scheduler reservation is split from the
    /// captured cohort. This is used only when a later member proves that the
    /// complete cohort cannot form one coherent compatible wave.
    pub(in crate::authority) fn cancel_unassigned_ready_job(
        self,
    ) -> Result<super::effect::EffectWakeTransition, AuthorityFault> {
        self.cancel_ready_job_inner(None)
    }

    fn cancel_ready_job_inner(
        self,
        reservation: Option<ReadySlotReservation>,
    ) -> Result<super::effect::EffectWakeTransition, AuthorityFault> {
        let Self {
            generation: _,
            chain_view: _,
            delta,
            support: _,
            staged_effect,
        } = self;
        drop(delta);
        drop(reservation);
        staged_effect
            .rollback_with_wake()
            .map_err(|_| AuthorityFault::EffectProjection)
    }
}

impl CompiledSharedReadyReresolution {
    fn commit(
        self,
        authority: &TxPoolAuthority,
        reservation: ReadyReservation,
    ) -> ReadyHeadCommitOutcome {
        let Self {
            generation,
            chain_view,
            delta,
            support,
        } = self;
        if authority.generation != generation || authority.chain_view != chain_view {
            drop(delta);
            drop(reservation);
            return ReadyHeadCommitOutcome::Stale { effect_wake: None };
        }
        match apply_seal::commit_reserved_ready_head_rows(authority, delta, support, reservation) {
            Ok(committed) => ReadyHeadCommitOutcome::Committed(committed.finish(authority, None)),
            Err(ConcurrentIndependentError::ChangedCut(_)) => {
                ReadyHeadCommitOutcome::Stale { effect_wake: None }
            }
            Err(ConcurrentIndependentError::Fault(fault)) => ReadyHeadCommitOutcome::Fault {
                fault,
                effect_wake: None,
            },
        }
    }
}

impl PreparedSharedReadyHeadDisposition<'_> {
    pub(in crate::authority) fn commit(
        self,
        reservation: ReadyReservation,
    ) -> ReadyHeadCommitOutcome {
        match self {
            Self::Effectful {
                authority,
                compiled,
            } => compiled.commit_ready_head(authority, reservation),
            Self::Reresolve {
                authority,
                compiled,
            } => compiled.commit(authority, reservation),
            Self::PeerRevocation(core) => {
                drop(reservation);
                match apply_seal::commit_shared_peer_revocation_core(core) {
                    Ok(committed) => ReadyHeadCommitOutcome::Committed(committed),
                    Err(failure) => {
                        let (error, effect_wake) = failure.into_parts();
                        match error {
                            ingress::ConcurrentRetainedIngressError::Stale => {
                                ReadyHeadCommitOutcome::Stale { effect_wake }
                            }
                            ingress::ConcurrentRetainedIngressError::Backpressure(pressure) => {
                                ReadyHeadCommitOutcome::Backpressure {
                                    pressure,
                                    effect_wake,
                                }
                            }
                            ingress::ConcurrentRetainedIngressError::Fault(fault) => {
                                ReadyHeadCommitOutcome::Fault { fault, effect_wake }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl PreparedSharedComputeSettlement<'_> {
    pub(in crate::authority) fn apply(self) -> SharedComputeSettlementOutcome {
        let Self {
            authority,
            delta,
            support,
            staged_effect,
            recovery,
        } = self;
        #[cfg(test)]
        authority.entries.enter_compute_settlement_commit_probe();
        match apply_seal::commit_unreserved_shared_rows(authority, delta, support) {
            Ok(committed) => {
                let effect_wake =
                    staged_effect.map(super::effect::StagedEffect::activate_with_wake);
                SharedComputeSettlementOutcome::Committed(committed.finish(authority, effect_wake))
            }
            Err(error) => {
                let effect_wake = match staged_effect {
                    Some(staged) => match staged.rollback_with_wake() {
                        Ok(wake) => Some(wake),
                        Err(_) => {
                            return SharedComputeSettlementOutcome::Failed {
                                failure: authority.compute_settlement_failure(
                                    PlanError::Fault(AuthorityFault::EffectProjection),
                                    recovery,
                                ),
                                effect_wake: None,
                            };
                        }
                    },
                    None => None,
                };
                let failure = match error {
                    ConcurrentIndependentError::ChangedCut(changed_cut) => {
                        authority.compute_settlement_changed_cut_failure(changed_cut, recovery)
                    }
                    ConcurrentIndependentError::Fault(fault) => {
                        authority.compute_settlement_failure(PlanError::Fault(fault), recovery)
                    }
                };
                SharedComputeSettlementOutcome::Failed {
                    failure,
                    effect_wake,
                }
            }
        }
    }
}

impl PreparedSharedComputePeerRevocation<'_> {
    pub(in crate::authority) fn apply(self) -> SharedComputeSettlementOutcome {
        self.core.apply_compute(self.recovery)
    }
}

impl ReadyCommittedRows {
    pub(in crate::authority) fn finish(
        self,
        authority: &TxPoolAuthority,
        effect_wake: Option<super::effect::EffectWakeTransition>,
    ) -> CommittedSharedApply {
        let post_commit_fault = match self.resource_health {
            ResourceCommitHealth::Healthy => None,
            ResourceCommitHealth::Faulted => Some(AuthorityFault::ResourceProjection),
        };
        let committed = if self.scheduler_unchanged {
            finish_apply_without_scheduler_change(
                authority,
                self.before,
                self.compute_slot_released,
                false,
                self.retirement,
            )
        } else {
            finish_apply(
                authority,
                self.before,
                self.compute_slot_released,
                false,
                self.retirement,
            )
        };
        let committed = match effect_wake {
            Some(wake) => committed.with_effect_wake(wake),
            None => committed,
        };
        CommittedSharedApply {
            committed,
            post_commit_fault,
        }
    }
}

impl CommittedSharedApply {
    fn clean(committed: CommittedDelta) -> Self {
        Self::from_resource_health(committed, ResourceCommitHealth::Healthy)
    }

    fn from_resource_health(
        committed: CommittedDelta,
        resource_health: ResourceCommitHealth,
    ) -> Self {
        let post_commit_fault = match resource_health {
            ResourceCommitHealth::Healthy => None,
            ResourceCommitHealth::Faulted => Some(AuthorityFault::ResourceProjection),
        };
        Self {
            committed,
            post_commit_fault,
        }
    }

    pub(in crate::authority) fn into_parts(self) -> (CommittedDelta, Option<AuthorityFault>) {
        (self.committed, self.post_commit_fault)
    }
}

#[cfg(test)]
#[must_use = "candidate disposition must be applied exactly once"]
pub(super) enum CandidateDispositionPlan<'authority> {
    Accepted(PreparedApply<'authority>),
    Rejected(PreparedCandidateRejection<'authority>),
}

/// Closed final-validation disposition. A caller cannot turn a lock-external
/// validation failure into an ad-hoc retry or forget the matching committed
/// rejection effect.
#[cfg(test)]
#[must_use = "final admission disposition must be applied exactly once"]
pub(super) enum FinalAdmissionDispositionPlan<'authority> {
    Candidate(CandidateDispositionPlan<'authority>),
    ValidationRejected(PreparedValidationRejection<'authority>),
    Reresolve(PreparedApply<'authority>),
}

/// Feature-internal synthetic admission has only two legal committed
/// outcomes. A duplicate is a true no-op; insertion still owns the ordinary
/// atomic membership Apply.
#[cfg(any(test, feature = "internal"))]
#[must_use = "internal plug disposition must be applied or returned as a no-op"]
pub(super) enum InternalPlugDisposition<'authority> {
    Insert(PreparedApply<'authority>),
    Duplicate,
}

#[cfg(any(test, feature = "internal"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InternalPlugPlanError {
    WouldDisplace,
    Plan(PlanError),
}

#[cfg(any(test, feature = "internal"))]
impl From<PlanError> for InternalPlugPlanError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

/// Pure final-membership policy result for TestAccept. This type cannot be
/// applied and carries no projection/effect delta; Local compiles the same
/// evaluator result through the canonical shared Direct compiler instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DirectAdmissionEvaluation {
    Accepted(EntryCompleted),
    Duplicate(RawTxHash),
    Rejected(MembershipReject),
}

/// A final candidate rejection whose public outcome is already part of the
/// same prepared authority Apply that removes the owner and releases charge.
/// The inner transition is private, so a caller cannot apply terminalization
/// while forgetting the matching journal publication.
#[cfg(test)]
#[must_use = "candidate rejection must be applied exactly once"]
pub(super) struct PreparedCandidateRejection<'authority> {
    reason: MembershipReject,
    plan: PreparedApply<'authority>,
}

#[cfg(test)]
#[must_use = "validation rejection must be applied exactly once"]
pub(super) struct PreparedValidationRejection<'authority> {
    reason: CommittedPublicReject,
    plan: PreparedApply<'authority>,
}

#[cfg(test)]
impl PreparedValidationRejection<'_> {
    pub(super) fn apply(self) -> (CommittedPublicReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
}

#[cfg(test)]
impl PreparedCandidateRejection<'_> {
    pub(super) fn apply(self) -> (MembershipReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
}

impl PreparedApply<'_> {
    fn stage(
        authority: &mut TxPoolAuthority,
        mut delta: DependencyAuthorityDelta,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let dependency = delta
            .take_dependency()
            .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let staged =
            StagedDependencyBatch::stage_primary_replacements(&authority.dependencies, dependency)?;
        Ok(PreparedApply {
            authority,
            kind: PreparedApplyKind::Dependency(Box::new(PreparedDependencyApply {
                delta,
                staged,
            })),
        })
    }

    fn plain(authority: &mut TxPoolAuthority, delta: PlainAuthorityDelta) -> PreparedApply<'_> {
        PreparedApply {
            authority,
            kind: PreparedApplyKind::Plain(delta),
        }
    }

    pub(super) fn apply(self) -> CommittedDelta {
        apply_seal::commit(self)
    }

    fn apply_with(self, token: &ApplyToken) -> CommittedDelta {
        let Self { authority, kind } = self;
        let compute_slot_released = match &kind {
            PreparedApplyKind::Dependency(prepared) => {
                prepared.delta.releases_preaccepted_active_work()
            }
            PreparedApplyKind::Plain(delta) => delta.releases_preaccepted_active_work(),
        };
        let before = authority.wake_projection();
        let retirement = match kind {
            PreparedApplyKind::Dependency(prepared) => {
                let PreparedDependencyApply { delta, staged } = *prepared;
                match delta {
                    DependencyAuthorityDelta::Entry(delta) => {
                        Self::apply_entry(&mut *authority, token, delta, staged)
                    }
                    #[cfg(any(test, feature = "internal"))]
                    DependencyAuthorityDelta::Membership(delta) => {
                        Self::apply_membership(&mut *authority, token, delta, staged)
                    }
                    DependencyAuthorityDelta::ClearPipeline(delta) => {
                        Self::apply_clear_pipeline(&mut *authority, token, delta, staged)
                    }
                    #[cfg(test)]
                    DependencyAuthorityDelta::Admin(delta) => {
                        Self::apply_admin(&mut *authority, token, delta, staged)
                    }
                    DependencyAuthorityDelta::Chain(delta) => {
                        Self::apply_chain(&mut *authority, token, delta, staged)
                    }
                }
            }
            PreparedApplyKind::Plain(PlainAuthorityDelta::Effect(delta)) => {
                Self::apply_effect(&mut *authority, token, delta)
            }
            PreparedApplyKind::Plain(PlainAuthorityDelta::ClearPool(delta)) => {
                Self::apply_clear_pool(&mut *authority, token, *delta)
            }
        };
        finish_apply(authority, before, compute_slot_released, true, retirement)
    }

    fn apply_entry(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: EntryDelta,
        dependency: StagedDependencyBatch,
    ) -> ApplyRetirement {
        let mut retired = delta.retired;
        let proposed_counts = super::shard::ShardProposedCountPlan::default();
        let support = authority.entries.owner_resource_write_support(
            std::iter::once(&delta.key),
            &proposed_counts,
            delta.resource.shard_plan(),
        );
        let update = OwnerResourceUpdate::new(delta.key, delta.after);
        let DerivedOwnerDelta {
            indexes,
            mut sources,
            template_sources,
        } = delta.owners;
        let source_changes = sources.take_template_selection();
        debug_assert_eq!(template_sources.counts().changed(), source_changes);
        authority.commit_owner_resources(
            token,
            PreparedOwnerResourceDelta::single(update, delta.resource, support)
                .with_owner_source_advance(template_sources.into_exclusive_advance()),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.indexes.apply(indexes);
        authority.source_versions.apply(sources);
        authority.scheduler.lock().apply(delta.scheduler);
        let dependency = dependency.publish_exclusive();
        let retired_effect = authority.effects.lock().apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: source_changes.0 || source_changes.1,
        }
    }

    #[cfg(any(test, feature = "internal"))]
    fn apply_membership(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        mut delta: MembershipDelta,
        dependency: StagedDependencyBatch,
    ) -> ApplyRetirement {
        let mut retired = delta.retired;
        let proposed_counts = delta.projection.take_proposed_counts();
        let support = authority.entries.owner_resource_write_support(
            delta
                .removals
                .iter()
                .map(|removal| &removal.hash)
                .chain(std::iter::once(&delta.changed_key)),
            &proposed_counts,
            delta.resource.shard_plan(),
        );
        let removal_updates = delta
            .removals
            .iter_mut()
            .map(|removal| OwnerResourceUpdate::new(removal.hash.clone(), removal.take_after()));
        let changed = std::iter::once(OwnerResourceUpdate::new(
            delta.changed_key,
            Some(delta.changed_after),
        ));
        let DerivedOwnerDelta {
            indexes,
            mut sources,
            template_sources,
        } = delta.owners;
        let source_changes = sources.take_template_selection();
        debug_assert_eq!(template_sources.counts().changed(), source_changes);
        authority.commit_owner_resources_indexes_membership(
            token,
            PreparedOwnerResourceDelta::batch(
                removal_updates.chain(changed),
                delta.resource,
                proposed_counts,
                support,
            ),
            indexes,
            delta.projection,
            template_sources.into_exclusive_advance(),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.source_versions.apply(sources);
        authority.scheduler.lock().apply(delta.scheduler);
        let dependency = dependency.publish_exclusive();
        let retired_effect = authority.effects.lock().apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: delta.async_process_start.map_or(
                AsyncProcessObservations::None,
                AsyncProcessObservations::One,
            ),
            removals: delta.removals,
            retired,
            retired_effect,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: source_changes.0 || source_changes.1,
        }
    }
}

impl PreparedIndependentApply<'_> {
    pub(in crate::authority) fn apply(
        self,
    ) -> Result<CommittedSharedApply, ConcurrentIndependentError> {
        apply_seal::commit_independent(self)
    }

    pub(in crate::authority) fn apply_reserved(
        self,
        reservation: ReadyReservation,
    ) -> Result<CommittedSharedApply, ConcurrentIndependentError> {
        apply_seal::commit_reserved_independent(self, reservation)
    }

    fn apply_with(
        self,
        token: &ApplyToken,
    ) -> Result<CommittedSharedApply, ConcurrentIndependentError> {
        match self {
            Self::Shared {
                authority,
                delta,
                support,
                staged_effect,
            } => Self::apply_shared(authority, token, delta, support, staged_effect, None),
            #[cfg(test)]
            Self::Exclusive { authority, delta } => {
                Self::apply_exclusive(authority, token, delta).map(CommittedSharedApply::clean)
            }
        }
    }

    fn apply_shared(
        authority: &TxPoolAuthority,
        token: &ApplyToken,
        delta: IndependentDelta,
        support: ShardApplySupport,
        staged_effect: Option<super::effect::StagedEffect>,
        reservation: Option<ReadyApplyReservation>,
    ) -> Result<CommittedSharedApply, ConcurrentIndependentError> {
        let committed = Self::apply_shared_rows(authority, token, delta, support, reservation)?;
        let effect_wake = staged_effect.map(super::effect::StagedEffect::activate_with_wake);
        let committed = committed.finish(authority, effect_wake);
        Ok(committed)
    }

    fn apply_shared_rows(
        authority: &TxPoolAuthority,
        token: &ApplyToken,
        mut delta: IndependentDelta,
        support: ShardApplySupport,
        reservation: Option<ReadyApplyReservation>,
    ) -> Result<ReadyCommittedRows, ConcurrentIndependentError> {
        let ready_phase_only = delta.is_ready_phase_only();
        let compute_slot_released = delta
            .resource
            .as_ref()
            .is_some_and(ResourceBatchPlan::releases_preaccepted_active_work);
        let scheduler_unchanged = reservation.is_none() && delta.scheduler.is_empty();
        let before = match reservation.as_ref() {
            None if scheduler_unchanged => authority.wake_projection_without_scheduler(),
            Some(reservation) => reservation
                .scheduler_wake_before()
                .map_err(|_| {
                    ConcurrentIndependentError::Fault(AuthorityFault::SchedulerProjection)
                })?
                .map_or_else(
                    || authority.wake_projection(),
                    |scheduler| authority.wake_projection_with_scheduler(scheduler),
                ),
            None => authority.wake_projection(),
        };
        let proposed_counts = delta.projection.take_proposed_counts();
        let mut retired = delta.retired;
        let DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        } = delta.owners;
        let template_source_changed = template_sources.counts().changed();
        let (dependency, resource_health) = authority.commit_shared_independent_rows(
            token,
            delta.owner_cuts,
            delta.resource,
            proposed_counts,
            support,
            indexes,
            delta.projection,
            delta.dependency,
            ready_phase_only,
            sources,
            template_sources.counts(),
            delta.scheduler,
            reservation,
            &mut retired,
        )?;
        let _reserved_clock_high_water = delta.clocks;
        let retirement = ApplyRetirement {
            async_process_observations: if delta.async_process_starts.is_empty() {
                AsyncProcessObservations::None
            } else {
                AsyncProcessObservations::Batch(delta.async_process_starts)
            },
            removals: delta.removals,
            retired,
            retired_effect: None,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: template_source_changed.0 || template_source_changed.1,
        };
        Ok(ReadyCommittedRows {
            before,
            compute_slot_released,
            resource_health,
            retirement,
            scheduler_unchanged,
        })
    }

    #[cfg(test)]
    fn apply_exclusive(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        mut delta: IndependentDelta,
    ) -> Result<CommittedDelta, ConcurrentIndependentError> {
        let resource = delta
            .resource
            .take()
            .ok_or(ConcurrentIndependentError::Fault(
                AuthorityFault::ResourceProjection,
            ))?;
        let compute_slot_released = resource.releases_preaccepted_active_work();
        let before = authority.wake_projection();
        let dependency = StagedDependencyBatch::stage_primary_replacements(
            &authority.dependencies,
            std::mem::take(&mut delta.dependency),
        )
        .map_err(|error| match error {
            DependencyStageError::Stale => {
                ConcurrentIndependentError::ChangedCut(SettlementChangedCut::owner_or_projection())
            }
            DependencyStageError::Capacity => ConcurrentIndependentError::ChangedCut(
                SettlementChangedCut::dependency_stage_capacity(),
            ),
            DependencyStageError::Projection | DependencyStageError::Allocation => {
                ConcurrentIndependentError::Fault(AuthorityFault::DependencyProjection)
            }
        })?;
        let proposed_counts = delta.projection.take_proposed_counts();
        let support = authority.entries.owner_resource_write_support(
            delta.owner_cuts.iter().filter_map(|owner| {
                matches!(owner.action, IndependentOwnerAction::Replace(_)).then_some(&owner.key)
            }),
            &proposed_counts,
            resource.shard_plan(),
        );
        let updates = delta.owner_cuts.into_iter().filter_map(|owner| {
            let IndependentOwnerAction::Replace(after) = owner.action else {
                return None;
            };
            Some(OwnerResourceUpdate::new(owner.key, after))
        });
        let mut retired = delta.retired;
        let DerivedOwnerDelta {
            indexes,
            mut sources,
            template_sources,
        } = delta.owners;
        let source_changes = sources.take_template_selection();
        debug_assert_eq!(template_sources.counts().changed(), source_changes);
        authority.commit_owner_resources_indexes_membership(
            token,
            PreparedOwnerResourceDelta::batch(updates, resource, proposed_counts, support),
            indexes,
            delta.projection,
            template_sources.into_exclusive_advance(),
            &mut retired,
        );
        let state = authority.write(token);
        state.source_versions.apply(sources);
        state.scheduler.lock().apply_batch(delta.scheduler);
        let dependency = dependency.publish_exclusive();
        let retired_effect = state.effects.lock().apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        let retirement = ApplyRetirement {
            async_process_observations: if delta.async_process_starts.is_empty() {
                AsyncProcessObservations::None
            } else {
                AsyncProcessObservations::Batch(delta.async_process_starts)
            },
            removals: delta.removals,
            retired,
            retired_effect,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: source_changes.0 || source_changes.1,
        };
        Ok(finish_apply(
            authority,
            before,
            compute_slot_released,
            true,
            retirement,
        ))
    }
}

impl PreparedApply<'_> {
    fn apply_effect(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: EffectOnlyDelta,
    ) -> ApplyRetirement {
        let authority = authority.write(token);
        let retired_effect = authority.effects.lock().apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect,
            retired_generation: None,
            dependency: None,
            template_source_changed: false,
        }
    }

    fn apply_clear_pool(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: ClearPoolDelta,
    ) -> ApplyRetirement {
        let FreshGeneration {
            entries,
            resources,
            scheduler,
            dependencies,
            dependency_publication,
        } = delta.fresh;
        let (previous_entries, previous_resources) =
            authority.replace_owner_generation_resources(token, entries, resources);
        let dependencies = dependencies.rebind_entries(&authority.entries);
        let authority = authority.write(token);
        let retired_generation = RetiredGeneration {
            entries: previous_entries,
            _resources: previous_resources,
            _scheduler: std::mem::replace(&mut authority.scheduler, scheduler),
            _dependencies: std::mem::replace(&mut authority.dependencies, dependencies),
        };
        authority.generation = delta.generation;
        authority.chain_view = delta.chain_view;
        authority.source_versions.apply(delta.sources);
        let retired_effect = authority.effects.lock().apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect,
            retired_generation: Some(retired_generation),
            dependency: Some(dependency_publication.into_finalization()),
            template_source_changed: true,
        }
    }

    fn apply_clear_pipeline(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: ClearPipelineDelta,
        dependency: StagedDependencyBatch,
    ) -> ApplyRetirement {
        let template_source_changed = delta.removal.owners.template_sources.counts().changed();
        let OwnerRemovalCommit {
            retired,
            dependency,
        } = Self::apply_owner_removal(authority, token, delta.removal, dependency);
        let authority = authority.write(token);
        authority.generation = delta.generation;
        let retired_effect = authority.effects.lock().apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: template_source_changed.0 || template_source_changed.1,
        }
    }

    #[cfg(test)]
    fn apply_admin(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: AdminDelta,
        dependency: StagedDependencyBatch,
    ) -> ApplyRetirement {
        let template_source_changed = delta.removal.owners.template_sources.counts().changed();
        let OwnerRemovalCommit {
            retired,
            dependency,
        } = Self::apply_owner_removal(authority, token, delta.removal, dependency);
        let authority = authority.write(token);
        let retired_effect = authority.effects.lock().apply(delta.effect);
        authority.entries.apply_exclusive_peer_fence(delta.marker);
        authority.peer_bans.apply(delta.marker);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: template_source_changed.0 || template_source_changed.1,
        }
    }

    fn apply_owner_removal(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        removal: OwnerRemovalBatch,
        dependency: StagedDependencyBatch,
    ) -> OwnerRemovalCommit {
        let OwnerRemovalBatch {
            hashes,
            expected_versions: _,
            owners,
            resources,
            mut membership,
            scheduler,
            dependency: _,
            mut retired,
        } = removal;
        let proposed_counts = membership.take_proposed_counts();
        let support = authority.entries.owner_resource_write_support(
            hashes.iter(),
            &proposed_counts,
            resources.shard_plan(),
        );
        let updates = hashes
            .into_iter()
            .map(|hash| OwnerResourceUpdate::new(hash, None));
        let DerivedOwnerDelta {
            indexes,
            mut sources,
            template_sources,
        } = owners;
        let source_changes = sources.take_template_selection();
        debug_assert_eq!(template_sources.counts().changed(), source_changes);
        authority.commit_owner_resources(
            token,
            PreparedOwnerResourceDelta::batch(
                updates,
                resources.into_exclusive_plan(),
                proposed_counts,
                support,
            )
            .with_owner_source_advance(template_sources.into_exclusive_advance()),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.indexes.apply(indexes);
        authority.source_versions.apply(sources);
        authority.membership.apply(membership);
        authority.scheduler.lock().apply_batch(scheduler);
        let dependency = dependency.publish_exclusive();
        OwnerRemovalCommit {
            retired,
            dependency,
        }
    }

    fn apply_chain(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        mut delta: ChainDelta,
        dependency: StagedDependencyBatch,
    ) -> ApplyRetirement {
        let mut retired = delta.retired;
        let proposed_counts = delta.membership.take_proposed_counts();
        let support = authority.entries.owner_resource_write_support(
            delta.updates.iter().map(|update| &update.key),
            &proposed_counts,
            delta.resources.shard_plan(),
        );
        let updates = delta
            .updates
            .into_iter()
            .map(|update| OwnerResourceUpdate::new(update.key, update.after));
        let DerivedOwnerDelta {
            indexes,
            mut sources,
            template_sources,
        } = delta.owners;
        let source_changes = sources.take_template_selection();
        debug_assert_eq!(template_sources.counts().changed(), source_changes);
        authority.commit_owner_resources(
            token,
            PreparedOwnerResourceDelta::batch(updates, delta.resources, proposed_counts, support)
                .with_owner_source_advance(template_sources.into_exclusive_advance()),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.indexes.apply(indexes);
        authority.source_versions.apply(sources);
        authority.membership.apply(delta.membership);
        authority.scheduler.lock().apply_batch(delta.scheduler);
        let dependency = dependency.publish_exclusive();
        let retired_effect = authority.effects.lock().apply(delta.effect);
        authority.chain_view = delta.view.clone();
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: true,
        }
    }
}

#[cfg(test)]
impl PreparedApply<'_> {
    pub(in crate::authority) fn entry_shard_support(
        &self,
    ) -> Option<(
        super::shard_support::AuthorityShardSupport,
        super::shard_support::ExclusiveSupport,
    )> {
        let PreparedApplyKind::Dependency(prepared) = &self.kind else {
            return None;
        };
        let DependencyAuthorityDelta::Entry(delta) = &prepared.delta else {
            return None;
        };
        Some(delta.shard_support())
    }
}

fn next_generation(generation: PoolGeneration) -> Result<PoolGeneration, PlanError> {
    generation
        .0
        .checked_add(1)
        .map(PoolGeneration)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_chain_revision(revision: ChainRevision) -> Result<ChainRevision, PlanError> {
    revision
        .0
        .checked_add(1)
        .map(ChainRevision)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct ClockReservationError;

impl From<ClockReservationError> for PlanError {
    fn from(_: ClockReservationError) -> Self {
        Self::Fault(AuthorityFault::CounterExhausted)
    }
}

/// A Plan capability for globally unique prospective owner identities.
///
/// Resource and policy checks which do not need an identity run first. Once
/// issued, an identity is never reused even if the prepared Plan is dropped;
/// only the parent receipt is discardable. This phase deliberately cannot
/// expose an Apply sequence and must be consumed by [`Self::commit`] before a
/// nonempty transition can be built.
pub(in crate::authority) struct ClockPlanReservation {
    bank: Arc<AuthorityClockBank>,
    clocks: AuthorityClocks,
}

/// One prospective owner-identity branch borrowed from a live clock Plan.
///
/// The bank advances when an identity is issued. [`Self::adopt`] records that
/// high-water mark in the parent receipt; dropping the branch leaves a
/// deliberate non-reusable gap. The exclusive borrow prevents stale or
/// cross-parent adoption.
pub(in crate::authority) struct OwnerClockBranch<'parent> {
    parent: &'parent mut ClockPlanReservation,
    next_version: EntryVersion,
    next_arrival: Arrival,
}

impl OwnerClockBranch<'_> {
    pub(in crate::authority) fn replacement(
        mut self,
    ) -> Result<(EntryVersion, Self), ClockReservationError> {
        let (version, clocks) = self
            .parent
            .bank
            .reserve_replacement()
            .map_err(|_| ClockReservationError)?;
        self.next_version = clocks.next_version;
        self.next_arrival = self.next_arrival.max(clocks.next_arrival);
        Ok((version, self))
    }

    pub(in crate::authority) fn insertion(
        mut self,
    ) -> Result<(EntryVersion, Arrival, Self), ClockReservationError> {
        let (version, arrival, clocks) = self
            .parent
            .bank
            .reserve_insertion()
            .map_err(|_| ClockReservationError)?;
        self.next_version = clocks.next_version;
        self.next_arrival = clocks.next_arrival;
        Ok((version, arrival, self))
    }

    pub(in crate::authority) fn replacements(
        mut self,
        members: NonZeroUsize,
    ) -> Result<(impl Iterator<Item = EntryVersion> + use<>, Self), ClockReservationError> {
        let (versions, clocks) = self
            .parent
            .bank
            .reserve_replacements(members)
            .map_err(|_| ClockReservationError)?;
        self.next_version = clocks.next_version;
        self.next_arrival = self.next_arrival.max(clocks.next_arrival);
        Ok((versions.map(EntryVersion), self))
    }

    pub(in crate::authority) fn adopt(self) {
        self.parent.clocks.next_version = self.parent.clocks.next_version.max(self.next_version);
        self.parent.clocks.next_arrival = self.parent.clocks.next_arrival.max(self.next_arrival);
    }
}

impl ClockPlanReservation {
    pub(in crate::authority) fn begin(bank: Arc<AuthorityClockBank>) -> Self {
        let clocks = bank.snapshot();
        Self { bank, clocks }
    }

    pub(in crate::authority) fn commit(
        mut self,
    ) -> Result<ApplyClockReservation, ClockReservationError> {
        let (sequence, clocks) = self
            .bank
            .reserve_sequence()
            .map_err(|_| ClockReservationError)?;
        self.clocks.next_sequence = self.clocks.next_sequence.max(clocks.next_sequence);
        Ok(ApplyClockReservation {
            sequence,
            plan: self,
        })
    }

    pub(in crate::authority) fn replacement(
        mut self,
    ) -> Result<(EntryVersion, Self), ClockReservationError> {
        let (version, branch) = self.owner_branch().replacement()?;
        branch.adopt();
        Ok((version, self))
    }

    pub(in crate::authority) fn insertion(
        mut self,
    ) -> Result<(EntryVersion, Arrival, Self), ClockReservationError> {
        let (version, arrival, branch) = self.owner_branch().insertion()?;
        branch.adopt();
        Ok((version, arrival, self))
    }

    pub(in crate::authority) fn replacements(
        mut self,
        members: NonZeroUsize,
    ) -> Result<(impl Iterator<Item = EntryVersion> + use<>, Self), ClockReservationError> {
        let (versions, branch) = self.owner_branch().replacements(members)?;
        branch.adopt();
        Ok((versions, self))
    }

    pub(in crate::authority) fn owner_branch(&mut self) -> OwnerClockBranch<'_> {
        OwnerClockBranch {
            next_version: self.clocks.next_version,
            next_arrival: self.clocks.next_arrival,
            parent: self,
        }
    }

    pub(in crate::authority) fn adopt_owner_progress(
        mut self,
        owner_clocks: AuthorityClocks,
    ) -> Result<Self, ClockReservationError> {
        let version_advance = owner_clocks
            .next_version
            .0
            .checked_sub(self.clocks.next_version.0)
            .ok_or(ClockReservationError)?;
        let arrival_advance = owner_clocks
            .next_arrival
            .0
            .checked_sub(self.clocks.next_arrival.0)
            .ok_or(ClockReservationError)?;
        if arrival_advance > version_advance {
            return Err(ClockReservationError);
        }
        let clocks = self.bank.adopt_owner_progress(owner_clocks);
        self.clocks.next_version = self.clocks.next_version.max(clocks.next_version);
        self.clocks.next_arrival = self.clocks.next_arrival.max(clocks.next_arrival);
        Ok(self)
    }
}

/// The sole sealed clock capability for one nonempty authority Apply.
///
/// Construction reserves exactly one Apply sequence. Owner identities may be
/// reserved before or after that seal through the same linear Plan protocol;
/// callers never construct a partial `AuthorityClocks` value.
pub(in crate::authority) struct ApplyClockReservation {
    sequence: ApplySequence,
    plan: ClockPlanReservation,
}

impl ApplyClockReservation {
    pub(in crate::authority) fn begin(
        bank: Arc<AuthorityClockBank>,
    ) -> Result<Self, ClockReservationError> {
        ClockPlanReservation::begin(bank).commit()
    }

    pub(in crate::authority) fn commit_owner_batch(
        bank: Arc<AuthorityClockBank>,
        expected: AuthorityClocks,
        owners: NonZeroUsize,
        insertions: usize,
    ) -> Result<Self, ApplyOwnerBatchReservationError> {
        let (sequence, clocks) = bank.reserve_apply_owner_batch(expected, owners, insertions)?;
        Ok(Self {
            sequence,
            plan: ClockPlanReservation { bank, clocks },
        })
    }

    pub(in crate::authority) const fn sequence(&self) -> ApplySequence {
        self.sequence
    }

    pub(in crate::authority) fn replacement(
        self,
    ) -> Result<(EntryVersion, Self), ClockReservationError> {
        let (version, plan) = self.plan.replacement()?;
        Ok((
            version,
            Self {
                sequence: self.sequence,
                plan,
            },
        ))
    }

    pub(in crate::authority) fn insertion(
        self,
    ) -> Result<(EntryVersion, Arrival, Self), ClockReservationError> {
        let (version, arrival, plan) = self.plan.insertion()?;
        Ok((
            version,
            arrival,
            Self {
                sequence: self.sequence,
                plan,
            },
        ))
    }

    #[cfg(test)]
    pub(in crate::authority) fn replacements(
        self,
        members: NonZeroUsize,
    ) -> Result<(impl Iterator<Item = EntryVersion> + use<>, Self), ClockReservationError> {
        let (versions, plan) = self.plan.replacements(members)?;
        Ok((
            versions,
            Self {
                sequence: self.sequence,
                plan,
            },
        ))
    }

    pub(in crate::authority) fn owner_branch(&mut self) -> OwnerClockBranch<'_> {
        self.plan.owner_branch()
    }

    pub(in crate::authority) fn adopt_owner_progress(
        self,
        owner_clocks: AuthorityClocks,
    ) -> Result<Self, ClockReservationError> {
        Ok(Self {
            sequence: self.sequence,
            plan: self.plan.adopt_owner_progress(owner_clocks)?,
        })
    }

    pub(in crate::authority) fn finish(self) -> AuthorityClocks {
        self.plan.clocks
    }
}

fn retired_buffer(capacity: usize) -> Result<RetiredOwners, PlanError> {
    RetiredOwners::try_with_capacity(capacity)
}

impl TxPoolAuthority {
    fn missing_resolution_disposition(
        &self,
        source: PreAcceptedSource,
        missing: &super::state::MissingDependencies,
    ) -> MissingResolutionDisposition {
        match source {
            PreAcceptedSource::Remote(_) => MissingResolutionDisposition::Wait,
            PreAcceptedSource::Proposal { .. } | PreAcceptedSource::Recovery(_) => {
                for key in missing.keys() {
                    let rejection = match key {
                        DependencyKey::Cell(out_point)
                            if !matches!(
                                self.entries.get(&RawTxHash(out_point.tx_hash())).as_deref(),
                                Some(OwnedTx::PreAccepted(_))
                            ) =>
                        {
                            Some(Reject::Resolve(OutPointError::Unknown(out_point.clone())))
                        }
                        DependencyKey::Header(hash) => {
                            Some(Reject::Resolve(OutPointError::InvalidHeader(hash.clone())))
                        }
                        DependencyKey::Cell(_) => None,
                    };
                    if let Some(rejection) = rejection {
                        return MissingResolutionDisposition::Reject(
                            SettlementRejection::ChainBound(CommittedPublicReject::new(rejection)),
                        );
                    }
                }
                MissingResolutionDisposition::Wait
            }
        }
    }

    fn validate_acceptance_evidence(
        &self,
        preaccepted: &PreAcceptedEntry,
        receipt: &FinalAdmissionReceipt,
    ) -> Result<(), PlanError> {
        if receipt.view() != &self.chain_view {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }
        let proof = receipt.proof();
        if proof.payload().identity() != &preaccepted.record.identity {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let dependencies = proof.payload().dependencies();
        if !self
            .dependencies
            .proof_is_current(dependencies, proof.dependency_cut())
        {
            return Err(PlanError::Stale(StalePlan::Dependency));
        }
        Ok(())
    }

    pub(super) fn final_admission_work(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
    ) -> Result<FinalAdmissionWork, PlanError> {
        let existing = self
            .entries
            .get(key)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &*existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Ready(verified) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        Ok(FinalAdmissionWork::new(
            key.clone(),
            expected,
            self.chain_view.clone(),
            verified.clone(),
        ))
    }

    pub(super) fn final_admission_preparation(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
    ) -> Result<FinalAdmissionPreparation, PlanError> {
        let existing = self
            .entries
            .get(key)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &*existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Ready(verified) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        Ok(FinalAdmissionPreparation::new(
            key.clone(),
            expected,
            self.chain_view.clone(),
            Arc::clone(verified.payload_arc()),
        ))
    }

    fn plan_membership_dependency_delta(
        &self,
        existing: Option<&OwnedTx>,
        after: &OwnedTx,
        removals: &[MembershipRemoval],
        sequence: ApplySequence,
    ) -> Result<DependencyBatchDelta, PlanError> {
        let owners = self.entries.read_all();
        let mut changes = Vec::new();
        changes
            .try_reserve(
                removals
                    .len()
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
            )
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        changes.push((existing, Some(after)));
        let mut removed_entries = Vec::new();
        removed_entries
            .try_reserve(removals.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for removal in removals {
            let removed = owners
                .get(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            changes.push((Some(removed), removal.after()));
            removed_entries.push(removed);
        }
        let lost = self.collect_dependency_loss_keys(removed_entries)?.keys;
        let mut available = match (existing, after) {
            (
                None | Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)),
                OwnedTx::Accepted(_),
            ) => {
                self.collect_dependency_loss_keys(std::iter::once(after))?
                    .keys
            }
            (None, OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_))
            | (
                Some(OwnedTx::PreAccepted(_)),
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_),
            )
            | (
                Some(OwnedTx::Accepted(_)),
                OwnedTx::PreAccepted(_) | OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_),
            )
            | (Some(OwnedTx::ReplacementHistory(_)), _) => Vec::new(),
        };
        if let OwnedTx::Accepted(candidate) = after {
            available.extend(self.collect_released_replacement_inputs(candidate, removals)?);
        }
        let control = self
            .dependencies
            .plan_events(available, lost, DependencyCut(sequence))?
            .unwrap_or_default();
        let delta = if existing.is_some() {
            self.dependencies.plan_replacements(changes)?
        } else {
            self.dependencies.plan_primary_replacements(changes)?
        };
        Ok(delta.with_control(control.into(), &self.dependencies)?)
    }

    fn plan_direct_absent_dependency_delta(
        &self,
        after: &OwnedTx,
        sequence: ApplySequence,
    ) -> Result<Option<DependencyBatchDelta>, PlanError> {
        let record = after.record();
        let origin = DependencyOrigin::Transaction(record.identity.raw.clone());
        let (origin_occupied, origin_keys) = self.dependencies.classify_empty_origin(&origin)?;
        if origin_occupied {
            return Ok(None);
        }
        let mut available = Vec::new();
        available
            .try_reserve(
                record
                    .tx
                    .data()
                    .raw()
                    .outputs()
                    .len()
                    .checked_add(0)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
            )
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        available.extend(record.tx.output_pts().into_iter().map(DependencyKey::Cell));
        let control = self
            .dependencies
            .plan_events_with_origin_expectation(
                available,
                Vec::new(),
                DependencyCut(sequence),
                origin,
                origin_keys,
            )?
            .unwrap_or_default();
        Ok(Some(
            self.dependencies
                .plan_primary_replacements(std::iter::once((None, Some(after))))?
                .with_control(control.into(), &self.dependencies)?,
        ))
    }

    /// Compute availability from the projected final spender/producer set.
    /// Removal order is irrelevant: an input is published only when the new
    /// candidate does not spend it and its backing cell survives on chain or
    /// in Accepted membership.
    fn collect_released_replacement_inputs(
        &self,
        candidate: &AcceptedEntry,
        removals: &[MembershipRemoval],
    ) -> Result<Vec<DependencyKey>, PlanError> {
        if removals.is_empty() {
            return Ok(Vec::new());
        }
        let owners = self.entries.read_all();
        let mut removed = HashSet::new();
        removed
            .try_reserve(removals.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        removed.extend(removals.iter().map(|removal| removal.hash.clone()));

        let candidate_footprint = &candidate.proof.payload().footprint;
        let mut candidate_inputs = HashSet::new();
        candidate_inputs
            .try_reserve(candidate_footprint.inputs().len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        candidate_inputs.extend(candidate_footprint.inputs().iter().cloned());
        let final_owners = ProjectedFinalOwnerSet {
            removed: ProjectedRemovalSet::Replacement(&removed),
        };

        let capacity = removals.iter().try_fold(0usize, |total, removal| {
            let victim = match owners.get(&removal.hash) {
                Some(OwnedTx::Accepted(entry)) => entry,
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            total
                .checked_add(victim.proof.payload().footprint.inputs().len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
        })?;
        let mut available = Vec::new();
        available
            .try_reserve(capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;

        for removal in removals {
            let victim = match owners.get(&removal.hash) {
                Some(OwnedTx::Accepted(entry)) => entry,
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            for input in victim.proof.payload().footprint.inputs() {
                if self
                    .released_input_backing_in_final_owner_set(
                        victim,
                        input,
                        final_owners,
                        ReleasedInputContext::Replacement {
                            candidate_inputs: &candidate_inputs,
                        },
                    )?
                    .is_available()
                {
                    available.push(DependencyKey::Cell(input.clone()));
                }
            }
        }
        Ok(available)
    }

    /// Availability created by a total administrative removal. The projected
    /// final owner set is the authority: an input is released only when its
    /// current Accepted spender leaves and the backing cell survives on chain
    /// or under a non-removed Accepted parent.
    fn collect_released_administrative_inputs(
        &self,
        removals: &AcceptedRemovalSet,
    ) -> Result<Vec<DependencyKey>, PlanError> {
        let owners = self.entries.read_all();
        let mut snapshots = Vec::new();
        snapshots
            .try_reserve_exact(removals.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in removals.iter() {
            let entry = match owners.get(hash) {
                Some(OwnedTx::Accepted(entry)) => Ok((hash, entry)),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    Err(PlanError::Fault(AuthorityFault::MembershipProjection))
                }
            }?;
            snapshots.push(entry);
        }
        self.collect_released_administrative_inputs_from(snapshots, removals)
            .map(|released| released.keys)
    }

    fn collect_released_administrative_inputs_from<'entry>(
        &self,
        snapshots: impl IntoIterator<Item = (&'entry RawTxHash, &'entry AcceptedEntry)>,
        removals: &AcceptedRemovalSet,
    ) -> Result<AdministrativeReleasedInputs, PlanError> {
        let snapshots = snapshots.into_iter();
        let mut available = Vec::new();
        let mut parents = Vec::new();
        if let Some(capacity) = snapshots.size_hint().1 {
            available
                .try_reserve(capacity)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            parents
                .try_reserve(capacity)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        }
        let final_owners = ProjectedFinalOwnerSet {
            removed: ProjectedRemovalSet::Administrative(removals),
        };
        for (hash, entry) in snapshots {
            available
                .try_reserve(entry.proof.payload().footprint.inputs().len())
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            parents
                .try_reserve(entry.proof.payload().footprint.inputs().len())
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            for input in entry.proof.payload().footprint.inputs() {
                match self.released_input_backing_in_final_owner_set(
                    entry,
                    input,
                    final_owners,
                    ReleasedInputContext::Administrative { victim: hash },
                )? {
                    ReleasedInputBacking::Unavailable => {}
                    ReleasedInputBacking::Chain => {
                        available.push(DependencyKey::Cell(input.clone()));
                    }
                    ReleasedInputBacking::Pool(parent) => {
                        available.push(DependencyKey::Cell(input.clone()));
                        parents.push(parent);
                    }
                }
            }
        }
        parents.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));
        for adjacent in parents.array_windows::<2>() {
            let [left, right] = adjacent;
            if left.hash == right.hash && left.version != right.version {
                return Err(PlanError::Stale(StalePlan::Version));
            }
        }
        parents.dedup();
        Ok(AdministrativeReleasedInputs {
            keys: available,
            parents,
        })
    }

    /// Decide one removed input from the projected final membership set. The
    /// context owns only the distinct spender premise; backing-cell survival
    /// has one implementation for replacement and administrative cohorts.
    fn released_input_backing_in_final_owner_set(
        &self,
        removed_entry: &AcceptedEntry,
        input: &OutPoint,
        final_owners: ProjectedFinalOwnerSet<'_>,
        context: ReleasedInputContext<'_>,
    ) -> Result<ReleasedInputBacking, PlanError> {
        match context {
            ReleasedInputContext::Replacement { candidate_inputs } => {
                if candidate_inputs.contains(input) {
                    return Ok(ReleasedInputBacking::Unavailable);
                }
                let spender = self
                    .membership
                    .spender(input)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                if !final_owners.contains_removed(&spender) {
                    return Ok(ReleasedInputBacking::Unavailable);
                }
            }
            ReleasedInputContext::Administrative { victim } => {
                if self.membership.spender(input) != Some(victim.clone()) {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            }
        }
        if removed_entry.proof.is_chain_input(input) {
            return Ok(ReleasedInputBacking::Chain);
        }
        let parent = RawTxHash(input.tx_hash());
        if final_owners.contains_removed(&parent) {
            return Ok(ReleasedInputBacking::Unavailable);
        }
        let owner = self.entries.get(&parent);
        let Some(OwnedTx::Accepted(parent)) = owner.as_deref() else {
            return Ok(ReleasedInputBacking::Unavailable);
        };
        let index: u32 = input.index().unpack();
        Ok(
            if usize::try_from(index)
                .ok()
                .is_some_and(|index| index < parent.record.tx.outputs().len())
            {
                ReleasedInputBacking::Pool(AcceptedBackingParent {
                    hash: parent.record.identity.raw.clone(),
                    version: parent.record.version,
                })
            } else {
                ReleasedInputBacking::Unavailable
            },
        )
    }

    fn plan_owner_sources<'entry>(
        &self,
        replacements: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
    ) -> Result<ShardOwnerSourcePlan, PlanError> {
        self.entries
            .plan_owner_sources(replacements)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
    }

    fn plan_membership_owner_derivations(
        &self,
        key: &RawTxHash,
        existing: Option<&OwnedTx>,
        after: &OwnedTx,
        removals: &[MembershipRemoval],
        sequence: ApplySequence,
    ) -> Result<DerivedOwnerDelta, PlanError> {
        let change_capacity = removals
            .len()
            .checked_add(1)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        if removals.is_empty() {
            let (_entries, indexes, source_versions) = self.owner_derivation_parts();
            let indexes = indexes.plan_replace(key, existing, Some(after))?;
            let sources = source_versions
                .plan_replacements(std::iter::once((existing, Some(after))), sequence);
            let template_sources =
                self.plan_owner_sources(std::iter::once((key, existing, Some(after))))?;
            return Ok(DerivedOwnerDelta {
                indexes,
                sources,
                template_sources,
            });
        }
        let mut removed_owners = Vec::new();
        removed_owners
            .try_reserve(removals.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for removal in removals {
            let removed = self
                .entries
                .get(&removal.hash)
                .as_deref()
                .cloned()
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            removed_owners.push((removal, removed));
        }
        let mut changes = Vec::new();
        changes
            .try_reserve(change_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        changes.push((key, existing, Some(after)));
        for (removal, removed) in &removed_owners {
            changes.push((&removal.hash, Some(removed), removal.after()));
        }
        let (_entries, indexes, source_versions) = self.owner_derivation_parts();
        let sources = source_versions.plan_replacements(
            changes.iter().map(|(_, before, after)| (*before, *after)),
            sequence,
        );
        let template_sources = self.plan_owner_sources(
            changes
                .iter()
                .map(|(key, before, after)| (*key, *before, *after)),
        )?;
        let indexes = indexes.plan_replacements(changes)?;
        Ok(DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        })
    }

    fn plan_direct_absent_owner_derivations(
        &self,
        key: &RawTxHash,
        after: &OwnedTx,
        sequence: ApplySequence,
    ) -> Result<DerivedOwnerDelta, PlanError> {
        let (_entries, indexes, _source_versions) = self.owner_derivation_parts();
        let indexes = indexes.plan_replace(key, None, Some(after))?;
        let sources = AuthoritySourceVersions::plan_template_selection_replacements(
            std::iter::once((None, Some(after))),
            sequence,
        );
        let template_sources =
            self.plan_owner_sources(std::iter::once((key, None, Some(after))))?;
        Ok(DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        })
    }

    /// Build the optional Accepted-victim continuation as one all-or-none
    /// cohort. Allocation pressure drops the optional history before any
    /// authoritative mutation; structural failures remain explicit faults.
    fn retain_replacement_history(
        &self,
        candidate: &AcceptedEntry,
        removals: &mut [MembershipRemoval],
        sequence: ApplySequence,
    ) -> Result<bool, PlanError> {
        if !removals
            .iter()
            .any(|removal| removal.cause == RemovalCause::Replacement)
        {
            return Ok(true);
        }
        let mut removed = HashSet::new();
        if removed.try_reserve(removals.len()).is_err() {
            return Ok(false);
        }
        removed.extend(removals.iter().map(|removal| removal.hash.clone()));

        // ExpandedFootprint canonicalizes inputs into sorted unique order, so
        // RBF-only trigger derivation needs no second candidate-input index.
        let candidate_inputs = candidate.proof.payload().footprint.inputs();
        for removal in removals.iter_mut() {
            if removal.cause != RemovalCause::Replacement {
                continue;
            }
            let owner = self.entries.get(&removal.hash);
            let accepted = match owner.as_deref() {
                Some(OwnedTx::Accepted(entry)) => entry,
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            let footprint = &accepted.proof.payload().footprint;
            let trigger_capacity = footprint
                .inputs()
                .len()
                .checked_add(footprint.dependencies().len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let mut trigger_keys = Vec::new();
            if trigger_keys.try_reserve(trigger_capacity).is_err() {
                return Ok(false);
            }
            for input in footprint.inputs() {
                let producer_removed = removed.contains(&RawTxHash(input.tx_hash()));
                if candidate_inputs.binary_search(input).is_ok()
                    || (producer_removed && !accepted.proof.is_chain_input(input))
                {
                    trigger_keys.push(DependencyKey::Cell(input.clone()));
                }
            }
            for dependency in footprint.dependencies() {
                if removed.contains(&RawTxHash(dependency.tx_hash()))
                    && !accepted.proof.is_chain_dependency(dependency)
                {
                    trigger_keys.push(DependencyKey::Cell(dependency.clone()));
                }
            }
            let recovery_triggers = match MissingDependencies::new(trigger_keys, trigger_capacity) {
                Ok(triggers) => triggers,
                Err(super::state::DependencySetError::Allocation) => {
                    return Ok(false);
                }
                Err(
                    super::state::DependencySetError::Empty
                    | super::state::DependencySetError::TooMany
                    | super::state::DependencySetError::Arithmetic,
                ) => return Err(PlanError::Fault(AuthorityFault::MembershipProjection)),
            };
            let history = match ReplacementHistoryEntry::from_accepted(
                accepted,
                recovery_triggers,
                self.resources
                    .replacement_history_charge(
                        &accepted.record.tx,
                        accepted.proof.payload().dependencies().len(),
                    )
                    .map_err(Self::membership_resource_error)?,
                // Resource projection and trigger validation do not depend on
                // the fresh history identity. The accepted identity is a
                // non-authoritative placeholder which is replaced only after
                // the complete optional cohort is proven retainable.
                accepted.record.version,
                accepted.record.arrival,
                DependencyCut(sequence),
            ) {
                Ok(history) => history,
                Err(ReplacementHistoryError::ResourceAllocation) => {
                    return Ok(false);
                }
                Err(ReplacementHistoryError::InvalidRecoveryTrigger) => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                Err(ReplacementHistoryError::ResourceArithmetic) => {
                    return Err(PlanError::Fault(AuthorityFault::CounterExhausted));
                }
            };
            removal.retain_replacement_history(history)?;
        }
        Ok(true)
    }

    fn plan_membership_resources(
        &self,
        key: &RawTxHash,
        before: Option<&OwnedTx>,
        after: &OwnedTx,
        removals: &[MembershipRemoval],
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let capacity = removals
            .len()
            .checked_add(1)
            .ok_or(ResourceError::Arithmetic)?;
        let mut changes = Vec::new();
        changes
            .try_reserve(capacity)
            .map_err(|_| ResourceError::Allocation)?;
        changes.push((
            key.clone(),
            before.map(OwnedTx::charge_record),
            Some(after.charge_record()),
        ));
        for removal in removals {
            let victim = self
                .entries
                .get(&removal.hash)
                .ok_or(ResourceError::ExistingChargeMismatch)?;
            changes.push((
                removal.hash.clone(),
                Some(victim.charge_record()),
                removal.after().map(OwnedTx::charge_record),
            ));
        }
        self.resources_for_plan().plan_batch(changes)
    }

    fn plan_sparse_membership_resources(
        &self,
        key: &RawTxHash,
        before: Option<&OwnedTx>,
        after: &OwnedTx,
        removals: &[MembershipRemoval],
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let capacity = removals
            .len()
            .checked_add(1)
            .ok_or(ResourceError::Arithmetic)?;
        let mut changes = Vec::new();
        changes
            .try_reserve_exact(capacity)
            .map_err(|_| ResourceError::Allocation)?;
        changes.push((
            key.clone(),
            before.map(OwnedTx::charge_record),
            Some(after.charge_record()),
        ));
        for removal in removals {
            let victim = self
                .entries
                .get(&removal.hash)
                .ok_or(ResourceError::ExistingChargeMismatch)?;
            changes.push((
                removal.hash.clone(),
                Some(victim.charge_record()),
                removal.after().map(OwnedTx::charge_record),
            ));
        }
        self.resources_for_plan()
            .plan_shared_transition_batch(changes)
    }

    fn membership_resource_error(error: ResourceError) -> PlanError {
        match error {
            ResourceError::Allocation => PlanError::Backpressure(Backpressure::Allocation),
            ResourceError::Arithmetic
            | ResourceError::PreAcceptedLimit
            | ResourceError::RemoteLimit
            | ResourceError::PeerLimit(_)
            | ResourceError::ReplacementHistoryLimit
            | ResourceError::AcceptedLimit
            | ResourceError::ExistingChargeMismatch
            | ResourceError::DuplicateChange
            | ResourceError::ComputeEnvelope
            | ResourceError::AttributionMismatch
            | ResourceError::CapacityBankFault => {
                PlanError::Fault(AuthorityFault::ResourceProjection)
            }
        }
    }

    fn plan_dependency_loss<'entry>(
        &self,
        parents: impl IntoIterator<Item = &'entry OwnedTx>,
        sequence: ApplySequence,
    ) -> Result<DependencyEntryControlDelta, PlanError> {
        let loss = self.collect_dependency_loss_keys(parents)?;
        Ok(self
            .dependencies
            .plan_events(Vec::new(), loss.keys, DependencyCut(sequence))?
            .unwrap_or_default())
    }

    fn collect_dependency_loss_keys<'entry>(
        &self,
        parents: impl IntoIterator<Item = &'entry OwnedTx>,
    ) -> Result<DependencyLossKeys, PlanError> {
        Self::collect_dependency_loss_keys_from(&self.dependencies, parents)
    }

    fn collect_dependency_loss_keys_from<'entry>(
        dependencies: &DependencyFrontier,
        parents: impl IntoIterator<Item = &'entry OwnedTx>,
    ) -> Result<DependencyLossKeys, PlanError> {
        let mut keys = Vec::new();
        #[cfg(test)]
        let mut work = DependencyLossWork::default();
        for parent in parents {
            let record = parent.record();
            let output_count = record.tx.data().raw().outputs().len();
            let origin = DependencyOrigin::Transaction(record.identity.raw.clone());
            let origin_keys = dependencies.keys_for_origin(&origin)?;
            let origin_count = origin_keys.as_ref().map_or(0, |keys| keys.len());
            let additional = output_count
                .checked_add(origin_count)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            keys.try_reserve(additional)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            keys.extend(record.tx.output_pts().into_iter().map(DependencyKey::Cell));
            if let Some(origin_keys) = origin_keys {
                keys.extend(origin_keys.iter().cloned());
            }
            #[cfg(test)]
            {
                work.output_keys = work
                    .output_keys
                    .checked_add(output_count)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                work.indexed_origin_keys = work
                    .indexed_origin_keys
                    .checked_add(origin_count)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            }
        }
        Ok(DependencyLossKeys {
            keys,
            #[cfg(test)]
            work,
        })
    }

    fn plan_charged_admission(
        &mut self,
        admission: ChargedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        // Closing the effect authority freezes every new owner transition.
        // Reject it before allocating an owner identity or Apply stamp.
        self.effects.lock().ensure_open()?;
        let key = admission.admission().identity.raw.clone();
        let existing = self.entries.get(&key).as_deref().cloned();
        if let Some(existing) = existing {
            return self.plan_existing_admission(key, existing, admission);
        }
        let (admission, charge) = admission.into_parts();
        if self
            .indexes
            .proposal_owner(&admission.identity.proposal)
            .is_some()
        {
            return Err(PlanError::Backpressure(Backpressure::ProposalCollision));
        }

        self.reserve_primary_owner_insertions(std::iter::once(&key))?;
        let planned_charge = ChargeRecord::PreAccepted {
            resources: charge,
            residency_peer: admission.source.ingress_peer(),
            compute_peer: None,
        };
        let resource =
            self.resources_for_plan()
                .plan_replace(key.clone(), None, Some(planned_charge))?;

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, arrival, clocks) = clocks.insertion()?;
        let payload_bytes = admission.payload_bytes;
        let encoded_edges = admission.encoded_edges;
        let dependencies = admission.dependencies;
        let record = TxRecord {
            tx: admission.tx,
            identity: admission.identity,
            version,
            arrival,
        };
        let after = OwnedTx::PreAccepted(PreAcceptedEntry {
            record,
            source: admission.source,
            basis: AdmissionBasis::new(dependencies, payload_bytes, encoded_edges, charge),
            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
            charge,
        });
        self.prepare_entry_delta_with_controls(
            EntryTransition::Insert { key, after },
            clocks.finish(),
            sequence,
            TransitionControls::none(),
            Some(resource),
        )
    }

    #[cfg(test)]
    fn plan_single_effect(
        &mut self,
        policy: EffectPolicy,
        effect: CommittedEffect,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let publication = self
            .effects
            .lock()
            .build_single_publication(policy, effect)
            .map_err(PlanError::from)?;
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;
        Ok(self.prepared_effect_only(effect, clocks))
    }

    fn plan_existing_admission(
        &mut self,
        key: RawTxHash,
        existing: OwnedTx,
        admission: ChargedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let (admission, admission_charge) = admission.into_parts();
        if let OwnedTx::ReplacementHistory(history) = &existing {
            return self.plan_replacement_history_admission(
                key,
                existing.clone(),
                history.clone(),
                admission,
                admission_charge,
            );
        }
        let OwnedTx::PreAccepted(entry) = &existing else {
            // Accepted ownership is by raw hash. A different witness cannot
            // replace a committed membership proof, but it is still the same
            // already-owned transaction rather than a second payload owner.
            return Err(PlanError::Duplicate);
        };
        let same_witness = entry.record.identity.witness == admission.identity.witness;
        let PreAcceptedSource::Proposal {
            base: ProposalBase::Trusted,
        } = admission.source
        else {
            return Err(if same_witness {
                PlanError::Duplicate
            } else {
                PlanError::PayloadVariant
            });
        };

        let proposal_base = match entry.source {
            PreAcceptedSource::Remote(remote) => ProposalBase::Remote(remote.residency),
            PreAcceptedSource::Proposal { base } => {
                if same_witness {
                    return Err(PlanError::Duplicate);
                }
                base
            }
            PreAcceptedSource::Recovery(_) => {
                return Err(if same_witness {
                    PlanError::Duplicate
                } else {
                    PlanError::PayloadVariant
                });
            }
        };

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (promoted, clocks) = if same_witness {
            // A policy-only promotion/refresh preserves EntryVersion and the
            // exact active lease. ActiveWork has already sealed its compute
            // attribution, so changing future scheduling trust cannot move
            // that capability between resource partitions.
            let mut promoted = entry.clone();
            promoted.source = PreAcceptedSource::Proposal {
                base: proposal_base,
            };
            if matches!(promoted.phase, PreAcceptedPhase::Waiting(_)) {
                // Missing-dependency disposition is source policy: Remote may
                // wait for an external parent, while Proposal may wait only
                // for a parent already owned by the pre-pool. Reusing the old
                // Waiting observation after promotion would remove the owner
                // from both Remote expiry/relay rebuild and executable work.
                // Return to the existing Resolve level so the new source is
                // adjudicated once; no extra state or repair scan is needed.
                promoted.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
                promoted.charge = promoted.original_charge();
            }
            (promoted, clocks)
        } else {
            let (version, clocks) = clocks.replacement()?;
            let promoted = PreAcceptedEntry {
                record: TxRecord {
                    tx: admission.tx,
                    identity: admission.identity,
                    version,
                    arrival: entry.record.arrival,
                },
                source: PreAcceptedSource::Proposal {
                    base: proposal_base,
                },
                basis: AdmissionBasis::new(
                    admission.dependencies,
                    admission.payload_bytes,
                    admission.encoded_edges,
                    admission_charge,
                ),
                phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                charge: admission_charge,
            };
            (promoted, clocks)
        };
        // The exact old owner is always carried beyond the authority guard.
        // A checked-out worker still holds the old version and can only return
        // a typed stale completion.
        self.prepare_entry_delta(
            EntryTransition::Replace {
                key,
                before: existing,
                after: OwnedTx::PreAccepted(promoted),
            },
            clocks.finish(),
            sequence,
        )
    }

    fn plan_replacement_history_admission(
        &mut self,
        key: RawTxHash,
        existing: OwnedTx,
        history: ReplacementHistoryEntry,
        admission: ValidatedAdmission,
        admission_charge: ResourceVector,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let same_witness = history.record().identity.witness == admission.identity.witness;
        let PreAcceptedSource::Proposal {
            base: ProposalBase::Trusted,
        } = admission.source
        else {
            return Err(if same_witness {
                PlanError::Duplicate
            } else {
                PlanError::PayloadVariant
            });
        };
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let arrival = history.record().arrival;
        let promoted = if same_witness {
            let mut promoted = history.into_recovery(self.generation, version);
            promoted.source = PreAcceptedSource::Proposal {
                base: ProposalBase::Trusted,
            };
            promoted
        } else {
            PreAcceptedEntry {
                record: TxRecord {
                    tx: admission.tx,
                    identity: admission.identity,
                    version,
                    arrival,
                },
                source: PreAcceptedSource::Proposal {
                    base: ProposalBase::Trusted,
                },
                basis: AdmissionBasis::new(
                    admission.dependencies,
                    admission.payload_bytes,
                    admission.encoded_edges,
                    admission_charge,
                ),
                phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                charge: admission_charge,
            }
        };
        self.prepare_entry_delta(
            EntryTransition::Replace {
                key,
                before: existing,
                after: OwnedTx::PreAccepted(promoted),
            },
            clocks.finish(),
            sequence,
        )
    }

    #[cfg(test)]
    pub(in crate::authority) fn plan_final_admission(
        &mut self,
        outcome: FinalAdmissionValidationOutcome,
    ) -> Result<FinalAdmissionDispositionPlan<'_>, PlanError> {
        match outcome {
            FinalAdmissionValidationOutcome::Candidate(receipt) => self
                .plan_candidate_disposition(receipt)
                .map(FinalAdmissionDispositionPlan::Candidate),
            FinalAdmissionValidationOutcome::Rejected(rejection) => self
                .plan_final_validation_rejection(rejection)
                .map(FinalAdmissionDispositionPlan::ValidationRejected),
            FinalAdmissionValidationOutcome::Reresolve(retry) => self
                .plan_final_reresolution(retry)
                .map(FinalAdmissionDispositionPlan::Reresolve),
        }
    }

    /// Compile one validated Ready head without lending the planner the outer
    /// authority writer. Candidate membership uses the canonical exact policy
    /// compiler; non-malformed rejection and re-resolution bind the validated
    /// dependency cut into their final mixed shard cut; malformed Remote input
    /// reuses the sole staged peer-revocation engine.
    pub(in crate::authority) fn prepare_shared_ready_head_disposition(
        &self,
        outcome: FinalAdmissionValidationOutcome,
    ) -> Result<PreparedSharedReadyHeadDisposition<'_>, PlanError> {
        match outcome {
            FinalAdmissionValidationOutcome::Candidate(receipt) => {
                let delta = self.compile_shared_candidate_disposition_delta(receipt)?;
                let compiled = self.seal_shared_independent(delta)?;
                Ok(PreparedSharedReadyHeadDisposition::Effectful {
                    authority: self,
                    compiled,
                })
            }
            FinalAdmissionValidationOutcome::Rejected(rejection) => {
                let (subject, reason) = rejection.into_parts();
                let preaccepted = self.final_admission_subject_owner(&subject)?;
                if reason.is_malformed()
                    && let Some(peer) = preaccepted.source.payload_blame_peer()
                {
                    let culprit = preaccepted.record.identity.raw;
                    let core = self.compile_shared_peer_revocation_core(peer, culprit, reason)?;
                    return Ok(PreparedSharedReadyHeadDisposition::PeerRevocation(core));
                }

                let key = subject.key().clone();
                let evidence = self
                    .dependencies
                    .capture_settlement_evidence(&key, preaccepted.dependencies(), None, None)
                    .map_err(PlanError::from)?;
                let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
                let audience = RejectionAudience::from_source(preaccepted.source);
                let publication = self
                    .effects
                    .lock()
                    .build_single_publication(
                        policy,
                        CommittedEffect::Rejected(CommittedRejection::Validation {
                            tx: Arc::clone(&preaccepted.record.tx),
                            audience,
                            reason,
                        }),
                    )
                    .map_err(PlanError::from)?;
                self.effects.lock().preflight_publication(&publication)?;
                let existing = OwnedTx::PreAccepted(preaccepted);
                let clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
                let sequence = clocks.sequence();
                let dependency = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
                let effect = self
                    .effects_for_plan()
                    .plan_publication(&publication, sequence)?;
                let delta = self.compile_entry_delta_with_controls(
                    EntryTransition::Remove {
                        key,
                        before: existing,
                    },
                    clocks.finish(),
                    sequence,
                    TransitionControls::dependency_and_effect(dependency, effect),
                    None,
                )?;
                let delta = delta.into_shared_terminalization(
                    self,
                    Some(evidence),
                    MembershipPolicyWitness::default(),
                )?;
                let compiled = self.seal_shared_independent(delta)?;
                Ok(PreparedSharedReadyHeadDisposition::Effectful {
                    authority: self,
                    compiled,
                })
            }
            FinalAdmissionValidationOutcome::Reresolve(retry) => {
                let subject = retry.into_subject();
                let preaccepted = self.final_admission_subject_owner(&subject)?;
                let key = subject.key().clone();
                let evidence = self
                    .dependencies
                    .capture_settlement_evidence(&key, preaccepted.dependencies(), None, None)
                    .map_err(PlanError::from)?;
                let clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
                let sequence = clocks.sequence();
                let (version, clocks) = clocks.replacement()?;
                let mut requeued = preaccepted.clone();
                requeued.record.version = version;
                requeued.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
                requeued.charge = preaccepted.original_charge();
                let delta = self.compile_entry_delta_with_controls(
                    EntryTransition::Replace {
                        key,
                        before: OwnedTx::PreAccepted(preaccepted),
                        after: OwnedTx::PreAccepted(requeued),
                    },
                    clocks.finish(),
                    sequence,
                    TransitionControls::none(),
                    None,
                )?;
                let (delta, support) = delta.into_shared_entry(self, Some(evidence))?;
                Ok(PreparedSharedReadyHeadDisposition::Reresolve {
                    authority: self,
                    compiled: CompiledSharedReadyReresolution {
                        generation: self.generation,
                        chain_view: self.chain_view.clone(),
                        delta,
                        support,
                    },
                })
            }
        }
    }

    fn final_admission_subject_owner(
        &self,
        subject: &FinalAdmissionSubject,
    ) -> Result<PreAcceptedEntry, PlanError> {
        if subject.view() != &self.chain_view {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }
        let existing = self
            .entries
            .get(subject.key())
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != subject.expected() {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &*existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Ready(verified) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if verified.dependency_cut() != subject.dependency_cut()
            || !self
                .dependencies
                .proof_is_current(verified.payload().dependencies(), subject.dependency_cut())
        {
            return Err(PlanError::Stale(StalePlan::Dependency));
        }
        Ok(preaccepted.clone())
    }

    #[cfg(test)]
    fn plan_final_validation_rejection(
        &mut self,
        rejection: FinalAdmissionRejection,
    ) -> Result<PreparedValidationRejection<'_>, PlanError> {
        let (subject, reason) = rejection.into_parts();
        let preaccepted = self.final_admission_subject_owner(&subject)?;
        let blame_peer = preaccepted.source.payload_blame_peer();
        let audience = RejectionAudience::from_source(preaccepted.source);
        if reason.is_malformed()
            && let Some(peer) = blame_peer
        {
            let plan =
                self.plan_peer_revocation(peer, preaccepted.record.identity.raw, reason.clone())?;
            return Ok(PreparedValidationRejection { reason, plan });
        }
        let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        let publication = self
            .effects
            .lock()
            .build_single_publication(
                policy,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience,
                    reason: reason.clone(),
                }),
            )
            .map_err(PlanError::from)?;
        let plan =
            self.plan_preaccepted_terminalization(subject.key(), subject.expected(), &publication)?;
        Ok(PreparedValidationRejection { reason, plan })
    }

    #[cfg(test)]
    fn plan_final_reresolution(
        &mut self,
        retry: FinalAdmissionRetry,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let subject = retry.into_subject();
        let preaccepted = self.final_admission_subject_owner(&subject)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let mut requeued = preaccepted.clone();
        requeued.record.version = version;
        requeued.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
        requeued.charge = preaccepted.original_charge();
        self.prepare_entry_delta(
            EntryTransition::Replace {
                key: subject.key().clone(),
                before: OwnedTx::PreAccepted(preaccepted),
                after: OwnedTx::PreAccepted(requeued),
            },
            clocks.finish(),
            sequence,
        )
    }

    /// Compile the only final-membership command exposed to production.  Both
    /// success and policy rejection are complete owner/resource/effect Plans;
    /// the caller cannot receive a bare membership error after validation and
    /// then independently decide whether to terminalize or publish it.
    #[cfg(test)]
    fn plan_candidate_disposition(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<CandidateDispositionPlan<'_>, PlanError> {
        let key = receipt.key().clone();
        let expected = receipt.expected();
        match self.prepare_accept_delta(receipt) {
            Ok(delta) => Ok(CandidateDispositionPlan::Accepted(PreparedApply::stage(
                self,
                DependencyAuthorityDelta::Membership(delta),
            )?)),
            Err(PlanError::Membership(reason)) => {
                let (policy, tx, audience) = {
                    let existing = self
                        .entries
                        .get(&key)
                        .ok_or(PlanError::Stale(StalePlan::Missing))?;
                    let OwnedTx::PreAccepted(preaccepted) = &*existing else {
                        return Err(PlanError::Stale(StalePlan::Phase));
                    };
                    (
                        EffectPolicy::for_preaccepted_source(preaccepted.source),
                        Arc::clone(&preaccepted.record.tx),
                        RejectionAudience::from_source(preaccepted.source),
                    )
                };
                let publication = self
                    .effects
                    .lock()
                    .build_single_publication(
                        policy,
                        CommittedEffect::Rejected(CommittedRejection::Membership {
                            tx,
                            audience,
                            reason: reason.clone(),
                        }),
                    )
                    .map_err(PlanError::from)?;
                let plan = self.plan_preaccepted_terminalization(&key, expected, &publication)?;
                Ok(CandidateDispositionPlan::Rejected(
                    PreparedCandidateRejection { reason, plan },
                ))
            }
            Err(error) => Err(error),
        }
    }

    /// Compile every Local candidate through the same exact membership
    /// evaluator used by Ready while holding only the shared generation
    /// barrier. A preseeded witness binds the owner snapshot used for Direct
    /// provenance/arrival to all later policy reads. Duplicate and policy
    /// rejection retain that witness through effect activation because they
    /// have no owner mutation to serve as an earlier linearization point.
    pub(super) fn prepare_shared_direct_admission(
        &self,
        receipt: DirectAdmissionReceipt,
    ) -> Result<PreparedSharedDirectAdmissionDisposition<'_>, PlanError> {
        self.effects.lock().ensure_open()?;
        self.validate_direct_acceptance_evidence(&receipt)?;
        let key = receipt.key().clone();
        let provisional_clocks = self.clocks.snapshot();
        let provisional_existing = self.entries.get(&key).as_deref().cloned();
        let (sizing_candidate, _) = Self::direct_candidate(
            receipt.clone(),
            provisional_existing.as_ref(),
            provisional_clocks.next_version,
            provisional_clocks.next_arrival,
        );
        let (existing, witness) =
            self.capture_direct_membership_subject(&key, &sizing_candidate)?;

        if matches!(&existing, Some(OwnedTx::Accepted(_))) {
            let publication = self
                .effects
                .lock()
                .build_single_publication(
                    EffectPolicy::Trusted,
                    CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                        tx_hash: key.clone(),
                        requesting_peer: None,
                    }),
                )
                .map_err(PlanError::from)?;
            let effect = self.prepare_shared_direct_membership_effect(
                DirectMembershipEffectResult::Duplicate(key),
                witness,
                publication,
            )?;
            return Ok(PreparedSharedDirectAdmissionDisposition::EffectOnly(effect));
        }

        let clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
        let (version, arrival, clocks) = match &existing {
            Some(owner) => {
                let (version, clocks) = clocks.replacement()?;
                (version, owner.record().arrival, clocks)
            }
            None => clocks.insertion()?,
        };
        let (accepted, async_process_start) =
            Self::direct_candidate(receipt, existing.as_ref(), version, arrival);
        match self.evaluate_direct_membership_policy(&key, &accepted, witness)? {
            MembershipPolicyOutcome::Accepted(evaluation) => {
                let prepared =
                    self.prepare_direct_membership_after_evaluation(&key, &accepted, evaluation)?;
                let vacant_leaf = existing.is_none() && prepared.removals.is_empty();
                let resource = if vacant_leaf {
                    let mut insertions = Vec::new();
                    insertions
                        .try_reserve_exact(1)
                        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                    insertions.push((key.clone(), accepted.charge_record()));
                    Some(
                        self.resources_for_plan()
                            .plan_direct_accepted_insertion_batch(insertions)
                            .map_err(|error| match error {
                                DirectAcceptedInsertionError::Contended(wait) => {
                                    PlanError::ResourceContended(wait)
                                }
                                DirectAcceptedInsertionError::Resource(
                                    ResourceError::AcceptedLimit,
                                ) => PlanError::Stale(StalePlan::AcceptedObservation),
                                DirectAcceptedInsertionError::Resource(error) => {
                                    Self::membership_resource_error(error)
                                }
                            })?,
                    )
                } else {
                    None
                };
                let (dependency, scheduler, owners) = if vacant_leaf {
                    let after = OwnedTx::Accepted(accepted.clone());
                    (
                        self.plan_direct_absent_dependency_delta(&after, clocks.sequence())?,
                        Some(SchedulerDelta::shared_absent_accepted()),
                        Some(self.plan_direct_absent_owner_derivations(
                            &key,
                            &after,
                            clocks.sequence(),
                        )?),
                    )
                } else {
                    (None, None, None)
                };
                let delta = self.compile_membership_delta(MembershipCompilation {
                    key,
                    existing,
                    accepted,
                    prepared,
                    clocks,
                    effects: MembershipEffects::Publish(EffectPolicy::Trusted),
                    async_process_start,
                    resource,
                    dependency,
                    scheduler,
                    owners,
                    sparse_resource: true,
                })?;
                let compiled = self.seal_shared_independent(delta.into_shared_exact()?)?;
                Ok(PreparedSharedDirectAdmissionDisposition::Accepted {
                    authority: self,
                    compiled,
                })
            }
            MembershipPolicyOutcome::Rejected(rejection) => {
                drop(clocks);
                let (reason, witness) = rejection.into_parts();
                let publication = self
                    .effects
                    .lock()
                    .build_single_publication(
                        EffectPolicy::Trusted,
                        CommittedEffect::Rejected(CommittedRejection::Membership {
                            tx: Arc::clone(&accepted.record.tx),
                            audience: RejectionAudience::default(),
                            reason: reason.clone(),
                        }),
                    )
                    .map_err(PlanError::from)?;
                let effect = self.prepare_shared_direct_membership_effect(
                    DirectMembershipEffectResult::Rejected(reason),
                    witness,
                    publication,
                )?;
                Ok(PreparedSharedDirectAdmissionDisposition::EffectOnly(effect))
            }
        }
    }

    fn prepare_shared_direct_membership_effect(
        &self,
        result: DirectMembershipEffectResult,
        read_witness: MembershipPolicyWitness,
        publication: EffectPublication,
    ) -> Result<PreparedSharedDirectMembershipEffect<'_>, PlanError> {
        let read_fence = read_witness.bind(self).map_err(PlanError::Stale)?;
        drop(read_fence);
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, clocks.sequence())?;
        let staged_effect = EffectLog::stage_publication(&self.effects, effect)?;
        Ok(PreparedSharedDirectMembershipEffect {
            authority: self,
            result,
            read_witness,
            staged_effect,
            clocks: clocks.finish(),
        })
    }

    /// Preserve the established internal `PlugEntry` fixture without
    /// creating a second membership implementation. Synthetic validation
    /// evidence is rechecked against the current authority cut, while the
    /// fixture is deliberately denied RBF and capacity-eviction authority.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn plan_internal_plug(
        &mut self,
        receipt: DirectAdmissionReceipt,
    ) -> Result<InternalPlugDisposition<'_>, InternalPlugPlanError> {
        self.effects.lock().ensure_open().map_err(PlanError::from)?;
        let key = receipt.key().clone();
        if self.entries.contains_key(&key) {
            return Ok(InternalPlugDisposition::Duplicate);
        }
        self.validate_direct_acceptance_evidence(&receipt)?;

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
            .map_err(PlanError::from)?;
        let (version, arrival, clocks) = clocks.insertion().map_err(PlanError::from)?;
        let (accepted, async_process_start) =
            Self::direct_candidate(receipt, None, version, arrival);
        let prepared = self
            .prepare_non_displacing_internal_candidate(&key, &accepted)?
            .ok_or(InternalPlugPlanError::WouldDisplace)?;
        let delta = self.compile_membership_delta(MembershipCompilation {
            key,
            existing: None,
            accepted,
            prepared,
            clocks,
            effects: MembershipEffects::SilentInternal,
            async_process_start,
            resource: None,
            dependency: None,
            scheduler: None,
            owners: None,
            sparse_resource: false,
        })?;
        Ok(InternalPlugDisposition::Insert(PreparedApply::stage(
            self,
            DependencyAuthorityDelta::Membership(delta),
        )?))
    }

    /// Run the exact Local membership policy as a read-only TestAccept
    /// evaluation. Existing ownership is a duplicate regardless of phase;
    /// no owner, resource, clock, projection, or effect capability is
    /// acquired and no prepared mutation is constructed then discarded.
    pub(super) fn evaluate_direct_admission(
        &self,
        receipt: DirectAdmissionReceipt,
    ) -> Result<DirectAdmissionEvaluation, PlanError> {
        let key = receipt.key().clone();
        if self.entries.contains_key(&key) {
            return Ok(DirectAdmissionEvaluation::Duplicate(key));
        }
        self.validate_direct_acceptance_evidence(&receipt)?;
        if self
            .indexes
            .proposal_owner(&receipt.proof().payload().identity().proposal)
            .is_some()
        {
            return Err(PlanError::Backpressure(Backpressure::ProposalCollision));
        }
        let completed = receipt.completed();
        let clocks = self.clocks.snapshot();
        let (accepted, _async_process_start) =
            Self::direct_candidate(receipt, None, clocks.next_version, clocks.next_arrival);
        match self.evaluate_membership_candidate(&key, &accepted) {
            Ok(_evaluation) => Ok(DirectAdmissionEvaluation::Accepted(completed)),
            Err(PlanError::Membership(reason)) => Ok(DirectAdmissionEvaluation::Rejected(reason)),
            Err(error) => Err(error),
        }
    }

    fn direct_candidate(
        receipt: DirectAdmissionReceipt,
        existing: Option<&OwnedTx>,
        version: EntryVersion,
        arrival: Arrival,
    ) -> (AcceptedEntry, Option<AsyncProcessStart>) {
        let provenance = existing
            .and_then(OwnedTx::ingress_peer)
            .map_or(AcceptedProvenance::Trusted, |ingress| {
                AcceptedProvenance::Peer { ingress }
            });
        let (tx, proof, proposal, accepted_at, async_process_start) =
            receipt.into_membership_parts();
        (
            AcceptedEntry {
                record: TxRecord {
                    tx,
                    identity: proof.payload().identity().clone(),
                    version,
                    arrival,
                },
                provenance,
                proof,
                proposal,
                accepted_at,
            },
            async_process_start,
        )
    }

    /// Stage an owner-free ingress/resolve/verify rejection under a shared
    /// generation/chain guard. The returned terminal derives and holds only
    /// its exact Accepted producer/spender read support before activating the
    /// sole journal record.
    pub(super) fn plan_shared_direct_transaction_rejection(
        &self,
        rejection: DirectTransactionRejection,
    ) -> Result<PreparedSharedDirectRejectionTerminal<'_>, PlanError> {
        let (tx, command, reason, validity) = rejection.into_parts();
        if command != DirectCommand::Local {
            return Err(PlanError::Fault(AuthorityFault::EffectProjection));
        }
        self.plan_shared_direct_rejection_terminal(tx, reason, validity)
    }

    /// Stage a final direct-validation rejection under the same shared
    /// owner-free terminal. No membership or other authority owner exists on
    /// this path; the validator's Accepted overlay is its complete read fence.
    pub(super) fn plan_shared_direct_validation_rejection(
        &self,
        rejection: DirectAdmissionRejection,
    ) -> Result<PreparedSharedDirectRejectionTerminal<'_>, PlanError> {
        let (subject, reason) = rejection.into_parts();
        let (tx, validity) = subject.into_parts();
        self.plan_shared_direct_rejection_terminal(tx, reason, validity)
    }

    fn plan_shared_direct_rejection_terminal(
        &self,
        tx: Arc<ckb_types::core::TransactionView>,
        reason: CommittedPublicReject,
        validity: DirectRejectionValidity,
    ) -> Result<PreparedSharedDirectRejectionTerminal<'_>, PlanError> {
        let read_witness = DirectRejectionReadWitness::capture(self, validity)?;
        let publication = self
            .effects
            .lock()
            .build_single_publication(
                EffectPolicy::Trusted,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx,
                    audience: RejectionAudience::default(),
                    reason: reason.clone(),
                }),
            )
            .map_err(PlanError::from)?;
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;
        let staged_effect = EffectLog::stage_publication(&self.effects, effect)?;
        Ok(PreparedSharedDirectRejectionTerminal {
            authority: self,
            reason,
            read_witness,
            staged_effect,
            clocks: clocks.finish(),
        })
    }

    pub(super) fn evaluate_direct_validation_rejection(
        &self,
        rejection: DirectAdmissionRejection,
    ) -> Result<CommittedPublicReject, PlanError> {
        let (subject, reason) = rejection.into_parts();
        self.validate_direct_admission_subject(&subject)?;
        Ok(reason)
    }

    fn validate_direct_admission_subject(
        &self,
        subject: &super::chain::DirectAdmissionSubject,
    ) -> Result<(), PlanError> {
        self.validate_direct_rejection_validity(subject.validity())
    }

    pub(super) fn direct_rejection_is_current(
        &self,
        validity: &DirectRejectionValidity,
    ) -> Result<(), PlanError> {
        self.validate_direct_rejection_validity(validity)
    }

    fn validate_direct_rejection_validity(
        &self,
        validity: &DirectRejectionValidity,
    ) -> Result<(), PlanError> {
        match validity {
            DirectRejectionValidity::Stable => Ok(()),
            DirectRejectionValidity::AcceptedReads { view, reads } => {
                if view != &self.chain_view {
                    return Err(PlanError::Stale(StalePlan::ChainRevision));
                }
                if !reads.is_current(self) {
                    return Err(PlanError::Stale(StalePlan::AcceptedObservation));
                }
                Ok(())
            }
        }
    }

    fn validate_direct_acceptance_evidence(
        &self,
        receipt: &DirectAdmissionReceipt,
    ) -> Result<(), PlanError> {
        if receipt.view() != &self.chain_view {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }
        let proof = receipt.proof();
        if !self
            .dependencies
            .owner_free_proof_is_current(proof.payload().dependencies(), proof.dependency_cut())
        {
            return Err(PlanError::Stale(StalePlan::Dependency));
        }
        Ok(())
    }

    #[cfg(test)]
    fn prepare_accept_delta(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<MembershipDelta, PlanError> {
        self.prepare_accept_delta_with_resource_mode(receipt, false)
    }

    fn prepare_shared_accept_delta(
        &self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<MembershipDelta, PlanError> {
        self.prepare_accept_delta_with_resource_mode(receipt, true)
    }

    fn prepare_accept_delta_with_resource_mode(
        &self,
        receipt: FinalAdmissionReceipt,
        sparse_resource: bool,
    ) -> Result<MembershipDelta, PlanError> {
        let PreacceptedCandidateEvaluation {
            key,
            existing,
            accepted,
            async_process_start,
            outcome,
        } = self.evaluate_preaccepted_candidate(receipt, sparse_resource)?;
        let evaluation = match outcome {
            MembershipPolicyOutcome::Accepted(evaluation) => evaluation,
            MembershipPolicyOutcome::Rejected(rejection) => {
                let (reason, _witness) = rejection.into_parts();
                return Err(PlanError::Membership(reason));
            }
        };
        self.compile_preaccepted_accept_delta(
            key,
            existing,
            accepted,
            async_process_start,
            evaluation,
            sparse_resource,
        )
    }

    fn evaluate_preaccepted_candidate(
        &self,
        receipt: FinalAdmissionReceipt,
        sparse_resource: bool,
    ) -> Result<PreacceptedCandidateEvaluation, PlanError> {
        self.effects.lock().ensure_open()?;
        let key = receipt.key().clone();
        let expected = receipt.expected();
        let existing = self
            .entries
            .get(&key)
            .as_deref()
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if !matches!(&preaccepted.phase, PreAcceptedPhase::Ready(_)) {
            return Err(PlanError::Stale(StalePlan::Phase));
        }
        self.validate_acceptance_evidence(preaccepted, &receipt)?;
        let (proof, proposal, accepted_at, async_process_start) = receipt.into_membership_parts();
        let accepted = AcceptedEntry {
            record: preaccepted.record.clone(),
            provenance: preaccepted.source.accepted_provenance(),
            proof,
            proposal,
            accepted_at,
        };
        // Membership/RBF/capacity policy is a read-only decision and does not
        // depend on the fresh owner version. Reject every closed policy branch
        // before touching the shared clock bank; only a candidate which can
        // compile into a commit reserves its unique identity and Apply stamp.
        let outcome = self.evaluate_preaccepted_membership_policy(
            &key,
            preaccepted,
            &accepted,
            sparse_resource,
        )?;
        Ok(PreacceptedCandidateEvaluation {
            key,
            existing,
            accepted,
            async_process_start,
            outcome,
        })
    }

    fn compile_preaccepted_accept_delta(
        &self,
        key: RawTxHash,
        existing: OwnedTx,
        mut accepted: AcceptedEntry,
        async_process_start: Option<AsyncProcessStart>,
        evaluation: membership::MembershipEvaluation,
        sparse_resource: bool,
    ) -> Result<MembershipDelta, PlanError> {
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        #[cfg(test)]
        if sparse_resource {
            self.entries.enter_ready_clock_commit_probe();
        }
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let (version, clocks) = clocks.replacement()?;
        accepted.record.version = version;
        let prepared =
            self.prepare_membership_after_evaluation(&key, preaccepted, &accepted, evaluation)?;
        let effect_policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        self.compile_membership_delta(MembershipCompilation {
            key,
            existing: Some(existing),
            accepted,
            prepared,
            clocks,
            effects: MembershipEffects::Publish(effect_policy),
            async_process_start,
            resource: None,
            dependency: None,
            scheduler: None,
            owners: None,
            sparse_resource,
        })
    }

    fn compile_shared_candidate_disposition_delta(
        &self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<IndependentDelta, PlanError> {
        let PreacceptedCandidateEvaluation {
            key,
            existing,
            accepted,
            async_process_start,
            outcome,
        } = self.evaluate_preaccepted_candidate(receipt, true)?;
        match outcome {
            MembershipPolicyOutcome::Accepted(evaluation) => self
                .compile_preaccepted_accept_delta(
                    key,
                    existing,
                    accepted,
                    async_process_start,
                    evaluation,
                    true,
                )?
                .into_shared_exact(),
            MembershipPolicyOutcome::Rejected(rejection) => {
                let (reason, witness) = rejection.into_parts();
                self.compile_shared_preaccepted_rejection_delta(
                    key, existing, accepted, reason, witness,
                )
            }
        }
    }

    fn compile_shared_preaccepted_rejection_delta(
        &self,
        key: RawTxHash,
        existing: OwnedTx,
        accepted: AcceptedEntry,
        reason: MembershipReject,
        witness: MembershipPolicyWitness,
    ) -> Result<IndependentDelta, PlanError> {
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        let publication = self
            .effects
            .lock()
            .build_single_publication(
                EffectPolicy::for_preaccepted_source(preaccepted.source),
                CommittedEffect::Rejected(CommittedRejection::Membership {
                    tx: Arc::clone(&accepted.record.tx),
                    audience: RejectionAudience::from_source(preaccepted.source),
                    reason,
                }),
            )
            .map_err(PlanError::from)?;
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let dependency_control = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;
        let delta = self.compile_entry_delta_with_controls(
            EntryTransition::Remove {
                key,
                before: existing,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency_and_effect(dependency_control, effect),
            None,
        )?;
        delta.into_shared_terminalization(self, None, witness)
    }

    fn compile_membership_delta(
        &self,
        compilation: MembershipCompilation,
    ) -> Result<MembershipDelta, PlanError> {
        let MembershipCompilation {
            key,
            existing,
            accepted,
            prepared,
            mut clocks,
            effects,
            async_process_start,
            resource: prepared_resource,
            dependency: prepared_dependency,
            scheduler: prepared_scheduler,
            owners: prepared_owners,
            sparse_resource,
        } = compilation;
        let changed_expected = existing
            .as_ref()
            .map_or(OwnerPrestate::Vacant, OwnerPrestate::from_owner);
        if existing.is_none() {
            self.reserve_primary_owner_insertions(std::iter::once(&key))?;
        }
        let PreparedMembership {
            mut removals,
            projection,
        } = prepared;
        let sequence = clocks.sequence();
        let mut retained_history =
            self.retain_replacement_history(&accepted, &mut removals, sequence)?;
        if !retained_history {
            removals.iter_mut().for_each(MembershipRemoval::terminalize);
        }

        let effect = match effects {
            MembershipEffects::Publish(policy) => {
                self.plan_admission_effects(&accepted, &removals, &projection, sequence, policy)?
            }
            #[cfg(any(test, feature = "internal"))]
            MembershipEffects::SilentInternal => EffectDelta::default(),
        };
        let after = OwnedTx::Accepted(accepted);
        let resource = match prepared_resource {
            Some(resource) if existing.is_none() && removals.is_empty() => resource,
            Some(_) => {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
            None => {
                let plan_resources = |removals: &[MembershipRemoval]| {
                    if sparse_resource {
                        self.plan_sparse_membership_resources(
                            &key,
                            existing.as_ref(),
                            &after,
                            removals,
                        )
                    } else {
                        self.plan_membership_resources(&key, existing.as_ref(), &after, removals)
                    }
                };
                match plan_resources(&removals) {
                    Ok(resource) => resource,
                    Err(
                        ResourceError::PreAcceptedLimit | ResourceError::ReplacementHistoryLimit,
                    ) => {
                        removals.iter_mut().for_each(MembershipRemoval::terminalize);
                        retained_history = false;
                        match plan_resources(&removals) {
                            Ok(resource) => resource,
                            Err(ResourceError::AcceptedLimit) if sparse_resource => {
                                return Err(PlanError::Stale(StalePlan::AcceptedObservation));
                            }
                            Err(error) => return Err(Self::membership_resource_error(error)),
                        }
                    }
                    Err(ResourceError::AcceptedLimit) if sparse_resource => {
                        return Err(PlanError::Stale(StalePlan::AcceptedObservation));
                    }
                    Err(error) => return Err(Self::membership_resource_error(error)),
                }
            }
        };
        if retained_history {
            let mut history_clocks = clocks.owner_branch();
            for removal in removals
                .iter_mut()
                .filter(|removal| removal.after().is_some())
            {
                let (version, arrival, next) = history_clocks.insertion()?;
                history_clocks = next;
                removal.assign_replacement_history_identity(version, arrival)?;
            }
            history_clocks.adopt();
        }
        let retirement_capacity = removals
            .len()
            .checked_add(usize::from(existing.is_some()))
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let retired = retired_buffer(retirement_capacity)?;
        let scheduler = match prepared_scheduler {
            Some(scheduler) if existing.is_none() && removals.is_empty() => scheduler,
            Some(_) => {
                return Err(PlanError::Fault(AuthorityFault::SchedulerProjection));
            }
            None => self
                .scheduler
                .lock()
                .plan_replace(existing.as_ref(), Some(&after), None)?,
        };
        let dependency = match prepared_dependency {
            Some(dependency) if existing.is_none() && removals.is_empty() => dependency,
            Some(_) => {
                return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
            }
            None => self.plan_membership_dependency_delta(
                existing.as_ref(),
                &after,
                &removals,
                sequence,
            )?,
        };
        let owners = match prepared_owners {
            Some(owners) if existing.is_none() && removals.is_empty() => owners,
            Some(_) => {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
            None => self.plan_membership_owner_derivations(
                &key,
                existing.as_ref(),
                &after,
                &removals,
                sequence,
            )?,
        };
        Ok(MembershipDelta {
            changed_key: key,
            changed_expected,
            changed_after: after,
            retired,
            removals,
            owners,
            resource,
            projection,
            scheduler,
            dependency,
            effect,
            clocks: clocks.finish(),
            async_process_start,
        })
    }

    fn plan_admission_effects(
        &self,
        accepted: &AcceptedEntry,
        removals: &[MembershipRemoval],
        projection: &ProjectionDelta,
        sequence: ApplySequence,
        policy: EffectPolicy,
    ) -> Result<EffectDelta, PlanError> {
        let effect_count = removals
            .len()
            .checked_add(1)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut effects = Vec::new();
        effects
            .try_reserve(effect_count)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        self.append_admission_effects(&mut effects, accepted, removals, projection)?;
        let publication = self
            .effects
            .lock()
            .build_publication(policy, effects)
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.effects_for_plan()
            .plan_publication(&publication, sequence)
            .map_err(PlanError::from)
    }

    /// Append one complete candidate terminal sequence to already reserved
    /// batch scratch. Both the single-candidate compiler and the disjoint
    /// leaf-RBF cohort therefore publish exactly `Accepted` followed by that
    /// candidate's victim outcomes from this one effect policy implementation.
    fn append_admission_effects(
        &self,
        effects: &mut Vec<CommittedEffect>,
        accepted: &AcceptedEntry,
        removals: &[MembershipRemoval],
        projection: &ProjectionDelta,
    ) -> Result<(), PlanError> {
        effects.push(CommittedEffect::Accepted(CommittedAcceptance::Admission {
            entry: self.committed_entry_after(accepted, projection)?,
            status: accepted.status(),
            ingress_peer: accepted.provenance.ingress_peer(),
        }));
        for removal in removals {
            let owner = self
                .entries
                .get(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let OwnedTx::Accepted(removed) = &*owner else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let entry = self.committed_entry_before(removed)?;
            let rejection = match removal.cause {
                RemovalCause::Replacement => CommittedRejection::Replaced {
                    entry,
                    winner: accepted.record.identity.raw.clone(),
                },
                RemovalCause::Capacity => {
                    let cost = removed.proof.metrics().cost;
                    CommittedRejection::CapacityEvicted {
                        entry,
                        fee_rate: ckb_types::core::FeeRate::calculate(
                            removed.proof.metrics().fee,
                            get_transaction_weight(cost.serialized_bytes, cost.cycles),
                        ),
                    }
                }
            };
            effects.push(CommittedEffect::Rejected(rejection));
        }
        Ok(())
    }

    fn committed_entry_before(
        &self,
        entry: &AcceptedEntry,
    ) -> Result<CommittedEntrySnapshot, PlanError> {
        let hash = &entry.record.identity.raw;
        let ancestors = self
            .membership
            .ancestor_aggregate(hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let descendants = self
            .membership
            .descendant_aggregate(hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        Ok(Self::committed_entry_snapshot(
            entry,
            ancestors,
            descendants,
        ))
    }

    fn committed_entry_after(
        &self,
        entry: &AcceptedEntry,
        projection: &ProjectionDelta,
    ) -> Result<CommittedEntrySnapshot, PlanError> {
        let hash = &entry.record.identity.raw;
        let ancestors = projection
            .ancestor_after(&self.membership, hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let descendants = projection
            .descendant_after(&self.membership, hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        Ok(Self::committed_entry_snapshot(
            entry,
            ancestors,
            descendants,
        ))
    }

    fn committed_entry_snapshot(
        entry: &AcceptedEntry,
        ancestors: AncestorAggregate,
        descendants: DescendantAggregate,
    ) -> CommittedEntrySnapshot {
        let metrics = entry.proof.metrics();
        CommittedEntrySnapshot {
            tx: Arc::clone(&entry.record.tx),
            cycles: metrics.cost.cycles,
            size: metrics.cost.serialized_bytes,
            fee: metrics.fee,
            ancestors_size: ancestors.serialized_bytes,
            ancestors_fee: ancestors.fee,
            ancestors_cycles: ancestors.cycles,
            ancestors_count: ancestors.entries,
            descendants_fee: descendants.fee,
            descendants_size: descendants.serialized_bytes,
            descendants_cycles: descendants.cycles,
            descendants_count: descendants.entries,
            timestamp: entry.accepted_at.0,
        }
    }

    #[cfg(test)]
    fn plan_preaccepted_terminalization(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .as_deref()
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(_) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        self.effects.lock().preflight_publication(publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let dependency_control = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self
            .effects_for_plan()
            .plan_publication(publication, sequence)?;
        self.prepare_entry_delta_with_controls(
            EntryTransition::Remove {
                key: key.clone(),
                before: existing,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency_and_effect(dependency_control, effect),
            None,
        )
    }

    /// Clear only executable and retained pre-pool ownership. Accepted
    /// membership and the paired chain view remain authoritative; generation
    /// advance invalidates every old Recovery capability, while exact
    /// version/lease checks make late compute settlement ordinary stale work.
    pub(super) fn plan_clear_pipeline(&mut self) -> Result<PreparedApply<'_>, PlanError> {
        let mut hashes = Vec::new();
        hashes
            .try_reserve(self.entries.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        let owners = self.entries.read_all();
        hashes.extend(
            owners
                .iter()
                .filter(|(_, owner)| {
                    matches!(
                        owner,
                        OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)
                    )
                })
                .map(|(hash, _)| hash.clone()),
        );
        drop(owners);
        hashes.sort_unstable();
        let generation = next_generation(self.generation)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self.effects.lock().plan_generation_reset(sequence)?;
        let removal = self.plan_owner_removal_batch(OwnerRemovalKeys::new(hashes)?, sequence)?;
        PreparedApply::stage(
            self,
            DependencyAuthorityDelta::ClearPipeline(ClearPipelineDelta {
                generation,
                removal,
                effect,
                clocks: clocks.finish(),
            }),
        )
    }

    /// Replace the complete pool generation and install exactly the validated
    /// next chain view. Apply swaps exactly 64 generation payload headers while
    /// persistent routed fence envelopes remain in place; active work
    /// capabilities are invalidated by missing ownership, not by a drain
    /// protocol.
    pub(super) fn plan_clear_pool(
        &mut self,
        tip_hash: ckb_types::packed::Byte32,
    ) -> Result<PreparedApply<'_>, GenerationReplacementPlanError> {
        let chain_revision = self
            .chain_revision()
            .0
            .checked_add(1)
            .map(ChainRevision)
            .ok_or(GenerationReplacementPlanError::Fault(
                AuthorityFault::CounterExhausted,
            ))?;
        let chain_view = ChainViewId::new(chain_revision, tip_hash);
        let generation = self.generation.0.checked_add(1).map(PoolGeneration).ok_or(
            GenerationReplacementPlanError::Fault(AuthorityFault::CounterExhausted),
        )?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
            .map_err(|_| GenerationReplacementPlanError::Fault(AuthorityFault::CounterExhausted))?;
        let sequence = clocks.sequence();
        let effect = self
            .effects
            .lock()
            .plan_generation_reset(sequence)
            .map_err(|error| match error {
                GenerationResetPlanError::Closed => GenerationReplacementPlanError::LifecycleClosed,
                GenerationResetPlanError::SequenceOvertaken => {
                    GenerationReplacementPlanError::Fault(AuthorityFault::EffectProjection)
                }
            })?;
        let sources = self.source_versions.plan_generation_replacement(sequence);
        let fresh = FreshGeneration::empty(&self.resources, &self.scheduler, &self.entries);
        let compute_slot_released = self.resources.read(&self.entries).preaccepted().active_work
            > fresh.preaccepted_active_work();
        Ok(PreparedApply::plain(
            self,
            PlainAuthorityDelta::ClearPool(Box::new(ClearPoolDelta {
                generation,
                chain_view,
                fresh,
                sources,
                effect,
                clocks: clocks.finish(),
                compute_slot_released,
            })),
        ))
    }

    /// Compile the exclusive peer-fence transition. Public local removal does
    /// not enter this writer-only route: it uses the canonical shared owner
    /// removal compiler and its exact administrative-closure evidence.
    #[cfg(test)]
    fn compile_administrative_removal(
        &mut self,
        hashes: Vec<RawTxHash>,
        marker: PeerBanDelta,
        revocation: CommittedPeerCohortRevocation,
    ) -> Result<AdminDelta, PlanError> {
        self.effects.lock().ensure_open()?;
        let hashes = OwnerRemovalKeys::new(hashes)?;
        for hash in hashes.iter() {
            let owner = self
                .entries
                .get(hash)
                .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?;
            let OwnedTx::PreAccepted(entry) = &*owner else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            if entry.source.ingress_peer() != Some(revocation.peer()) {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
        }

        let mut effects = Vec::new();
        effects
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        effects.push(CommittedEffect::PeerCohortRevoked(revocation));
        let publication = self
            .effects
            .lock()
            .build_publication(EffectPolicy::CriticalDetail, effects)
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;

        let removal = self.plan_owner_removal_batch(hashes, sequence)?;
        Ok(AdminDelta {
            marker,
            removal,
            effect,
            clocks: clocks.finish(),
        })
    }

    fn plan_owner_removal_batch(
        &self,
        hashes: OwnerRemovalKeys,
        sequence: ApplySequence,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        let mut accepted_removals = Vec::new();
        accepted_removals
            .try_reserve_exact(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        accepted_removals.extend(
            hashes
                .iter()
                .filter(|hash| {
                    matches!(
                        self.entries.get(hash).as_deref(),
                        Some(OwnedTx::Accepted(_))
                    )
                })
                .cloned(),
        );
        let accepted_removals = AcceptedRemovalSet::try_from_vec(accepted_removals)?;
        let available = self.collect_released_administrative_inputs(&accepted_removals)?;
        self.compile_owner_removal_batch(hashes, accepted_removals, available, sequence)
    }

    /// A peer cohort index contains only not-yet-Accepted owners. Proving that
    /// bounded fact per indexed member makes the generic Accepted descendant
    /// input-release population scan both unnecessary and incorrect for the
    /// true-shard revocation path; the canonical removal compiler remains the
    /// sole producer of every derived delta.
    fn plan_preaccepted_peer_cohort_removal_batch(
        &self,
        hashes: OwnerRemovalKeys,
        peer: ckb_network::PeerIndex,
        sequence: ApplySequence,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        for hash in hashes.iter() {
            let owner = self
                .entries
                .get(hash)
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            let OwnedTx::PreAccepted(entry) = &*owner else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            if entry.source.ingress_peer() != Some(peer) {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
        }
        self.compile_owner_removal_batch(
            hashes,
            AcceptedRemovalSet::try_from_vec(Vec::new())?,
            Vec::new(),
            sequence,
        )
    }

    fn compile_owner_removal_batch(
        &self,
        hashes: OwnerRemovalKeys,
        accepted_removals: AcceptedRemovalSet,
        available: Vec<DependencyKey>,
        sequence: ApplySequence,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        if hashes.iter().any(|hash| !self.entries.contains_key(hash)) {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        let mut owner_snapshots = Vec::new();
        owner_snapshots
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in hashes.iter() {
            owner_snapshots.push(
                self.entries
                    .get(hash)
                    .as_deref()
                    .cloned()
                    .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?,
            );
        }
        self.compile_owner_removal_batch_from_snapshots(
            hashes,
            accepted_removals,
            available,
            sequence,
            OwnerRemovalSnapshots {
                owners: owner_snapshots,
                closure: None,
            },
        )
    }

    /// Reclassify a live-row contradiction observed after an exact owner
    /// cohort was captured. A missing, phase-changed, or version-advanced
    /// owner is committed external progress and must stale this compilation;
    /// a contradiction while every incarnation remains intact is a genuine
    /// projection fault. This is bounded by the removal cohort and adds no
    /// hot-path state or retry loop.
    fn removal_cohort_observation_error(
        &self,
        owner_snapshots: &[OwnedTx],
        fault: AuthorityFault,
    ) -> PlanError {
        let diverged = owner_snapshots.iter().any(|snapshot| {
            let Some(live) = self.entries.get(&snapshot.record().identity.raw) else {
                return true;
            };
            let same_phase = matches!(
                (snapshot, &*live),
                (OwnedTx::PreAccepted(_), OwnedTx::PreAccepted(_))
                    | (OwnedTx::Accepted(_), OwnedTx::Accepted(_))
                    | (
                        OwnedTx::ReplacementHistory(_),
                        OwnedTx::ReplacementHistory(_)
                    )
            );
            !same_phase || live.record().version != snapshot.record().version
        });
        if diverged {
            PlanError::Stale(StalePlan::AcceptedObservation)
        } else {
            PlanError::Fault(fault)
        }
    }

    fn compile_owner_removal_batch_from_snapshots(
        &self,
        hashes: OwnerRemovalKeys,
        accepted_removals: AcceptedRemovalSet,
        mut available: Vec<DependencyKey>,
        sequence: ApplySequence,
        snapshots: OwnerRemovalSnapshots,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        let OwnerRemovalSnapshots {
            owners: owner_snapshots,
            closure,
        } = snapshots;
        if hashes.len() != owner_snapshots.len()
            || hashes
                .iter()
                .zip(&owner_snapshots)
                .any(|(hash, owner)| &owner.record().identity.raw != hash)
        {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        let membership = self
            .prepare_chain_projection(&accepted_removals, &HashMap::new())
            .map_err(|error| match error {
                PlanError::Fault(fault @ AuthorityFault::MembershipProjection) => {
                    self.removal_cohort_observation_error(&owner_snapshots, fault)
                }
                other => other,
            })?;
        if closure
            .as_ref()
            .is_some_and(|closure| !closure.matches_projection(&accepted_removals, &membership))
        {
            return Err(PlanError::Stale(StalePlan::AcceptedObservation));
        }

        let (_entries, resources_ledger, dependencies_frontier, source_versions, indexes) =
            self.concurrent_owner_removal_plan_parts();
        let mut dependency_error = None;
        available.retain(
            |key| match dependencies_frontier.has_waiter_outside(key, &hashes) {
                Ok(retain) => retain,
                Err(error) => {
                    dependency_error = Some(error);
                    false
                }
            },
        );
        if let Some(error) = dependency_error {
            return Err(error.into());
        }
        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        resource_changes.extend(
            hashes
                .iter()
                .zip(&owner_snapshots)
                .map(|(hash, owner)| (hash.clone(), owner.charge_record())),
        );
        let resources = resources_ledger
            .plan_removal_batch(resource_changes)
            .map_err(|error| match error {
                ResourceError::ExistingChargeMismatch => self.removal_cohort_observation_error(
                    &owner_snapshots,
                    AuthorityFault::ResourceProjection,
                ),
                other => PlanError::from(other),
            })?;
        let scheduler = self
            .scheduler
            .lock()
            .plan_batch(owner_snapshots.iter().map(|owner| (Some(owner), None)))?;
        let lost =
            Self::collect_dependency_loss_keys_from(dependencies_frontier, owner_snapshots.iter())?
                .keys;
        let dependency_control = dependencies_frontier
            .plan_events(available, lost, DependencyCut(sequence))?
            .unwrap_or_default();
        let dependency = dependencies_frontier
            .plan_replacements(owner_snapshots.iter().map(|owner| (Some(owner), None)))?
            .with_control(dependency_control.into(), dependencies_frontier)?;
        let sources = source_versions.plan_replacements(
            owner_snapshots.iter().map(|owner| (Some(owner), None)),
            sequence,
        );
        let template_sources = self.plan_owner_sources(
            hashes
                .iter()
                .zip(&owner_snapshots)
                .map(|(hash, owner)| (hash, Some(owner), None)),
        )?;
        let indexes = indexes.plan_replacements(
            hashes
                .iter()
                .zip(&owner_snapshots)
                .map(|(hash, owner)| (hash, Some(owner), None)),
        )?;
        let owners = DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        };
        let retired = retired_buffer(hashes.len())?;
        let expected_versions = owner_snapshots
            .iter()
            .map(|owner| owner.record().version)
            .collect();
        Ok(OwnerRemovalBatch {
            hashes: hashes.into_inner(),
            expected_versions,
            owners,
            resources,
            membership,
            scheduler,
            dependency,
            retired,
        })
    }

    /// Remove the complete bounded pre-accepted cohort owned by one banned
    /// ingress peer. Accepted membership is deliberately absent from the peer
    /// index: a commit that applies first remains Accepted, while a ban that
    /// applies first removes active work and makes its later lease settlement
    /// stale under the same authority.
    #[cfg(test)]
    fn plan_peer_revocation(
        &mut self,
        peer: ckb_network::PeerIndex,
        tx_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.compile_peer_revocation(peer, tx_hash, reason)?;
        PreparedApply::stage(self, DependencyAuthorityDelta::Admin(delta))
    }

    #[cfg(test)]
    fn compile_peer_revocation(
        &mut self,
        peer: ckb_network::PeerIndex,
        tx_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Result<AdminDelta, PlanError> {
        let mut hashes = Vec::new();
        if let Some(indexed) = self.indexes.preaccepted_for_peer(peer) {
            hashes
                .try_reserve(indexed.len())
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            hashes.extend(indexed.iter().cloned());
        }
        hashes.sort_unstable();
        let marker = self
            .peer_bans_for_plan()
            .plan_record(peer, Instant::now())?;
        self.entries
            .reserve_exclusive_peer_fence(marker)
            .map_err(|error| match error {
                crate::authority::shard::PeerFenceStageError::Allocation => {
                    PlanError::Backpressure(Backpressure::Allocation)
                }
                crate::authority::shard::PeerFenceStageError::Stale => {
                    PlanError::Stale(StalePlan::Version)
                }
            })?;
        let revocation = CommittedPeerCohortRevocation::malformed(marker.lease(), tx_hash, reason)
            .ok_or(PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.compile_administrative_removal(hashes, marker, revocation)
    }

    fn capture_accepted_administrative_removal(
        &self,
        root: &RawTxHash,
    ) -> Result<AcceptedAdministrativeRemoval, PlanError> {
        let closure = self.administrative_descendant_closure_witness(root)?;
        let (hashes, closure_witness) = closure.into_parts();

        let mut removal_set = Vec::new();
        removal_set
            .try_reserve_exact(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        removal_set.extend(hashes.iter().cloned());
        let accepted_removals = AcceptedRemovalSet::try_from_vec(removal_set)?;

        let mut owners = Vec::new();
        owners
            .try_reserve_exact(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in &hashes {
            let owner = self
                .entries
                .get(hash)
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            let OwnedTx::Accepted(entry) = &*owner else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            owners.push(OwnedTx::Accepted(entry.clone()));
        }

        let AdministrativeReleasedInputs { keys, parents } = {
            let mut accepted = Vec::new();
            accepted
                .try_reserve_exact(owners.len())
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            for (hash, owner) in hashes.iter().zip(&owners) {
                let OwnedTx::Accepted(entry) = owner else {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                };
                accepted.push((hash, entry));
            }
            self.collect_released_administrative_inputs_from(accepted, &accepted_removals)
                .map_err(|error| match error {
                    PlanError::Fault(fault @ AuthorityFault::MembershipProjection) => {
                        self.removal_cohort_observation_error(&owners, fault)
                    }
                    other => other,
                })?
        };

        Ok(AcceptedAdministrativeRemoval {
            hashes: OwnerRemovalKeys::new(hashes)?,
            accepted_removals,
            released: keys,
            snapshots: OwnerRemovalSnapshots {
                owners,
                closure: Some(closure_witness),
            },
            control: AdministrativeRemovalControl { parents },
        })
    }

    /// Compile one explicit public removal against an exact shared cut.
    /// Accepted roots use the same complete administrative closure as expiry;
    /// transient/history roots use a singleton snapshot. No cause-specific
    /// owner, resource, scheduler, dependency or source writer exists here.
    /// `None` is the exact absent-owner linearization; a later lost cut is a
    /// typed stale result at the runtime boundary, never another absence.
    pub(super) fn compile_shared_local_removal(
        &self,
        root: &RawTxHash,
    ) -> Result<Option<CompiledSharedLocalRemoval>, PlanError> {
        let Some(root_owner) = self.entries.get(root).as_deref().cloned() else {
            return Ok(None);
        };
        self.effects.lock().ensure_open()?;

        let (hashes, accepted_removals, released, snapshots, control) = match &root_owner {
            OwnedTx::Accepted(_) => {
                let captured = self.capture_accepted_administrative_removal(root)?;
                (
                    captured.hashes,
                    captured.accepted_removals,
                    captured.released,
                    captured.snapshots,
                    captured.control,
                )
            }
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {
                let mut hashes = Vec::new();
                let mut owners = Vec::new();
                hashes
                    .try_reserve_exact(1)
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                owners
                    .try_reserve_exact(1)
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                hashes.push(root.clone());
                owners.push(root_owner.clone());
                (
                    OwnerRemovalKeys::new(hashes)?,
                    AcceptedRemovalSet::try_from_vec(Vec::new())?,
                    Vec::new(),
                    OwnerRemovalSnapshots {
                        owners,
                        closure: None,
                    },
                    AdministrativeRemovalControl {
                        parents: Vec::new(),
                    },
                )
            }
        };

        let publication = CommittedRemoteIngressRelease::removed_owner(root.clone(), &root_owner)
            .map(|release| {
                self.effects
                    .lock()
                    .build_single_publication(
                        EffectPolicy::Trusted,
                        CommittedEffect::RemoteIngressReleased(release),
                    )
                    .map_err(PlanError::from)
            })
            .transpose()?;
        if let Some(publication) = publication.as_ref() {
            self.effects.lock().preflight_publication(publication)?;
        }

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let removal = self.compile_owner_removal_batch_from_snapshots(
            hashes,
            accepted_removals,
            released,
            sequence,
            snapshots,
        )?;
        Ok(Some(CompiledSharedLocalRemoval {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal: CompiledSharedOwnerRemoval {
                removal,
                publication,
                sequence,
                clocks: clocks.finish(),
            },
            control,
        }))
    }

    /// Compile one Accepted expiry using the same cause-neutral closure and
    /// canonical owner-removal compiler as public local removal.
    pub(super) fn compile_shared_accepted_expiry(
        &self,
        cutoff: AcceptedAtMillis,
    ) -> Result<Option<CompiledSharedAcceptedExpiry>, PlanError> {
        let Some(head) = self.indexes.accepted_expiry_head(cutoff)? else {
            return Ok(None);
        };
        let captured = self.capture_accepted_administrative_removal(&head.due().hash)?;
        #[cfg(test)]
        self.entries.enter_accepted_expiry_mid_compile_pause();
        let root_owner = captured
            .snapshots
            .owners
            .iter()
            .find(|owner| owner.record().identity.raw == head.due().hash)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let OwnedTx::Accepted(root_entry) = root_owner else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        if root_entry.record.version != head.version()
            || root_entry.accepted_at != head.due().accepted_at
            || root_entry.accepted_at > cutoff
        {
            return Err(PlanError::Stale(StalePlan::Version));
        }

        let mut effects = Vec::new();
        effects
            .try_reserve_exact(captured.snapshots.owners.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for owner in &captured.snapshots.owners {
            let OwnedTx::Accepted(entry) = owner else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let before = self
                .committed_entry_before(entry)
                .map_err(|error| match error {
                    PlanError::Fault(fault @ AuthorityFault::MembershipProjection) => {
                        self.removal_cohort_observation_error(&captured.snapshots.owners, fault)
                    }
                    other => other,
                })?;
            effects.push(CommittedEffect::Rejected(CommittedRejection::Expired {
                entry: before,
            }));
        }
        let publication = self
            .effects
            .lock()
            .build_publication(EffectPolicy::CriticalDetail, effects)
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.effects.lock().preflight_publication(&publication)?;

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let AcceptedAdministrativeRemoval {
            hashes,
            accepted_removals,
            released,
            snapshots,
            control,
        } = captured;
        let removal = self.compile_owner_removal_batch_from_snapshots(
            hashes,
            accepted_removals,
            released,
            sequence,
            snapshots,
        )?;
        Ok(Some(CompiledSharedAcceptedExpiry {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal: CompiledSharedOwnerRemoval {
                removal,
                publication: Some(publication),
                sequence,
                clocks: clocks.finish(),
            },
            control: AcceptedExpiryControl {
                head,
                administrative: control,
            },
        }))
    }

    /// Compile up to `limit` Remote owners whose retained residency lease has
    /// elapsed. The fixed-layout index captures the exact sorted prefix plus
    /// its next head in O(64 + limit) work. This phase holds no live scheduler
    /// gate; Bind stages every fallible projection, and Apply revalidates the
    /// already-allocated witness under one sparse-write/full-read mixed cut.
    pub(super) fn plan_remote_expiry(
        &self,
        cutoff: RemoteDeadline,
        limit: NonZeroUsize,
    ) -> Result<Option<CompiledSharedRemoteExpiry>, PlanError> {
        let mut witness = self.indexes.remote_expiry_witness(cutoff, limit.get())?;
        if witness.is_empty() {
            return Ok(None);
        }

        let mut effects = Vec::new();
        effects
            .try_reserve_exact(witness.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        effects.extend(
            witness
                .members()
                .map(|(due, _)| CommittedEffect::RemoteExpired {
                    tx_hash: due.hash.clone(),
                }),
        );
        let prefix = self
            .effects
            .lock()
            .build_remote_prefix(effects)
            .map_err(|error| match error {
                EffectBuildError::Empty
                | EffectBuildError::TooMany
                | EffectBuildError::TooLarge
                | EffectBuildError::Arithmetic
                | EffectBuildError::ReservedReset => {
                    PlanError::Fault(AuthorityFault::EffectProjection)
                }
            })?
            .ok_or(PlanError::Fault(AuthorityFault::EffectProjection))?;
        let (publication, selected) = prefix.into_parts();
        witness.truncate(selected.get())?;
        self.effects.lock().preflight_publication(&publication)?;

        let mut hashes = Vec::new();
        hashes
            .try_reserve_exact(witness.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(witness.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for (candidate, expected_version) in witness.members() {
            let owner = self
                .entries
                .get(&candidate.hash)
                .ok_or(PlanError::Stale(StalePlan::Missing))?;
            let OwnedTx::PreAccepted(entry) = &*owner else {
                return Err(PlanError::Stale(StalePlan::Phase));
            };
            if entry.record.version != expected_version
                || entry.source.active_remote_deadline() != Some(candidate.expires_at)
            {
                return Err(PlanError::Stale(StalePlan::Version));
            }
            hashes.push(candidate.hash.clone());
            owners.push(OwnedTx::PreAccepted(entry.clone()));
        }
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let removal = self.compile_owner_removal_batch_from_snapshots(
            OwnerRemovalKeys::new(hashes)?,
            AcceptedRemovalSet::try_from_vec(Vec::new())?,
            Vec::new(),
            sequence,
            OwnerRemovalSnapshots {
                owners,
                closure: None,
            },
        )?;
        Ok(Some(CompiledSharedRemoteExpiry {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal: CompiledSharedOwnerRemoval {
                removal,
                publication: Some(publication),
                sequence,
                clocks: clocks.finish(),
            },
            witness,
        }))
    }

    pub(super) fn effect_publication_observation(&self) -> EffectPublicationObservation {
        self.effects.lock().publication_observation()
    }

    pub(super) fn apply_effect_settlement(
        &self,
        settlement: EffectSettlement,
    ) -> Result<(EffectSettlementCommit, EffectPublicationObservation), EffectSettlementFailure>
    {
        let mut effects = self.effects.lock();
        let plan = match effects.plan_settlement(&settlement) {
            Ok(plan) => plan,
            Err(error) => {
                return Err(EffectSettlementFailure {
                    error: match error {
                        EffectSettlementPlanError::StaleLease => EffectSettlementError::StaleLease,
                        EffectSettlementPlanError::Projection => EffectSettlementError::Projection,
                    },
                    settlement,
                });
            }
        };
        let EffectSettlementPlan::Apply(effect) = plan else {
            let next = effects.publication_observation();
            return Ok((EffectSettlementCommit::Superseded(settlement), next));
        };
        let clocks = match ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks)) {
            Ok(clocks) => clocks,
            Err(_) => {
                return Err(EffectSettlementFailure {
                    error: EffectSettlementError::CounterExhausted,
                    settlement,
                });
            }
        };
        let before = Self::wake_projection_for_effect_settlement(effects.wake_projection());
        let retired_effect = match effects.apply_settlement(effect) {
            Ok(retired) => retired,
            Err(error) => {
                return Err(EffectSettlementFailure {
                    error: match error {
                        EffectSettlementPlanError::StaleLease => EffectSettlementError::StaleLease,
                        EffectSettlementPlanError::Projection => EffectSettlementError::Projection,
                    },
                    settlement,
                });
            }
        };
        let after = Self::wake_projection_for_effect_settlement(effects.wake_projection());
        let next = effects.publication_observation();
        drop(effects);
        let _reserved_clock_high_water = clocks.finish();
        let committed = finish_apply_between(
            self,
            before,
            after,
            false,
            false,
            ApplyRetirement {
                async_process_observations: AsyncProcessObservations::None,
                removals: Vec::new(),
                retired: RetiredOwners::default(),
                retired_effect,
                retired_generation: None,
                dependency: None,
                template_source_changed: false,
            },
        );
        Ok((EffectSettlementCommit::Applied(committed), next))
    }

    pub(super) fn plan_effect_close(&mut self) -> Result<PreparedApply<'_>, EffectCloseError> {
        if self.resources.read(&self.entries).preaccepted().active_work != 0 {
            return Err(EffectCloseError::ActiveWork);
        }
        let effect = self
            .effects
            .lock()
            .plan_close()
            .map_err(|error| match error {
                EffectClosePlanError::AlreadyClosed => EffectCloseError::AlreadyClosed,
            })?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
            .map_err(|_| EffectCloseError::CounterExhausted)?;
        Ok(self.prepared_effect_only(effect, clocks))
    }

    pub(super) fn effects_closed_and_drained(&self) -> bool {
        self.effects.lock().is_closed_and_drained()
    }

    pub(super) fn pending_recent_reject(&self, hash: &RawTxHash) -> Option<PendingRecentReject> {
        self.effects.lock().pending_recent_reject(hash)
    }

    fn prepared_effect_only(
        &mut self,
        effect: EffectDelta,
        clocks: ApplyClockReservation,
    ) -> PreparedApply<'_> {
        PreparedApply::plain(
            self,
            PlainAuthorityDelta::Effect(EffectOnlyDelta {
                effect,
                clocks: clocks.finish(),
            }),
        )
    }

    fn classify_dependency_maintenance_plan_error(
        &self,
        ticket: &super::dependency::DependencyMaintenanceTicket,
        expected_owner_version: Option<EntryVersion>,
        error: PlanError,
    ) -> PlanError {
        let projection_fault = matches!(
            error,
            PlanError::Fault(
                AuthorityFault::ResourceProjection
                    | AuthorityFault::MembershipProjection
                    | AuthorityFault::IndexProjection
                    | AuthorityFault::SchedulerProjection
                    | AuthorityFault::DependencyProjection
            )
        );
        if !projection_fault {
            return error;
        }
        let owner_is_current = match (ticket.hash(), expected_owner_version) {
            (None, None) => true,
            (Some(hash), Some(version)) => self
                .entries
                .get(hash)
                .is_some_and(|owner| owner.record().version == version),
            (Some(hash), None) => self.entries.get(hash).is_none(),
            (None, Some(_)) => false,
        };
        if owner_is_current && self.dependencies.maintenance_ticket_is_current(ticket) {
            error
        } else {
            PlanError::Stale(StalePlan::Dependency)
        }
    }

    pub(super) fn plan_dependency_maintenance(
        &self,
    ) -> Result<Option<PreparedIndependentApply<'_>>, PlanError> {
        self.effects.lock().ensure_open()?;
        let Some(ticket) = self.dependencies.next_maintenance()? else {
            return Ok(None);
        };
        let hash = ticket.hash().cloned();
        // Never retain an owner point guard while reading dependency shards:
        // the final mixed cut owns the one canonical ascending lock order.
        let owner = hash
            .as_ref()
            .and_then(|hash| self.entries.get(hash).as_deref().cloned());
        let expected_owner_version = owner.as_ref().map(|owner| owner.record().version);
        let evidence = match &owner {
            Some(OwnedTx::ReplacementHistory(history))
                if history.observation().contains(ticket.key()) =>
            {
                Some(
                    self.dependencies
                        .capture_settlement_evidence(
                            &history.record().identity.raw,
                            history.observation().observed(),
                            None,
                            None,
                        )
                        .map_err(|error| {
                            self.classify_dependency_maintenance_plan_error(
                                &ticket,
                                expected_owner_version,
                                PlanError::from(error),
                            )
                        })?,
                )
            }
            Some(OwnedTx::ReplacementHistory(_)) => None,
            Some(OwnedTx::PreAccepted(_) | OwnedTx::Accepted(_)) | None => None,
        };
        let action = ticket
            .action(owner.as_ref(), evidence.as_ref())
            .map_err(|error| {
                self.classify_dependency_maintenance_plan_error(
                    &ticket,
                    expected_owner_version,
                    PlanError::from(error),
                )
            })?;
        let maintenance = self
            .dependencies
            .plan_maintenance(ticket.clone())
            .map_err(|error| {
                self.classify_dependency_maintenance_plan_error(
                    &ticket,
                    expected_owner_version,
                    PlanError::from(error),
                )
            })?;
        match action {
            DependencyMaintenanceAction::Advance => {
                let dependency = self
                    .dependencies
                    .seal_shared_maintenance(maintenance)
                    .map_err(|error| {
                        self.classify_dependency_maintenance_plan_error(
                            &ticket,
                            expected_owner_version,
                            PlanError::from(error),
                        )
                    })?;
                let mut owner_cuts = Vec::new();
                if let Some(owner) = &owner {
                    owner_cuts
                        .try_reserve_exact(1)
                        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                    owner_cuts.push(IndependentOwnerCut {
                        key: owner.record().identity.raw.clone(),
                        expected: OwnerPrestate::from_owner(owner),
                        action: IndependentOwnerAction::Observe,
                    });
                }
                let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
                let delta = IndependentDelta {
                    owner_cuts,
                    owners: DerivedOwnerDelta {
                        indexes: IndexDelta::default(),
                        sources: SourceVersionDelta::empty(),
                        template_sources: ShardOwnerSourcePlan::none(),
                    },
                    resource: None,
                    projection: ProjectionDelta::empty(),
                    scheduler: SchedulerBatchDelta::default(),
                    dependency,
                    effect: EffectDelta::default(),
                    clocks: clocks.finish(),
                    async_process_starts: Vec::new(),
                    removals: Vec::new(),
                    retired: RetiredOwners::default(),
                };
                let support = delta.physical_support(self);
                return Ok(Some(PreparedIndependentApply::Shared {
                    authority: self,
                    delta,
                    support,
                    staged_effect: None,
                }));
            }
            DependencyMaintenanceAction::Requeue => {}
        }

        #[cfg(test)]
        self.entries.enter_dependency_maintenance_plan_probe();

        let hash = hash.ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let existing = owner.ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        if existing.record().identity.raw != hash {
            return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
        }
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let after = match existing.clone() {
            OwnedTx::PreAccepted(preaccepted) => existing
                .with_preaccepted_phase(
                    PreAcceptedPhase::Queued(QueuedWork::Resolve),
                    version,
                    preaccepted.original_charge(),
                )
                .map_err(PlanError::Stale)?,
            OwnedTx::ReplacementHistory(history) => {
                OwnedTx::PreAccepted(history.into_recovery(self.generation, version))
            }
            OwnedTx::Accepted(_) => {
                return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
            }
        };
        let delta = self
            .compile_entry_delta_with_controls(
                EntryTransition::Replace {
                    key: hash,
                    before: existing,
                    after,
                },
                clocks.finish(),
                sequence,
                TransitionControls::none(),
                None,
            )
            .map_err(|error| {
                self.classify_dependency_maintenance_plan_error(
                    &ticket,
                    expected_owner_version,
                    error,
                )
            })?;
        let (delta, support) = delta
            .into_shared_maintenance(self, maintenance, evidence)
            .map_err(|error| {
                self.classify_dependency_maintenance_plan_error(
                    &ticket,
                    expected_owner_version,
                    error,
                )
            })?;
        Ok(Some(PreparedIndependentApply::Shared {
            authority: self,
            delta,
            support,
            staged_effect: None,
        }))
    }

    fn checkout_eligibility(
        &self,
        preaccepted: &PreAcceptedEntry,
        permit: super::state::WorkPermit,
    ) -> Result<CheckoutEligibility, PlanError> {
        let PreAcceptedPhase::Queued(queued) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let queued_lane = match queued {
            QueuedWork::Resolve => QueueLane::Resolve,
            QueuedWork::Verify(_) => QueueLane::Verify,
        };
        if QueueLane::for_permit(permit) != queued_lane {
            return Err(PlanError::Stale(StalePlan::Phase));
        }
        if let QueuedWork::Verify(resolved) = queued
            && !self
                .dependencies
                .proof_is_current(resolved.payload().dependencies(), resolved.dependency_cut())
        {
            // Dependency publication intentionally avoids an unbounded eager
            // scheduler rewrite. Until bounded maintenance requeues this
            // owner, its derived queue head is locally ineligible; it must not
            // abort selection for unrelated owners.
            return Ok(CheckoutEligibility::StaleDependency);
        }

        let grant = self.resources.compute_grant(preaccepted, permit);
        if let QueuedWork::Verify(resolved) = queued
            && grant
                .retained_charge(
                    resolved.payload().resolved_resident_bytes(),
                    resolved.payload().footprint.edge_count(),
                )
                .is_none()
        {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
        }
        let charge = self
            .resources
            .retained_entry_charge(
                preaccepted,
                preaccepted.basis.payload_bytes(),
                preaccepted.dependencies().len(),
            )
            .map_err(|_| PlanError::Fault(AuthorityFault::ResourceProjection))?;
        let charge = charge
            .reserve_compute(grant)
            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
        Ok(CheckoutEligibility::Ready {
            grant,
            after_charge: charge,
        })
    }

    /// Consume a move-only compute completion in one atomic command. A
    /// successful Plan is intentionally not exposed as a droppable value:
    /// doing so could destroy the only lease completion while the authority
    /// still retained `Computing`.
    #[expect(
        clippy::result_large_err,
        reason = "the linear failure must return the exact unboxed settlement capability; boxing would allocate on effect/resource backpressure"
    )]
    pub(in crate::authority) fn prepare_shared_compute_settlement(
        &self,
        settlement: ComputeSettlement,
    ) -> Result<SharedComputeSettlementPreparation<'_>, ComputeSettlementFailure> {
        let ComputeSettlement { token, next } = settlement;
        let recompute_next = match &next {
            SettlementNext::Waiting(missing) => SettlementNext::Waiting(missing.clone()),
            _ => SettlementNext::Retry,
        };
        let policy = match self.compile_settlement_policy(&token, next) {
            Ok(policy) => policy,
            Err(PrepareSettlementError::Recompute(error)) => {
                return Err(self.compute_settlement_planning_failure(
                    error,
                    ComputeSettlement {
                        token,
                        next: recompute_next,
                    },
                ));
            }
            Err(PrepareSettlementError::Preserve { error, next }) => {
                return Err(self.compute_settlement_planning_failure(
                    error,
                    ComputeSettlement { token, next },
                ));
            }
        };
        let compilation = match self.compile_shared_settlement_entry(&token, policy) {
            Ok(compilation) => compilation,
            Err(PrepareSettlementError::Recompute(error)) => {
                return Err(self.compute_settlement_planning_failure(
                    error,
                    ComputeSettlement {
                        token,
                        next: recompute_next,
                    },
                ));
            }
            Err(PrepareSettlementError::Preserve { error, next }) => {
                return Err(self.compute_settlement_planning_failure(
                    error,
                    ComputeSettlement { token, next },
                ));
            }
        };
        match compilation {
            SharedSettlementEntryCompilation::PeerRevocation {
                core,
                recovery_next,
            } => Ok(SharedComputeSettlementPreparation::PeerRevocation(
                PreparedSharedComputePeerRevocation {
                    core,
                    recovery: ComputeSettlement {
                        token,
                        next: recovery_next,
                    },
                },
            )),
            SharedSettlementEntryCompilation::Entry {
                delta,
                dependency,
                publication,
                recovery_next,
                sequence,
            } => {
                let (delta, support) = match delta.into_shared_settlement(self, dependency) {
                    Ok(compiled) => compiled,
                    Err(error) => {
                        return Err(self.compute_settlement_planning_failure(
                            error,
                            ComputeSettlement {
                                token,
                                next: recovery_next,
                            },
                        ));
                    }
                };
                let staged_effect = match publication {
                    Some(publication) => {
                        let effect = match self
                            .effects_for_plan()
                            .plan_publication(&publication, sequence)
                        {
                            Ok(effect) => effect,
                            Err(error) => {
                                return Err(self.compute_settlement_failure(
                                    error.into(),
                                    ComputeSettlement {
                                        token,
                                        next: recovery_next,
                                    },
                                ));
                            }
                        };
                        match EffectLog::stage_publication(&self.effects, effect) {
                            Ok(staged) => Some(staged),
                            Err(error) => {
                                return Err(self.compute_settlement_failure(
                                    error.into(),
                                    ComputeSettlement {
                                        token,
                                        next: recovery_next,
                                    },
                                ));
                            }
                        }
                    }
                    None => None,
                };
                Ok(SharedComputeSettlementPreparation::Prepared(
                    PreparedSharedComputeSettlement {
                        authority: self,
                        delta,
                        support,
                        staged_effect,
                        recovery: ComputeSettlement {
                            token,
                            next: recovery_next,
                        },
                    },
                ))
            }
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the linear planning failure returns the exact recovery state without allocating on stale, resource, or effect backpressure"
    )]
    fn compile_shared_settlement_entry(
        &self,
        token: &SettlementToken,
        policy: SettlementPolicy,
    ) -> Result<SharedSettlementEntryCompilation<'_>, PrepareSettlementError> {
        let SettlementPolicy {
            existing,
            dependency,
            disposition,
            mut recovery_next,
        } = policy;
        let (phase, retained_charge, publication) = match disposition {
            SettlementDisposition::Retain { phase, charge } => (phase, charge, None),
            SettlementDisposition::RetainAndPublish {
                phase,
                charge,
                publication,
            } => (phase, charge, Some(publication)),
            SettlementDisposition::Terminal(rejection) => {
                let OwnedTx::PreAccepted(preaccepted) = &existing else {
                    return Err(PlanError::Stale(StalePlan::Phase).into());
                };
                let reason = rejection.into_public();
                if reason.is_malformed()
                    && let Some(peer) = preaccepted.source.payload_blame_peer()
                {
                    let culprit = preaccepted.record.identity.raw.clone();
                    let result = self.compile_shared_peer_revocation_core(peer, culprit, reason);
                    return match result {
                        Ok(core) => Ok(SharedSettlementEntryCompilation::PeerRevocation {
                            core,
                            recovery_next,
                        }),
                        Err(error) => Err(PrepareSettlementError::Preserve {
                            error,
                            next: recovery_next,
                        }),
                    };
                }
                let result = self.compile_shared_compute_rejection_entry(existing, reason);
                return match result {
                    Ok((delta, publication, sequence)) => {
                        Ok(SharedSettlementEntryCompilation::Entry {
                            delta,
                            dependency,
                            publication: Some(publication),
                            recovery_next,
                            sequence,
                        })
                    }
                    Err(error) => Err(PrepareSettlementError::Preserve {
                        error,
                        next: recovery_next,
                    }),
                };
            }
        };
        let result =
            (|| -> Result<(EntryDelta, Option<EffectPublication>, ApplySequence), PlanError> {
                let OwnedTx::PreAccepted(preaccepted) = &existing else {
                    return Err(PlanError::Stale(StalePlan::Phase));
                };
                let expected_charge = existing.charge_record();
                let desired_charge = ChargeRecord::PreAccepted {
                    resources: retained_charge,
                    residency_peer: preaccepted.source.ingress_peer(),
                    compute_peer: None,
                };
                let resource = match self.resources_for_plan().plan_replace(
                    token.hash.clone(),
                    Some(expected_charge),
                    Some(desired_charge),
                ) {
                    Ok(resource) => resource,
                    Err(
                        ResourceError::PreAcceptedLimit
                        | ResourceError::RemoteLimit
                        | ResourceError::PeerLimit(_),
                    ) => {
                        let rejection = SettlementRejection::ResourceBound(
                            CommittedPublicReject::new(Reject::Full(
                                "transaction exceeds the tx-pool residency envelope".to_owned(),
                            )),
                        );
                        recovery_next = SettlementNext::Rejected(rejection.clone());
                        let (delta, publication, sequence) = self
                            .compile_shared_compute_rejection_entry(
                                existing,
                                rejection.into_public(),
                            )?;
                        return Ok((delta, Some(publication), sequence));
                    }
                    Err(error) => return Err(error.into()),
                };
                if let Some(publication) = publication.as_ref() {
                    self.effects
                        .lock()
                        .preflight_publication(publication)
                        .map_err(PlanError::from)?;
                }
                let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
                    .map_err(PlanError::from)?;
                let sequence = clocks.sequence();
                let (version, clocks) = clocks.replacement().map_err(PlanError::from)?;
                let after = existing
                    .with_preaccepted_phase(phase, version, retained_charge)
                    .map_err(PlanError::Stale)?;
                let delta = self.compile_entry_delta_with_controls(
                    EntryTransition::Replace {
                        key: token.hash.clone(),
                        before: existing,
                        after,
                    },
                    clocks.finish(),
                    sequence,
                    TransitionControls::none(),
                    Some(resource),
                )?;
                Ok((delta, publication, sequence))
            })();
        match result {
            Ok((delta, publication, sequence)) => Ok(SharedSettlementEntryCompilation::Entry {
                delta,
                dependency,
                publication,
                recovery_next,
                sequence,
            }),
            Err(error) => Err(PrepareSettlementError::Preserve {
                error,
                next: recovery_next,
            }),
        }
    }

    fn compute_settlement_failure(
        &self,
        error: PlanError,
        settlement: ComputeSettlement,
    ) -> ComputeSettlementFailure {
        let ComputeSettlement { token, next } = settlement;
        if matches!(error, PlanError::Stale(_)) {
            return match self.settlement_owner_stale(&token) {
                Some(stale) => ComputeSettlementFailure::new(PlanError::Stale(stale), token, next),
                None => ComputeSettlementFailure::new(
                    PlanError::Fault(AuthorityFault::DependencyProjection),
                    token,
                    next,
                ),
            };
        }
        ComputeSettlementFailure::new(error, token, next)
    }

    fn compute_settlement_planning_failure(
        &self,
        error: PlanError,
        settlement: ComputeSettlement,
    ) -> ComputeSettlementFailure {
        match error {
            PlanError::Stale(stale) => self.compute_settlement_changed_cut_failure(
                SettlementChangedCut::planning(stale),
                settlement,
            ),
            error => self.compute_settlement_failure(error, settlement),
        }
    }

    fn compute_settlement_changed_cut_failure(
        &self,
        changed_cut: SettlementChangedCut,
        settlement: ComputeSettlement,
    ) -> ComputeSettlementFailure {
        let ComputeSettlement { token, next } = settlement;
        match self.settlement_owner_stale(&token) {
            Some(stale) => ComputeSettlementFailure::new(PlanError::Stale(stale), token, next),
            None => ComputeSettlementFailure::retry_exact(changed_cut, token, next),
        }
    }

    fn settlement_owner_stale(&self, token: &SettlementToken) -> Option<StalePlan> {
        match self.entries.get(&token.hash).as_deref() {
            None => Some(StalePlan::Missing),
            Some(owner) if owner.record().version != token.version => Some(StalePlan::Version),
            Some(OwnedTx::PreAccepted(preaccepted))
                if matches!(preaccepted.phase, PreAcceptedPhase::Computing(_)) =>
            {
                None
            }
            Some(_) => Some(StalePlan::Phase),
        }
    }

    #[cfg(test)]
    #[expect(
        clippy::result_large_err,
        reason = "the linear failure must return the exact unboxed settlement capability; boxing would allocate on effect/resource backpressure"
    )]
    pub(super) fn apply_settlement(
        &mut self,
        settlement: ComputeSettlement,
    ) -> Result<CommittedDelta, ComputeSettlementFailure> {
        let ComputeSettlement { token, next } = settlement;
        match self.prepare_settlement(&token, next) {
            Ok(plan) => Ok(plan.apply()),
            Err(PrepareSettlementError::Recompute(error)) => Err(ComputeSettlementFailure::new(
                error,
                token,
                SettlementNext::Retry,
            )),
            Err(PrepareSettlementError::Preserve { error, next }) => {
                Err(ComputeSettlementFailure::new(error, token, next))
            }
        }
    }

    #[cfg(test)]
    #[expect(
        clippy::result_large_err,
        reason = "the error owns the exact linear SettlementNext capability; boxing would allocate on ordinary stale/backpressure recovery"
    )]
    fn prepare_settlement<'a>(
        &'a mut self,
        token: &SettlementToken,
        next: SettlementNext,
    ) -> Result<PreparedApply<'a>, PrepareSettlementError> {
        // A Remote missing frontier is non-rebuildable publication detail. If
        // journal capacity is temporarily full, preserve that exact bounded
        // result instead of converting it to `Retry` and hot-looping through
        // resolution. Other compute results are safely reproducible, except
        // terminal rejections which the inner planner retains explicitly.
        let waiting_retry = match &next {
            SettlementNext::Waiting(missing) => Some(missing.clone()),
            _ => None,
        };
        self.prepare_settlement_inner(token, next)
            .map_err(|error| match (error, waiting_retry) {
                (PrepareSettlementError::Recompute(error), Some(missing)) => {
                    PrepareSettlementError::Preserve {
                        error,
                        next: SettlementNext::Waiting(missing),
                    }
                }
                (error, _) => error,
            })
    }

    /// Classify one finished compute result against a coherent authority cut.
    ///
    /// `OwnerLocal` results change only the named owner and its derived
    /// projections. The compute exchange visits them by owner version and
    /// retains the deterministic greedy subsequence admitted by its shared
    /// resource projection; an overflowing owner returns to exact settlement,
    /// while a later independent owner may still fit. Results which may
    /// publish an effect, revoke a peer, or terminalize dependency owners
    /// remain `NonLocal` and retain their exact move-only evidence for the
    /// existing cohort planner.
    fn classify_settlement(
        &self,
        preaccepted: &PreAcceptedEntry,
        active: &super::state::ActiveWork,
        dependency: &SettlementDependencyEvidence,
        next: SettlementNext,
    ) -> Result<SettlementClassification, PlanError> {
        let raw_charge = preaccepted.original_charge();
        let dependency_cut = active.dependency_cut;
        if !dependency.proof_is_current(preaccepted.dependencies(), dependency_cut) {
            return Ok(SettlementClassification::OwnerLocal(OwnerLocalSettlement {
                phase: OwnerLocalPhase::Resolve,
                charge: raw_charge,
            }));
        }

        let chain_state_is_current = self.chain_view.has_same_chain_state(&active.chain_view);
        let local = |phase: OwnerLocalPhase, charge| {
            SettlementClassification::OwnerLocal(OwnerLocalSettlement { phase, charge })
        };
        match next {
            SettlementNext::QueuedVerify(resolved) => {
                if resolved.payload().identity() != &preaccepted.record.identity
                    || resolved.chain_view() != &active.chain_view
                {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                if resolved.dependency_cut() != dependency_cut {
                    return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                }
                if !chain_state_is_current {
                    return Ok(local(OwnerLocalPhase::Resolve, raw_charge));
                }
                let dependencies = resolved.payload().dependencies().clone();
                let retained_charge = active
                    .grant
                    .retained_charge(
                        resolved.payload().resolved_resident_bytes(),
                        dependencies.len(),
                    )
                    .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                if dependency.resolution_is_current(
                    preaccepted.dependencies(),
                    &dependencies,
                    dependency_cut,
                ) {
                    Ok(local(OwnerLocalPhase::Verify(resolved), retained_charge))
                } else {
                    Ok(local(OwnerLocalPhase::Resolve, raw_charge))
                }
            }
            SettlementNext::Waiting(missing) => {
                let dependencies = missing.dependencies().clone();
                if chain_state_is_current
                    && dependency.missing_result_is_current(
                        preaccepted.dependencies(),
                        &dependencies,
                        missing.missing(),
                        dependency_cut,
                    )
                {
                    Ok(SettlementClassification::NonLocal(
                        NonLocalSettlement::Waiting(missing),
                    ))
                } else {
                    Ok(local(OwnerLocalPhase::Resolve, raw_charge))
                }
            }
            SettlementNext::Ready(verified) => {
                if verified.payload().identity() != &preaccepted.record.identity
                    || verified.chain_view() != &active.chain_view
                {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                if verified.dependency_cut() != dependency_cut {
                    return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                }
                let dependencies = verified.payload().dependencies().clone();
                let retained_charge = active
                    .grant
                    .retained_charge(verified.metrics().cost.resident_bytes, dependencies.len())
                    .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                if dependency.resolution_is_current(
                    preaccepted.dependencies(),
                    &dependencies,
                    dependency_cut,
                ) {
                    Ok(local(OwnerLocalPhase::Ready(verified), retained_charge))
                } else {
                    Ok(local(OwnerLocalPhase::Resolve, raw_charge))
                }
            }
            SettlementNext::Rejected(rejection) => {
                if chain_state_is_current || rejection.remains_valid_after_chain_change() {
                    Ok(SettlementClassification::NonLocal(
                        NonLocalSettlement::Rejected(rejection),
                    ))
                } else {
                    Ok(local(OwnerLocalPhase::Resolve, raw_charge))
                }
            }
            SettlementNext::VerificationRejected {
                rejection,
                resolved,
            } => {
                if resolved.payload().identity() != &preaccepted.record.identity
                    || resolved.chain_view() != &active.chain_view
                {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                if resolved.dependency_cut() != dependency_cut {
                    return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                }
                let current_policy = preaccepted.source.payload_policy();
                match active.payload_policy.evolution_to(current_policy) {
                    PayloadPolicyEvolution::Unchanged => {
                        if chain_state_is_current {
                            Ok(SettlementClassification::NonLocal(
                                NonLocalSettlement::VerificationRejected {
                                    rejection,
                                    resolved,
                                },
                            ))
                        } else {
                            Ok(local(OwnerLocalPhase::Resolve, raw_charge))
                        }
                    }
                    PayloadPolicyEvolution::RemoteToTrusted => {
                        if !chain_state_is_current {
                            return Ok(local(OwnerLocalPhase::Resolve, raw_charge));
                        }
                        let dependencies = resolved.payload().dependencies().clone();
                        let retained_charge = active
                            .grant
                            .retained_charge(
                                resolved.payload().resolved_resident_bytes(),
                                dependencies.len(),
                            )
                            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                        if dependency.resolution_is_current(
                            preaccepted.dependencies(),
                            &dependencies,
                            dependency_cut,
                        ) {
                            Ok(local(OwnerLocalPhase::Verify(resolved), retained_charge))
                        } else {
                            Ok(local(OwnerLocalPhase::Resolve, raw_charge))
                        }
                    }
                    PayloadPolicyEvolution::Invalid => {
                        Err(PlanError::Fault(AuthorityFault::MembershipProjection))
                    }
                }
            }
            SettlementNext::Retry => Ok(local(OwnerLocalPhase::Resolve, raw_charge)),
        }
    }

    /// Bind the exact compute result to one canonical owner policy before
    /// choosing exclusive or shared physical Apply. The returned recovery is
    /// the minimum move-owned evidence which preserves the existing retry
    /// contract if later planning or OCC cannot commit.
    #[expect(
        clippy::result_large_err,
        reason = "the policy error owns the exact unboxed settlement result needed for recovery"
    )]
    fn compile_settlement_policy(
        &self,
        token: &SettlementToken,
        next: SettlementNext,
    ) -> Result<SettlementPolicy, PrepareSettlementError> {
        let existing = self
            .entries
            .get(&token.hash)
            .ok_or(PlanError::Stale(StalePlan::Missing))?
            .clone();
        if existing.record().version != token.version {
            return Err(PlanError::Stale(StalePlan::Version).into());
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase).into());
        };
        let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase).into());
        };
        if preaccepted.charge.active_work != 1 {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection).into());
        }
        let (candidate_dependencies, missing_dependencies) = settlement_dependency_inputs(&next);
        let dependency = self
            .dependencies
            .capture_settlement_evidence(
                &token.hash,
                preaccepted.dependencies(),
                candidate_dependencies,
                missing_dependencies,
            )
            .map_err(PlanError::from)?;
        let (disposition, recovery_next) =
            match self.classify_settlement(preaccepted, active, &dependency, next)? {
                SettlementClassification::OwnerLocal(OwnerLocalSettlement { phase, charge }) => (
                    SettlementDisposition::Retain {
                        phase: phase.into_preaccepted(),
                        charge,
                    },
                    SettlementNext::Retry,
                ),
                SettlementClassification::NonLocal(NonLocalSettlement::Waiting(missing)) => {
                    let dependencies = missing.dependencies().clone();
                    match self.missing_resolution_disposition(preaccepted.source, missing.missing())
                    {
                        MissingResolutionDisposition::Reject(rejection) => {
                            let recovery = rejection.clone();
                            (
                                SettlementDisposition::Terminal(rejection),
                                SettlementNext::Rejected(recovery),
                            )
                        }
                        MissingResolutionDisposition::Wait => {
                            let retained_charge = active
                                .grant
                                .retained_base_charge(dependencies.len())
                                .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                            let observed = self.dependencies.observe_missing(
                                missing.missing(),
                                dependencies,
                                active.dependency_cut,
                            );
                            let publication = match preaccepted.source {
                                PreAcceptedSource::Remote(remote) => ParentTransactionRequest::new(
                                    remote.residency.peer,
                                    Arc::clone(missing.parent_transactions()),
                                )
                                .map(|request| {
                                    self.effects.lock().build_single_publication(
                                        EffectPolicy::Remote,
                                        CommittedEffect::ParentTransactionsRequested(request),
                                    )
                                })
                                .transpose()
                                .map_err(PlanError::from)?,
                                PreAcceptedSource::Proposal { .. }
                                | PreAcceptedSource::Recovery(_) => None,
                            };
                            let recovery = missing.clone();
                            let disposition = match publication {
                                Some(publication) => SettlementDisposition::RetainAndPublish {
                                    phase: PreAcceptedPhase::Waiting(observed),
                                    charge: retained_charge,
                                    publication,
                                },
                                None => SettlementDisposition::Retain {
                                    phase: PreAcceptedPhase::Waiting(observed),
                                    charge: retained_charge,
                                },
                            };
                            (disposition, SettlementNext::Waiting(recovery))
                        }
                    }
                }
                SettlementClassification::NonLocal(NonLocalSettlement::Rejected(rejection)) => {
                    let recovery = rejection.clone();
                    (
                        SettlementDisposition::Terminal(rejection),
                        SettlementNext::Rejected(recovery),
                    )
                }
                SettlementClassification::NonLocal(NonLocalSettlement::VerificationRejected {
                    rejection,
                    resolved,
                }) => {
                    drop(resolved);
                    let rejection = SettlementRejection::ChainBound(rejection);
                    let recovery = rejection.clone();
                    (
                        SettlementDisposition::Terminal(rejection),
                        SettlementNext::Rejected(recovery),
                    )
                }
            };
        Ok(SettlementPolicy {
            existing,
            dependency,
            disposition,
            recovery_next,
        })
    }

    #[cfg(test)]
    #[expect(
        clippy::result_large_err,
        reason = "the error owns the exact linear SettlementNext capability; boxing would allocate on ordinary stale/backpressure recovery"
    )]
    fn prepare_settlement_inner<'a>(
        &'a mut self,
        token: &SettlementToken,
        next: SettlementNext,
    ) -> Result<PreparedApply<'a>, PrepareSettlementError> {
        let existing = self
            .entries
            .get(&token.hash)
            .ok_or(PlanError::Stale(StalePlan::Missing))?
            .clone();
        if existing.record().version != token.version {
            return Err(PlanError::Stale(StalePlan::Version).into());
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase).into());
        };
        let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase).into());
        };
        // The non-reused EntryVersion and Computing phase decide completion
        // authority. The move-only work value prevents duplicate settlement;
        // a second numeric compute identity would repeat the version without
        // distinguishing any legal transition. Chain identity
        // decides only whether the resulting proof may be retained: a tip
        // change cannot invalidate the sole capability able to release this
        // Computing owner and its active charge.
        if preaccepted.charge.active_work != 1 {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection).into());
        }
        let (candidate_dependencies, missing_dependencies) = settlement_dependency_inputs(&next);
        let dependency = self
            .dependencies
            .capture_settlement_evidence(
                &token.hash,
                preaccepted.dependencies(),
                candidate_dependencies,
                missing_dependencies,
            )
            .map_err(PlanError::from)?;
        let disposition = match self.classify_settlement(preaccepted, active, &dependency, next)? {
            SettlementClassification::OwnerLocal(OwnerLocalSettlement { phase, charge }) => {
                SettlementDisposition::Retain {
                    phase: phase.into_preaccepted(),
                    charge,
                }
            }
            SettlementClassification::NonLocal(NonLocalSettlement::Waiting(missing)) => {
                let dependencies = missing.dependencies().clone();
                match self.missing_resolution_disposition(preaccepted.source, missing.missing()) {
                    MissingResolutionDisposition::Reject(rejection) => {
                        SettlementDisposition::Terminal(rejection)
                    }
                    MissingResolutionDisposition::Wait => {
                        let retained_charge = active
                            .grant
                            .retained_base_charge(dependencies.len())
                            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                        let observed = self.dependencies.observe_missing(
                            missing.missing(),
                            dependencies,
                            active.dependency_cut,
                        );
                        let publication = match preaccepted.source {
                            PreAcceptedSource::Remote(remote) => ParentTransactionRequest::new(
                                remote.residency.peer,
                                Arc::clone(missing.parent_transactions()),
                            )
                            .map(|request| {
                                self.effects.lock().build_single_publication(
                                    EffectPolicy::Remote,
                                    CommittedEffect::ParentTransactionsRequested(request),
                                )
                            })
                            .transpose()
                            .map_err(PlanError::from)?,
                            PreAcceptedSource::Proposal { .. } | PreAcceptedSource::Recovery(_) => {
                                None
                            }
                        };
                        match publication {
                            Some(publication) => SettlementDisposition::RetainAndPublish {
                                phase: PreAcceptedPhase::Waiting(observed),
                                charge: retained_charge,
                                publication,
                            },
                            None => SettlementDisposition::Retain {
                                phase: PreAcceptedPhase::Waiting(observed),
                                charge: retained_charge,
                            },
                        }
                    }
                }
            }
            SettlementClassification::NonLocal(NonLocalSettlement::Rejected(rejection)) => {
                SettlementDisposition::Terminal(rejection)
            }
            SettlementClassification::NonLocal(NonLocalSettlement::VerificationRejected {
                rejection,
                resolved,
            }) => {
                // The single-result path has already validated the exact
                // resolved receipt. Once the outcome becomes a committed
                // public rejection, that payload is no longer retry evidence.
                drop(resolved);
                SettlementDisposition::Terminal(SettlementRejection::ChainBound(rejection))
            }
        };
        let (phase, retained_charge, publication) = match disposition {
            SettlementDisposition::Retain { phase, charge } => (phase, charge, None),
            SettlementDisposition::RetainAndPublish {
                phase,
                charge,
                publication,
            } => (phase, charge, Some(publication)),
            SettlementDisposition::Terminal(rejection) => {
                let retry_rejection = rejection.clone();
                return self
                    .prepare_compute_rejection(existing, rejection)
                    .map_err(|error| PrepareSettlementError::Preserve {
                        error,
                        next: SettlementNext::Rejected(retry_rejection),
                    });
            }
        };
        let expected_charge = existing.charge_record();
        let desired_charge = ChargeRecord::PreAccepted {
            resources: retained_charge,
            residency_peer: preaccepted.source.ingress_peer(),
            compute_peer: None,
        };
        let (phase, retained_charge, resource) = match self.resources_for_plan().plan_replace(
            token.hash.clone(),
            Some(expected_charge),
            Some(desired_charge),
        ) {
            Ok(resource) => (phase, retained_charge, resource),
            Err(
                ResourceError::PreAcceptedLimit
                | ResourceError::RemoteLimit
                | ResourceError::PeerLimit(_),
            ) => {
                let rejection = SettlementRejection::ResourceBound(CommittedPublicReject::new(
                    Reject::Full("transaction exceeds the tx-pool residency envelope".to_owned()),
                ));
                let retry_rejection = rejection.clone();
                return self
                    .prepare_compute_rejection(existing, rejection)
                    .map_err(|error| PrepareSettlementError::Preserve {
                        error,
                        next: SettlementNext::Rejected(retry_rejection),
                    });
            }
            Err(error) => return Err(PlanError::from(error).into()),
        };
        if let Some(publication) = publication.as_ref() {
            self.effects
                .lock()
                .preflight_publication(publication)
                .map_err(PlanError::from)?;
        }
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
            .map_err(PlanError::from)?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement().map_err(PlanError::from)?;
        let after = existing
            .with_preaccepted_phase(phase, version, retained_charge)
            .map_err(PlanError::Stale)?;
        let effect = publication
            .as_ref()
            .map_or_else(
                || Ok(EffectDelta::default()),
                |publication| {
                    self.effects_for_plan()
                        .plan_publication(publication, sequence)
                },
            )
            .map_err(PlanError::from)?;
        self.prepare_entry_delta_with_controls(
            EntryTransition::Replace {
                key: token.hash.clone(),
                before: existing,
                after,
            },
            clocks.finish(),
            sequence,
            TransitionControls::effect(effect),
            Some(resource),
        )
        .map_err(PrepareSettlementError::from)
    }

    #[cfg(test)]
    fn prepare_compute_rejection(
        &mut self,
        existing: OwnedTx,
        rejection: SettlementRejection,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if !matches!(preaccepted.phase, PreAcceptedPhase::Computing(_)) {
            return Err(PlanError::Stale(StalePlan::Phase));
        }
        let blame_peer = preaccepted.source.payload_blame_peer();
        let reason = rejection.into_public();
        if reason.is_malformed()
            && let Some(peer) = blame_peer
        {
            return self.plan_peer_revocation(
                peer,
                preaccepted.record.identity.raw.clone(),
                reason,
            );
        }
        let delta = self.compile_compute_rejection_entry(existing, reason)?;
        PreparedApply::stage(self, DependencyAuthorityDelta::Entry(delta))
    }

    fn compile_shared_compute_rejection_entry(
        &self,
        existing: OwnedTx,
        reason: CommittedPublicReject,
    ) -> Result<(EntryDelta, EffectPublication, ApplySequence), PlanError> {
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if !matches!(preaccepted.phase, PreAcceptedPhase::Computing(_))
            || reason.is_malformed() && preaccepted.source.payload_blame_peer().is_some()
        {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let audience = RejectionAudience::from_source(preaccepted.source);
        let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        let publication = self
            .effects
            .lock()
            .build_single_publication(
                policy,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience,
                    reason,
                }),
            )
            .map_err(PlanError::from)?;
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let dependency = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let key = preaccepted.record.identity.raw.clone();
        let delta = self.compile_entry_delta_with_controls(
            EntryTransition::Remove {
                key,
                before: existing,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency(dependency),
            None,
        )?;
        Ok((delta, publication, sequence))
    }

    #[cfg(test)]
    fn compile_compute_rejection_entry(
        &self,
        existing: OwnedTx,
        reason: CommittedPublicReject,
    ) -> Result<EntryDelta, PlanError> {
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if !matches!(preaccepted.phase, PreAcceptedPhase::Computing(_))
            || reason.is_malformed() && preaccepted.source.payload_blame_peer().is_some()
        {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let audience = RejectionAudience::from_source(preaccepted.source);
        let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        let publication = self
            .effects
            .lock()
            .build_single_publication(
                policy,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience,
                    reason,
                }),
            )
            .map_err(PlanError::from)?;
        self.effects.lock().preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let dependency = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;
        let key = preaccepted.record.identity.raw.clone();
        self.compile_entry_delta_with_controls(
            EntryTransition::Remove {
                key,
                before: existing,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency_and_effect(dependency, effect),
            None,
        )
    }

    fn prepare_entry_delta(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            TransitionControls::none(),
            None,
        )
    }

    fn prepare_entry_delta_with_controls(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        controls: TransitionControls,
        explicit_resources: Option<ResourcePlan>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.compile_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            controls,
            explicit_resources,
        )?;
        PreparedApply::stage(self, DependencyAuthorityDelta::Entry(delta))
    }

    /// Compile the canonical one-owner transition without lending mutable
    /// authority to the planner. Exclusive callers wrap the result; shared
    /// callers lift the same delta into the existing independent batch engine.
    fn compile_entry_delta_with_controls(
        &self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        controls: TransitionControls,
        explicit_resources: Option<ResourcePlan>,
    ) -> Result<EntryDelta, PlanError> {
        self.effects.lock().ensure_open()?;
        let TransitionControls {
            dependency: dependency_control,
            effect,
        } = controls;
        let (key, expected, after, primary_insertions) = match transition {
            EntryTransition::Insert { key, after } => (key, None, Some(after), 1),
            EntryTransition::Replace { key, before, after } => (key, Some(before), Some(after), 0),
            EntryTransition::Remove { key, before } => (key, Some(before), None, 0),
        };
        let expected_owner = expected
            .as_ref()
            .map_or(OwnerPrestate::Vacant, OwnerPrestate::from_owner);
        let expected_charge = expected.as_ref().map(OwnedTx::charge_record);
        let after_charge = after.as_ref().map(OwnedTx::charge_record);
        if primary_insertions != 0 {
            self.reserve_primary_owner_insertions(std::iter::once(&key))?;
        }
        let resource = match explicit_resources {
            Some(resources) => resources,
            None => self.resources_for_plan().plan_replace(
                key.clone(),
                expected_charge,
                after_charge,
            )?,
        };
        let scheduler =
            self.scheduler
                .lock()
                .plan_replace(expected.as_ref(), after.as_ref(), None)?;
        let dependency = self
            .dependencies
            .plan_replace(expected.as_ref(), after.as_ref())?
            .with_control(dependency_control)
            .into_shared_batch(&self.dependencies, None)?;
        let sources = self.source_versions.plan_replacements(
            std::iter::once((expected.as_ref(), after.as_ref())),
            sequence,
        );
        let template_sources =
            self.plan_owner_sources(std::iter::once((&key, expected.as_ref(), after.as_ref())))?;
        let indexes =
            self.indexes_for_plan()
                .plan_replace(&key, expected.as_ref(), after.as_ref())?;
        let owners = DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        };
        Ok(EntryDelta {
            key,
            expected: expected_owner,
            after,
            owners,
            retired: RetiredOwners::default(),
            resource,
            scheduler,
            dependency,
            effect,
            clocks,
        })
    }
}

impl OwnedTx {
    pub(super) fn with_preaccepted_phase(
        &self,
        phase: PreAcceptedPhase,
        version: EntryVersion,
        charge: ResourceVector,
    ) -> Result<Self, StalePlan> {
        let Self::PreAccepted(entry) = self else {
            return Err(StalePlan::Phase);
        };
        let mut record = entry.record.clone();
        record.version = version;
        Ok(Self::PreAccepted(PreAcceptedEntry {
            record,
            source: entry.source,
            basis: entry.basis.clone(),
            phase,
            charge,
        }))
    }
}
