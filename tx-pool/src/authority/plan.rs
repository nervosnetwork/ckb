mod apply_seal;
mod chain_transition;
mod compute_exchange;
mod ingress;
mod membership;
mod settlement;

pub(in crate::authority) use self::apply_seal::ApplyToken;
pub(in crate::authority) use self::compute_exchange::{
    CommittedComputeExchange, ComputeExchangeAssignment, ComputeExchangeCompletion,
    ComputeExchangePlanFailure, ComputePeerExclusion, RecoveredComputeExchange,
    SharedComputeExchangeOutcome,
};
pub(in crate::authority) use self::ingress::{
    CommittedRetainedAdmissionBatch, ConcurrentRetainedIngressError, SharedRetainedIngressHead,
};

#[cfg(test)]
#[path = "tests/support/plan.rs"]
pub(in crate::authority) mod test_support;

#[cfg(test)]
use super::ban::PeerBanDelta;
use super::ban::{PeerBanError, PeerBanSlotBank};
use super::chain::{
    DirectAdmissionReceipt, DirectAdmissionRejection, ExpectedPreAcceptedOwner,
    FinalAdmissionPreparation, FinalAdmissionReceipt, FinalAdmissionSubject, FinalAdmissionWork,
};
use super::dependency::{
    DependencyApplyOutcome, DependencyBatchDelta, DependencyControlDelta,
    DependencyEntryControlDelta, DependencyError, DependencyFrontier, DependencyMaintenanceAction,
    DependencyMaintenancePlan, DependencyPrepareError, PreparedDependencyBatch,
    SettlementDependencyEvidence,
};
#[cfg(test)]
use super::effect::CommittedPeerCohortRevocation;
use super::effect::{
    CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
    CommittedRejection, CommittedRemoteIngressRelease, EffectBatch, EffectBuildError,
    EffectConfigError, EffectDelta, EffectError, EffectLimits, EffectLog, EffectPolicy,
    EffectPublication, EffectPublicationObservation, EffectReceipt, EffectSettlementError,
    EffectWakeProjection, GenerationResetPlanError, ParentTransactionRequest, PendingRecentReject,
    RejectionAudience,
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
    ResourceBatchPlan, ResourceCapacityWaitIdentity, ResourceCommitHealth, ResourceError,
    ResourceLedger, ResourceLimits, ResourceVector,
};
use super::scheduler::{
    FairFrontier, QueueLane, ReadyReservation, ReadySlotReservation, SchedulerBatchDelta,
    SchedulerDelta, SchedulerError, SchedulerWakeProjection, VerifyOrder,
};
use super::shard::{
    OwnerShardRemovalRevision, ShardApplySupport, ShardOwnerSourcePlan, ShardReadSupport,
    ShardWriteSupport,
};
use super::shard::{
    ShardProposedCountPlanError, ShardedOwnerMap, ShardedOwnerReadCut, ShardedOwnerReadGuard,
    ShardedOwnerWriteCut,
};
#[cfg(test)]
use super::source::AuthoritySourceVersionSnapshot;
use super::source::{AuthoritySourceVersions, PoolTemplateVersions, SourceVersionDelta};
use super::state::{
    AcceptedAtMillis, AcceptedEntry, AcceptedProvenance, AdmissionBasis, ApplySequence, Arrival,
    AsyncProcessStart, AuthorityClockBank, AuthorityClocks, ChainRevision, ChainViewId,
    DependencyCut, DependencyKey, EntryVersion, KnownDependencies, MissingDependencies, OwnedTx,
    OwnerClockProgress, PayloadPolicy, PayloadPolicyEvolution, PoolGeneration, PreAcceptedEntry,
    PreAcceptedPhase, PreAcceptedSource, ProposalBase, QueuedWork, RawTxHash, RemoteDeadline,
    ReplacementHistoryEntry, ReplacementHistoryError, ResolvedFacts, TxRecord, ValidatedAdmission,
    VerifiedFacts,
};
use super::validation::{FinalAdmissionValidationOutcome, ReadyPopulationCut};
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
pub(in crate::authority) use settlement::{SettlementBatch, SharedReadyWaveCompilation};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;
#[cfg(test)]
use std::time::Instant;

pub(in crate::authority) use apply_seal::TxPoolAuthority;
use apply_seal::{OwnerRemovalResourcePlan, OwnerResourceUpdate, PreparedOwnerResourceDelta};

impl TxPoolAuthority {
    pub(super) fn entry_guard(&self, hash: &RawTxHash) -> Option<ShardedOwnerReadGuard<'_>> {
        self.entries.get(hash)
    }

    #[cfg(test)]
    pub(super) fn entry(&self, hash: &RawTxHash) -> Option<OwnedTx> {
        self.entries.get(hash).as_deref().cloned()
    }

    pub(super) fn operational_metrics(&self) -> crate::metrics::OperationalMetrics {
        // Metrics own no policy. Read each resource shard independently so
        // observation cannot create a global transaction-commit barrier.
        let resources = self.resources.operational_totals(&self.entries);
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

    /// Capture the lifecycle identity and persistent sharded owner layout used
    /// to populate one Ready batch after the generation guard is released.
    pub(super) fn ready_population_cut(&self) -> ReadyPopulationCut {
        ReadyPopulationCut::new(
            self.generation,
            self.chain_view.clone(),
            self.entries.clone(),
        )
    }

    pub(super) fn ready_population_cut_is_current(&self, cut: &ReadyPopulationCut) -> bool {
        cut.matches(self.generation, &self.chain_view)
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

    fn wake_projection_without_effect(&self) -> AuthorityWakeProjection {
        let scheduler = self.scheduler.lock().wake_projection();
        self.wake_projection_with_scheduler_and_effect(scheduler, None)
    }

    fn wake_projection_with_scheduler(
        &self,
        scheduler: SchedulerWakeProjection,
    ) -> AuthorityWakeProjection {
        let effects = self.effects.lock().wake_projection();
        self.wake_projection_with_scheduler_and_effect(scheduler, Some(effects))
    }

    fn wake_projection_with_scheduler_without_effect(
        &self,
        scheduler: SchedulerWakeProjection,
    ) -> AuthorityWakeProjection {
        self.wake_projection_with_scheduler_and_effect(scheduler, None)
    }

    fn wake_projection_with_scheduler_and_effect(
        &self,
        scheduler: SchedulerWakeProjection,
        effects: Option<EffectWakeProjection>,
    ) -> AuthorityWakeProjection {
        let template = self.source_versions.template();
        AuthorityWakeProjection {
            scheduler,
            active_work: 0,
            effects,
            template: [
                template.proposals.barrier(),
                template.transactions.barrier(),
                template.chain,
            ],
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
    blame_peer: Option<ckb_network::PeerIndex>,
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
    OwnerOrProjection,
    Scheduler,
    ResourceCapacity,
}

impl SettlementChangedCut {
    fn planning(stale: StalePlan) -> Self {
        Self {
            domain: SettlementChangedDomain::Planning(stale),
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
}

impl ComputeSettlementRecovery {
    fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(stale) => Self::Obsolete(stale),
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::WaitEffectCapacity,
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
    fn new(
        error: PlanError,
        blame_peer: Option<ckb_network::PeerIndex>,
        token: SettlementToken,
        next: SettlementNext,
    ) -> Self {
        let recovery = ComputeSettlementRecovery::from_plan(error);
        let blame_peer = if matches!(&recovery, ComputeSettlementRecovery::Obsolete(_)) {
            None
        } else {
            blame_peer
        };
        Self {
            recovery,
            blame_peer,
            token,
            next,
        }
    }

    fn retry_exact(
        changed_cut: SettlementChangedCut,
        blame_peer: Option<ckb_network::PeerIndex>,
        token: SettlementToken,
        next: SettlementNext,
    ) -> Self {
        Self {
            recovery: ComputeSettlementRecovery::RetryExact(changed_cut),
            blame_peer,
            token,
            next,
        }
    }

    pub(super) fn recovery(&self) -> &ComputeSettlementRecovery {
        &self.recovery
    }

    pub(super) const fn blame_peer(&self) -> Option<ckb_network::PeerIndex> {
        self.blame_peer
    }

    pub(super) fn into_settlement(self) -> ComputeSettlement {
        let Self {
            recovery: _,
            blame_peer: _,
            token,
            next,
        } = self;
        ComputeSettlement { token, next }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectCloseError {
    ActiveWork,
    AlreadyClosed,
}

/// Effect settlement has the same linear handoff rule as compute: planning
/// failure must return the publisher receipt instead of silently losing the
/// only tentative endpoint cursor for the resident record.
#[derive(Debug)]
#[must_use = "a failed effect settlement still owns the exact publication receipt"]
pub(super) struct EffectSettlementFailure {
    error: EffectSettlementError,
    #[expect(
        dead_code,
        reason = "this move-only capability retains the exact cursor until the publisher fault is disposed"
    )]
    receipt: EffectReceipt,
}

impl EffectSettlementFailure {
    pub(super) fn error(&self) -> EffectSettlementError {
        self.error
    }
}

/// Journal-local post-commit capability. Effect acknowledgement changes no
/// owner, scheduler, dependency, resource or template authority, so it must
/// not impersonate a complete transaction Apply.
#[must_use = "retire the acknowledged batch before publishing its wake edge"]
pub(super) struct EffectSettlementApplied {
    retired_effect: Option<Arc<EffectBatch>>,
    wake: super::effect::EffectWakeTransition,
}

impl EffectSettlementApplied {
    pub(super) fn into_parts(
        self,
    ) -> (
        Option<Arc<EffectBatch>>,
        super::effect::EffectWakeTransition,
    ) {
        (self.retired_effect, self.wake)
    }

    #[cfg(test)]
    pub(in crate::authority) fn into_committed_for_foundation(self) -> CommittedDelta {
        let (retired_effect, wake) = self.into_parts();
        finish_apply_retirement(
            ApplyRetirement {
                async_process_observations: AsyncProcessObservations::None,
                removals: Vec::new(),
                retired: RetiredOwners::default(),
                retired_effect,
                retired_generation: None,
                dependency: None,
                template_source_changed: false,
            },
            AuthorityWakeTransition::effect(wake),
        )
    }
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
            IndexError::Stale => Self::Stale(StalePlan::Version),
            IndexError::ProposalCollision => Self::Backpressure(Backpressure::ProposalCollision),
            IndexError::Allocation => Self::Fault(AuthorityFault::IndexProjection),
            IndexError::Arithmetic => Self::Fault(AuthorityFault::CounterExhausted),
            IndexError::Projection => Self::Fault(AuthorityFault::IndexProjection),
        }
    }
}

impl From<SchedulerError> for PlanError {
    fn from(error: SchedulerError) -> Self {
        match error {
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

impl From<DependencyPrepareError> for PlanError {
    fn from(error: DependencyPrepareError) -> Self {
        match error {
            DependencyPrepareError::Stale => Self::Stale(StalePlan::Dependency),
            DependencyPrepareError::Projection => Self::Fault(AuthorityFault::DependencyProjection),
        }
    }
}

impl From<EffectError> for PlanError {
    fn from(error: EffectError) -> Self {
        match error {
            EffectError::Full => Self::Backpressure(Backpressure::EffectCapacity),
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
            PeerBanError::Faulted => Self::Fault(AuthorityFault::MembershipProjection),
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
    effects: Option<EffectWakeProjection>,
    template: [ApplySequence; 3],
}

/// Exact before/after runnable projection produced by one committed Apply.
///
/// It carries no authority state and cannot select work. The runtime consumes
/// it only after the store guard and retirement payloads have been released.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AuthorityWakeTransition {
    compute_advanced: bool,
    ready_advanced: bool,
    dependency_maintenance_activated: bool,
    effect_publisher_advanced: bool,
    effect_capacity_released: bool,
    template_source_changed: bool,
}

impl AuthorityWakeTransition {
    pub(in crate::authority) fn between(
        before: AuthorityWakeProjection,
        after: AuthorityWakeProjection,
    ) -> Self {
        let template_source_changed = before.template != after.template;
        let (effect_publisher_advanced, effect_capacity_released) =
            match (before.effects, after.effects) {
                (Some(before), Some(after)) => (
                    after.publisher_advanced_from(before),
                    after.capacity_released_from(before),
                ),
                _ => (false, false),
            };
        let compute_slot_released = after.active_work < before.active_work;
        let compute_advanced =
            Self::head_advanced(before.scheduler.resolve, after.scheduler.resolve)
                || Self::head_advanced(before.scheduler.verify_small, after.scheduler.verify_small)
                || Self::head_advanced(before.scheduler.verify_any, after.scheduler.verify_any)
                || (compute_slot_released
                    && (after.scheduler.resolve.is_some()
                        || after.scheduler.verify_small.is_some()
                        || after.scheduler.verify_any.is_some()));
        Self {
            compute_advanced,
            ready_advanced: Self::head_advanced(before.scheduler.ready, after.scheduler.ready),
            dependency_maintenance_activated: false,
            effect_publisher_advanced,
            effect_capacity_released,
            template_source_changed,
        }
    }

    fn effect(effect: super::effect::EffectWakeTransition) -> Self {
        Self {
            compute_advanced: false,
            ready_advanced: false,
            dependency_maintenance_activated: false,
            effect_publisher_advanced: effect.publisher_advanced(),
            effect_capacity_released: effect.capacity_released(),
            template_source_changed: false,
        }
    }

    fn with_effect_wake(mut self, effect: super::effect::EffectWakeTransition) -> Self {
        self.effect_publisher_advanced |= effect.publisher_advanced();
        self.effect_capacity_released |= effect.capacity_released();
        self
    }

    fn head_advanced(before: Option<EntryVersion>, after: Option<EntryVersion>) -> bool {
        after.is_some() && before != after
    }

    /// Publish one compute level when any compatible scheduler head changes or
    /// an active-work release may make a stable head newly eligible. The
    /// coordinator derives exact role assignments from the authoritative
    /// scheduler; this boolean is deliberately not a second routing policy.
    pub(super) fn compute_advanced(self) -> bool {
        self.compute_advanced
    }

    pub(super) fn ready_advanced(self) -> bool {
        self.ready_advanced
    }

    pub(super) fn dependency_maintenance_activated(self) -> bool {
        self.dependency_maintenance_activated
    }

    pub(super) fn effect_publisher_advanced(self) -> bool {
        self.effect_publisher_advanced
    }

    pub(super) fn effect_capacity_released(self) -> bool {
        self.effect_capacity_released
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
    resource_health: ResourceCommitHealth,
    wake: AuthorityWakeTransition,
}

struct ApplyRetirement {
    async_process_observations: AsyncProcessObservations,
    removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
    retired_effect: Option<Arc<EffectBatch>>,
    retired_generation: Option<RetiredGeneration>,
    dependency: Option<DependencyApplyOutcome>,
    template_source_changed: bool,
}

/// Dependency Apply outcome produced by the canonical owner-removal compiler.
/// Keeping it paired with retirement makes it impossible for administration
/// or pipeline reset to keep the owner mutation while discarding the only
/// maintenance wake fact for the same Apply.
struct OwnerRemovalCommit {
    retired: RetiredOwners,
    dependency: DependencyApplyOutcome,
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

fn finish_effect_only_apply(
    effect: super::effect::EffectWakeTransition,
    retirement: ApplyRetirement,
) -> CommittedDelta {
    finish_apply_retirement(retirement, AuthorityWakeTransition::effect(effect))
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
    finish_apply_retirement(retirement, AuthorityWakeTransition::between(before, after))
}

fn finish_apply_retirement(
    retirement: ApplyRetirement,
    mut wake: AuthorityWakeTransition,
) -> CommittedDelta {
    let ApplyRetirement {
        async_process_observations,
        removals,
        retired,
        retired_effect,
        retired_generation,
        dependency,
        template_source_changed,
    } = retirement;
    let dependency_maintenance_activated = match dependency {
        None | Some(DependencyApplyOutcome::Quiet) => false,
        Some(DependencyApplyOutcome::Activated) => true,
    };
    wake.dependency_maintenance_activated = dependency_maintenance_activated;
    wake.template_source_changed |= template_source_changed;
    CommittedDelta {
        async_process_observations,
        removals,
        retired,
        retired_effect,
        retired_generation,
        post_commit_fault: None,
        resource_health: ResourceCommitHealth::Healthy,
        wake,
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

    fn with_capacity(capacity: usize) -> Self {
        Self {
            first: None,
            rest: Vec::with_capacity(capacity.saturating_sub(1)),
        }
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
            resource_health,
            wake,
        } = self;
        drop(removals);
        drop(retired);
        drop(retired_effect);
        drop(retired_generation);
        let post_commit_fault = post_commit_fault.or(match resource_health {
            ResourceCommitHealth::Healthy => None,
            ResourceCommitHealth::Faulted => Some(AuthorityFault::ResourceProjection),
        });
        AuthorityPostCommit {
            async_process_observations,
            post_commit_fault,
            wake,
        }
    }

    /// Scratch-generation Applies are not externally published. Consume their
    /// retirement locally while retaining only whether dependency maintenance
    /// must be woken after the completed generation is swapped into service.
    fn into_scratch_dependency_wake(self) -> bool {
        let Self {
            async_process_observations: _,
            removals,
            retired,
            retired_effect,
            retired_generation,
            post_commit_fault: _,
            resource_health: _,
            wake,
        } = self;
        drop(removals);
        drop(retired);
        drop(retired_effect);
        drop(retired_generation);
        wake.dependency_maintenance_activated
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
    resource: ResourceBatchPlan,
    scheduler: SchedulerDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
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
        owner_cuts.reserve_exact(1);
        owner_cuts.push(IndependentOwnerCut {
            key,
            expected,
            removal_revision: None,
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
            resource: Some(resource),
            projection,
            scheduler,
            dependency,
            effect,
            async_process_starts: Vec::new(),
            removals: Vec::new(),
            retired,
        };
        let support = delta.physical_support(authority);
        Ok((delta, support))
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
        owner_cuts.reserve_exact(owner_count);
        for removal in &mut self.removals {
            let expected = self
                .projection
                .expected_accepted_version(removal.hash())
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            owner_cuts.push(IndependentOwnerCut {
                key: removal.hash().clone(),
                expected: OwnerPrestate::Accepted(expected),
                removal_revision: None,
                action: IndependentOwnerAction::Replace(removal.take_after()),
            });
        }
        let expected = self.changed_expected;
        let expected_is_witnessed = match expected {
            OwnerPrestate::Vacant => self.projection.expected_owner_vacant(&self.changed_key),
            OwnerPrestate::PreAccepted(expected) => {
                self.projection
                    .expected_preaccepted_version(&self.changed_key)
                    == Some(expected.version)
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
            removal_revision: None,
            action: IndependentOwnerAction::Replace(Some(self.changed_after)),
        });
        owner_cuts.sort_unstable_by(|left, right| left.key.cmp(&right.key));
        let scheduler = self.scheduler.into_shared_batch()?;
        let mut async_process_starts = Vec::new();
        if let Some(start) = self.async_process_start {
            async_process_starts.reserve_exact(1);
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
    removal_revision: Option<OwnerShardRemovalRevision>,
    action: IndependentOwnerAction,
}

impl IndependentOwnerCut {
    fn is_fresh(&self, entries: &ShardedOwnerMap, owners: &ShardedOwnerWriteCut<'_>) -> bool {
        let shard = entries.owner_shard(&self.key);
        self.expected.is_fresh(owners.owner(entries, &self.key))
            && self
                .removal_revision
                .is_none_or(|expected| owners.owner_removal_revision(shard) == expected)
    }
}

#[derive(Clone, Copy)]
enum OwnerPrestate {
    Vacant,
    PreAccepted(ExpectedPreAcceptedOwner),
    Accepted(EntryVersion),
    ReplacementHistory(EntryVersion),
}

impl OwnerPrestate {
    fn from_owner(owner: &OwnedTx) -> Self {
        match owner {
            OwnedTx::PreAccepted(entry) => Self::PreAccepted(ExpectedPreAcceptedOwner {
                version: entry.record.version,
                source: entry.source,
            }),
            OwnedTx::Accepted(entry) => Self::Accepted(entry.record.version),
            OwnedTx::ReplacementHistory(entry) => Self::ReplacementHistory(entry.record().version),
        }
    }

    fn is_fresh(self, current: Option<&OwnedTx>) -> bool {
        match self {
            Self::Vacant => current.is_none(),
            Self::PreAccepted(expected) => matches!(
                current,
                Some(OwnedTx::PreAccepted(entry))
                    if entry.record.version == expected.version && entry.source == expected.source
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

    fn version(self) -> Option<EntryVersion> {
        match self {
            Self::Vacant => None,
            Self::PreAccepted(expected) => Some(expected.version),
            Self::Accepted(version) | Self::ReplacementHistory(version) => Some(version),
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
    async_process_starts: Vec<AsyncProcessStart>,
    removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
}

impl IndependentDelta {
    #[cfg(test)]
    pub(in crate::authority) fn dependency_gate_support_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> super::shard::DependencyGateSupport {
        let mut support = self.dependency.dependency_gate_support(&authority.entries);
        support.include(self.projection.dependency_gate_support(&authority.entries));
        support
    }

    fn has_retained_owner_revisions(&self) -> bool {
        !self.owner_cuts.is_empty()
            && self
                .owner_cuts
                .iter()
                .all(|owner| owner.removal_revision.is_some())
    }

    fn is_shared_retained_owner_only_shape(&self, consumed_items: usize) -> bool {
        !self.owner_cuts.is_empty()
            && self.owner_cuts.len() <= consumed_items
            && self.has_retained_owner_revisions()
            && self.owner_cuts.iter().all(|owner| {
                let IndependentOwnerAction::Replace(Some(after)) = &owner.action else {
                    return false;
                };
                let OwnedTx::PreAccepted(entry) = after else {
                    return false;
                };
                matches!(
                    entry.source,
                    PreAcceptedSource::Remote(_) | PreAcceptedSource::Proposal { .. }
                ) && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                    && owner.expected.version().is_none_or(|before| {
                        before != entry.record.version
                            && matches!(entry.source, PreAcceptedSource::Proposal { .. })
                    })
            })
            && self.resource.is_some()
            && self.owners.template_sources.counts().changed() == (false, false)
            && self.effect.is_empty()
            && self.async_process_starts.is_empty()
            && self.removals.is_empty()
            && self.retired.is_empty()
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
        support
    }

    fn physical_support(&self, authority: &TxPoolAuthority) -> ShardApplySupport {
        let mut reads = if self.has_retained_owner_revisions() {
            ShardReadSupport::default()
        } else {
            self.dependency
                .shared_independent_final_read_support(&authority.entries)
        };
        reads.include(self.owners.indexes.sharded_read_support(&authority.entries));
        reads.include(self.projection.sharded_read_support(&authority.entries));
        for owner in &self.owner_cuts {
            if matches!(owner.action, IndependentOwnerAction::Observe) {
                reads.insert(authority.entries.owner_shard(&owner.key));
            }
        }
        ShardApplySupport::new(reads, self.physical_write_support(authority))
    }
}

#[cfg(test)]
struct EffectOnlyDelta {
    effect: EffectDelta,
}

struct FreshGeneration {
    entries: ShardedOwnerMap,
    resources: ResourceLedger,
    scheduler: Arc<Mutex<FairFrontier>>,
    dependencies: DependencyFrontier,
    dependency_maintenance_activated: bool,
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
        let dependencies = DependencyFrontier::for_entries(&entries);
        Self {
            entries,
            resources: ResourceLedger::new(resources.limits()),
            scheduler: Arc::new(Mutex::new(FairFrontier::new(
                scheduler.lock().verify_order(),
            ))),
            dependencies,
            dependency_maintenance_activated: false,
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
    compute_slot_released: bool,
}

struct ClearPipelineDelta {
    generation: PoolGeneration,
    removal: OwnerRemovalBatch,
    effect: EffectDelta,
}

#[cfg(test)]
struct AdminDelta {
    marker: PeerBanDelta,
    removal: OwnerRemovalBatch,
    effect: EffectDelta,
}

/// Unique owner-removal input whose caller-selected order is preserved.
/// Duplicate rejection happens once before cause-specific effects or derived
/// deltas are compiled; truncation of a prefix preserves the proof.
struct OwnerRemovalKeys(Vec<RawTxHash>);

impl OwnerRemovalKeys {
    fn new(hashes: Vec<RawTxHash>) -> Result<Self, PlanError> {
        let mut unique = HashSet::new();
        unique.reserve(hashes.len());
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
/// preparation has not begun. Compile owns only bounded deltas and exact
/// clocks; Bind stages scheduler/effect rows and prepares dependency gates;
/// Apply consumes the final physical cut with no recoverable branch after its
/// first write.
#[must_use = "a compiled shared owner removal must bind to its exact live cut or be discarded"]
pub(super) struct CompiledSharedOwnerRemoval<C> {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    removal: OwnerRemovalBatch,
    publication: Option<EffectPublication>,
    sequence: ApplySequence,
    control: C,
}

#[must_use = "a bound shared owner removal must commit its exact cut or roll back every hidden row"]
pub(super) struct PreparedSharedOwnerRemoval<'authority, C> {
    authority: &'authority TxPoolAuthority,
    removal: OwnerRemovalBatch,
    projections: ingress::StagedRetainedIngress<'authority>,
    staged_effect: Option<super::effect::StagedEffect>,
    control: C,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedBackingParent {
    hash: RawTxHash,
    version: EntryVersion,
}

/// Cause-neutral evidence for removing one Accepted administrative closure.
/// Every caller shares the same closure, owner, backing-parent and projection
/// freshness proof; a cause may add evidence but cannot replace this carrier.
pub(super) struct AdministrativeRemovalControl {
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
pub(super) struct AcceptedExpiryControl {
    head: AcceptedExpiryHead,
    administrative: AdministrativeRemovalControl,
}

/// Cause aliases preserve the static API vocabulary without duplicating the
/// compile/bind/apply lifecycle. Remote plans still own no live gate, so an
/// earlier due insertion can commit before Bind and invalidate their witness.
pub(super) type CompiledSharedRemoteExpiry = CompiledSharedOwnerRemoval<RemoteExpiryWitness>;
pub(super) type CompiledSharedAcceptedExpiry = CompiledSharedOwnerRemoval<AcceptedExpiryControl>;
pub(super) type CompiledSharedLocalRemoval =
    CompiledSharedOwnerRemoval<AdministrativeRemovalControl>;

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
}

#[cfg_attr(
    any(test, feature = "internal"),
    expect(
        clippy::large_enum_variant,
        reason = "the prepared authority transition stays allocation-free after Plan; boxing a large arm would move a fixed semantic delta behind an infallible allocation"
    )
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
    #[cfg(test)]
    Effect(EffectOnlyDelta),
    ClearPool(Box<ClearPoolDelta>),
}

impl PlainAuthorityDelta {
    fn releases_preaccepted_active_work(&self) -> bool {
        match self {
            #[cfg(test)]
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

enum ReleasedInputBacking {
    Unavailable,
    Chain,
    Pool(AcceptedBackingParent),
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
    prepared: PreparedDependencyBatch,
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
    before: Option<AuthorityWakeProjection>,
    compute_slot_released: bool,
    resource_health: ResourceCommitHealth,
    retirement: ApplyRetirement,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns preallocated retirement storage after irreversible owner mutation; boxing here would add a fallible post-commit allocation"
)]
pub(in crate::authority) enum ReadyJobCommitOutcome {
    Committed(CommittedDelta),
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
    Committed(CommittedDelta),
    Stale {
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
    Committed(CommittedDelta),
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
    Accepted(CommittedDelta),
    Duplicate {
        key: RawTxHash,
        committed: CommittedDelta,
    },
    Rejected {
        reason: MembershipReject,
        committed: CommittedDelta,
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
        committed: CommittedDelta,
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
        } = self;
        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
        );
        let read_fence = match read_witness.bind(authority) {
            Ok(fence) => fence,
            Err(_) => {
                return Self::rollback(staged_effect);
            }
        };
        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionReadCutBeforeActivation,
        );
        let effect_wake = staged_effect.activate_with_wake();
        drop(read_fence);
        let retirement = ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect: None,
            retired_generation: None,
            dependency: None,
            template_source_changed: false,
        };
        let committed = finish_effect_only_apply(effect_wake, retirement);
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
        } = self;
        #[cfg(test)]
        authority.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::DirectRejectionEffectStagedBeforeReadCut,
        );

        let read_fence = match read_witness.bind(authority) {
            Ok(fence) => fence,
            Err(stale) => {
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
        let retirement = ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect: None,
            retired_generation: None,
            dependency: None,
            template_source_changed: false,
        };
        let committed = finish_effect_only_apply(effect_wake, retirement);
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
    Committed(CommittedDelta),
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

    pub(in crate::authority) fn authority_cut_is_current(
        &self,
        authority: &TxPoolAuthority,
    ) -> bool {
        authority.generation == self.generation && authority.chain_view == self.chain_view
    }

    pub(in crate::authority) fn commit_ready_job(
        self,
        authority: &TxPoolAuthority,
        reservation: ReadySlotReservation,
    ) -> ReadyJobCommitOutcome {
        if !self.authority_cut_is_current(authority) {
            return match self.cancel_ready_job(reservation) {
                Ok(wake) => ReadyJobCommitOutcome::Stale(wake),
                Err(fault) => ReadyJobCommitOutcome::Fault {
                    fault,
                    effect_wake: None,
                },
            };
        }
        let Self {
            generation: _,
            chain_view: _,
            delta,
            support,
            staged_effect,
        } = self;
        let mut reservation = reservation;
        match apply_seal::commit_ready_job_rows(authority, delta, support, &mut reservation) {
            Ok(committed) => {
                let effect_wake = staged_effect.activate_with_wake();
                ReadyJobCommitOutcome::Committed(committed.finish(authority, Some(effect_wake)))
            }
            Err(error) => {
                let effect_wake = staged_effect.rollback_with_wake().ok();
                drop(reservation);
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
        reservation: ReadySlotReservation,
    ) -> ReadyHeadCommitOutcome {
        match self.commit_ready_job(authority, reservation) {
            ReadyJobCommitOutcome::Committed(committed) => {
                ReadyHeadCommitOutcome::Committed(committed)
            }
            ReadyJobCommitOutcome::Stale(effect_wake) => ReadyHeadCommitOutcome::Stale {
                effect_wake: Some(effect_wake),
            },
            ReadyJobCommitOutcome::Fault { fault, effect_wake } => {
                ReadyHeadCommitOutcome::Fault { fault, effect_wake }
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

    /// Cancel one compiled Ready job before owner mutation. The staged effect
    /// is terminalized before the reservation can expose Ready again, and the
    /// exact wake edge is returned to the runtime for publication. This
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
        let result = staged_effect
            .rollback_with_wake()
            .map_err(|_| AuthorityFault::EffectProjection);
        drop(reservation);
        result
    }
}

impl CompiledSharedReadyReresolution {
    fn commit(
        self,
        authority: &TxPoolAuthority,
        reservation: ReadySlotReservation,
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
        let mut reservation = reservation;
        match apply_seal::commit_ready_job_rows(authority, delta, support, &mut reservation) {
            Ok(committed) => ReadyHeadCommitOutcome::Committed(committed.finish(authority, None)),
            Err(error) => {
                drop(reservation);
                match error {
                    ConcurrentIndependentError::ChangedCut(_) => {
                        ReadyHeadCommitOutcome::Stale { effect_wake: None }
                    }
                    ConcurrentIndependentError::Fault(fault) => ReadyHeadCommitOutcome::Fault {
                        fault,
                        effect_wake: None,
                    },
                }
            }
        }
    }
}

impl PreparedSharedReadyHeadDisposition<'_> {
    pub(in crate::authority) fn commit(
        self,
        reservation: ReadySlotReservation,
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
                let outcome = match apply_seal::commit_shared_owner_removal(core) {
                    Ok(committed) => ReadyHeadCommitOutcome::Committed(committed),
                    Err(failure) => {
                        let (error, effect_wake) = failure.into_parts();
                        match error {
                            ingress::ConcurrentRetainedIngressError::Stale => {
                                ReadyHeadCommitOutcome::Stale { effect_wake }
                            }
                            ingress::ConcurrentRetainedIngressError::Fault(fault) => {
                                ReadyHeadCommitOutcome::Fault { fault, effect_wake }
                            }
                        }
                    }
                };
                drop(reservation);
                outcome
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
    ) -> CommittedDelta {
        let committed = match self.before {
            Some(before) => {
                let after = authority.wake_projection_without_effect();
                finish_apply_between(
                    authority,
                    before,
                    after,
                    self.compute_slot_released,
                    false,
                    self.retirement,
                )
            }
            None => finish_apply_retirement(self.retirement, AuthorityWakeTransition::default()),
        };
        let committed = match effect_wake {
            Some(wake) => committed.with_effect_wake(wake),
            None => committed,
        };
        committed.with_resource_health(self.resource_health)
    }
}

impl CommittedDelta {
    fn with_resource_health(mut self, resource_health: ResourceCommitHealth) -> Self {
        self.resource_health = resource_health;
        self
    }

    pub(in crate::authority) fn into_parts(mut self) -> (Self, Option<AuthorityFault>) {
        let post_commit_fault =
            match std::mem::replace(&mut self.resource_health, ResourceCommitHealth::Healthy) {
                ResourceCommitHealth::Healthy => None,
                ResourceCommitHealth::Faulted => Some(AuthorityFault::ResourceProjection),
            };
        (self, post_commit_fault)
    }
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

impl PreparedApply<'_> {
    fn prepare(
        authority: &mut TxPoolAuthority,
        mut delta: DependencyAuthorityDelta,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let dependency = delta
            .take_dependency()
            .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let prepared = PreparedDependencyBatch::prepare_primary_replacements(
            &authority.dependencies,
            dependency,
        )?;
        Ok(PreparedApply {
            authority,
            kind: PreparedApplyKind::Dependency(Box::new(PreparedDependencyApply {
                delta,
                prepared,
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
                let PreparedDependencyApply { delta, prepared } = *prepared;
                match delta {
                    DependencyAuthorityDelta::Entry(delta) => {
                        Self::apply_entry(&mut *authority, token, delta, prepared)
                    }
                    #[cfg(any(test, feature = "internal"))]
                    DependencyAuthorityDelta::Membership(delta) => {
                        Self::apply_membership(&mut *authority, token, delta, prepared)
                    }
                    DependencyAuthorityDelta::ClearPipeline(delta) => {
                        Self::apply_clear_pipeline(&mut *authority, token, delta, prepared)
                    }
                    #[cfg(test)]
                    DependencyAuthorityDelta::Admin(delta) => {
                        Self::apply_admin(&mut *authority, token, delta, prepared)
                    }
                    DependencyAuthorityDelta::Chain(delta) => {
                        Self::apply_chain(&mut *authority, token, delta, prepared)
                    }
                }
            }
            #[cfg(test)]
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
        dependency: PreparedDependencyBatch,
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
        let dependency = dependency.apply_exclusive();
        let retired_effect = authority.effects.lock().apply(delta.effect);
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
        dependency: PreparedDependencyBatch,
    ) -> ApplyRetirement {
        let mut retired = delta.retired;
        let proposed_counts = delta.projection.take_proposed_counts();
        let support = authority.entries.owner_resource_write_support(
            delta
                .removals
                .iter()
                .map(MembershipRemoval::hash)
                .chain(std::iter::once(&delta.changed_key)),
            &proposed_counts,
            delta.resource.shard_plan(),
        );
        let removal_updates = delta
            .removals
            .iter_mut()
            .map(|removal| OwnerResourceUpdate::new(removal.hash().clone(), removal.take_after()));
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
        let dependency = dependency.apply_exclusive();
        let retired_effect = authority.effects.lock().apply(delta.effect);
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
    pub(in crate::authority) fn apply(self) -> Result<CommittedDelta, ConcurrentIndependentError> {
        apply_seal::commit_independent(self)
    }

    fn apply_with(self, token: &ApplyToken) -> Result<CommittedDelta, ConcurrentIndependentError> {
        match self {
            Self::Shared {
                authority,
                delta,
                support,
                staged_effect,
            } => Self::apply_shared(authority, token, delta, support, staged_effect, None),
        }
    }

    fn apply_shared(
        authority: &TxPoolAuthority,
        token: &ApplyToken,
        delta: IndependentDelta,
        support: ShardApplySupport,
        staged_effect: Option<super::effect::StagedEffect>,
        reservation: Option<&mut ReadySlotReservation>,
    ) -> Result<CommittedDelta, ConcurrentIndependentError> {
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
        reservation: Option<&mut ReadySlotReservation>,
    ) -> Result<ReadyCommittedRows, ConcurrentIndependentError> {
        let compute_slot_released = delta
            .resource
            .as_ref()
            .is_some_and(ResourceBatchPlan::releases_preaccepted_active_work);
        let scheduler_unchanged = reservation.is_none() && delta.scheduler.is_empty();
        let before = match reservation.as_ref() {
            None if scheduler_unchanged && !compute_slot_released => None,
            Some(reservation) => {
                let scheduler = reservation.scheduler_wake_before().map_err(|_| {
                    ConcurrentIndependentError::Fault(AuthorityFault::SchedulerProjection)
                })?;
                Some(authority.wake_projection_with_scheduler_without_effect(scheduler))
            }
            None => Some(authority.wake_projection_without_effect()),
        };
        let proposed_counts = delta.projection.take_proposed_counts();
        let mut retired = delta.retired;
        let DerivedOwnerDelta {
            indexes,
            sources,
            template_sources,
        } = delta.owners;
        let template_source_changed = template_sources.counts().changed();
        let (dependency, resource_health, staged_before) = authority
            .commit_shared_independent_rows(
                token,
                delta.owner_cuts,
                delta.resource,
                proposed_counts,
                support,
                indexes,
                delta.projection,
                delta.dependency,
                sources,
                template_sources.counts(),
                delta.scheduler,
                reservation,
                &mut retired,
            )?;
        let before = staged_before.or(before);
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
        })
    }
}

impl PreparedApply<'_> {
    #[cfg(test)]
    fn apply_effect(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: EffectOnlyDelta,
    ) -> ApplyRetirement {
        let authority = authority.write(token);
        let retired_effect = authority.effects.lock().apply(delta.effect);
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
            dependency_maintenance_activated,
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
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect,
            retired_generation: Some(retired_generation),
            dependency: dependency_maintenance_activated
                .then_some(DependencyApplyOutcome::Activated),
            template_source_changed: true,
        }
    }

    fn apply_clear_pipeline(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: ClearPipelineDelta,
        dependency: PreparedDependencyBatch,
    ) -> ApplyRetirement {
        let template_source_changed = delta.removal.owners.template_sources.counts().changed();
        let OwnerRemovalCommit {
            retired,
            dependency,
        } = Self::apply_owner_removal(authority, token, delta.removal, dependency);
        let authority = authority.write(token);
        authority.generation = delta.generation;
        let retired_effect = authority.effects.lock().apply(delta.effect);
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
        dependency: PreparedDependencyBatch,
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
        dependency: PreparedDependencyBatch,
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
        let dependency = dependency.apply_exclusive();
        OwnerRemovalCommit {
            retired,
            dependency,
        }
    }

    fn apply_chain(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        mut delta: ChainDelta,
        dependency: PreparedDependencyBatch,
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
        let dependency = dependency.apply_exclusive();
        let retired_effect = authority.effects.lock().apply(delta.effect);
        authority.chain_view = delta.view.clone();
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
    owner_baseline: OwnerClockProgress,
}

impl ClockPlanReservation {
    pub(in crate::authority) fn begin(bank: Arc<AuthorityClockBank>) -> Self {
        let owner_baseline = bank.snapshot().owner_progress();
        Self {
            bank,
            owner_baseline,
        }
    }

    pub(in crate::authority) fn commit(
        self,
    ) -> Result<ApplyClockReservation, ClockReservationError> {
        let sequence = self
            .bank
            .reserve_sequence()
            .map_err(|_| ClockReservationError)?;
        Ok(ApplyClockReservation {
            sequence,
            plan: self,
        })
    }

    pub(in crate::authority) fn replacement(
        &mut self,
    ) -> Result<EntryVersion, ClockReservationError> {
        self.bank
            .reserve_replacement()
            .map_err(|_| ClockReservationError)
    }

    pub(in crate::authority) fn insertion(
        &mut self,
    ) -> Result<(EntryVersion, Arrival), ClockReservationError> {
        self.bank
            .reserve_insertion()
            .map_err(|_| ClockReservationError)
    }

    pub(in crate::authority) fn replacements(
        &mut self,
        members: NonZeroUsize,
    ) -> Result<impl Iterator<Item = EntryVersion> + use<>, ClockReservationError> {
        self.bank
            .reserve_replacements(members)
            .map(|versions| versions.map(EntryVersion))
            .map_err(|_| ClockReservationError)
    }

    pub(in crate::authority) fn adopt_owner_progress(
        &self,
        progress: OwnerClockProgress,
    ) -> Result<(), ClockReservationError> {
        let version_advance = progress
            .next_version
            .0
            .checked_sub(self.owner_baseline.next_version.0)
            .ok_or(ClockReservationError)?;
        let arrival_advance = progress
            .next_arrival
            .0
            .checked_sub(self.owner_baseline.next_arrival.0)
            .ok_or(ClockReservationError)?;
        if arrival_advance > version_advance {
            return Err(ClockReservationError);
        }
        self.bank.adopt_owner_progress(progress);
        Ok(())
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

    pub(in crate::authority) const fn sequence(&self) -> ApplySequence {
        self.sequence
    }

    pub(in crate::authority) fn replacement(
        &mut self,
    ) -> Result<EntryVersion, ClockReservationError> {
        self.plan.replacement()
    }

    pub(in crate::authority) fn insertion(
        &mut self,
    ) -> Result<(EntryVersion, Arrival), ClockReservationError> {
        self.plan.insertion()
    }

    pub(in crate::authority) fn adopt_owner_progress(
        &self,
        progress: OwnerClockProgress,
    ) -> Result<(), ClockReservationError> {
        self.plan.adopt_owner_progress(progress)
    }
}

fn retired_buffer(capacity: usize) -> RetiredOwners {
    RetiredOwners::with_capacity(capacity)
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
                        DependencyKey::Cell(out_point) => {
                            let producer = self.entries.get(&RawTxHash(out_point.tx_hash()));
                            let index: u32 = out_point.index().unpack();
                            let can_wait_for_preaccepted_output = matches!(
                                producer.as_deref(),
                                Some(OwnedTx::PreAccepted(parent))
                                    if usize::try_from(index).ok().is_some_and(|index| {
                                        index < parent.record.tx.outputs().len()
                                    })
                            );
                            (!can_wait_for_preaccepted_output)
                                .then(|| Reject::Resolve(OutPointError::Unknown(out_point.clone())))
                        }
                        DependencyKey::Header(hash) => {
                            Some(Reject::Resolve(OutPointError::InvalidHeader(hash.clone())))
                        }
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
        let released = match after {
            OwnedTx::Accepted(candidate) => {
                self.collect_released_replacement_inputs(candidate, removals)?
            }
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => Vec::new(),
        };
        #[cfg(test)]
        self.entries.enter_membership_dependency_plan_probe();

        let mut changes = Vec::with_capacity(
            removals
                .len()
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
        );
        changes.push((existing, Some(after)));
        for removal in removals {
            changes.push((Some(removal.before()), removal.after()));
        }
        let loss =
            self.collect_dependency_loss_keys(removals.iter().map(MembershipRemoval::before))?;
        let lost = loss.keys;
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
        available.extend(released);
        let control = self
            .dependencies
            .plan_shared_events(available, lost, DependencyCut(sequence))?
            .unwrap_or_default();
        Ok(self.dependencies.compile_membership_replacements(
            changes,
            existing.is_none(),
            control,
        )?)
    }

    fn plan_direct_absent_dependency_delta(
        &self,
        after: &OwnedTx,
        sequence: ApplySequence,
    ) -> Result<Option<DependencyBatchDelta>, PlanError> {
        let record = after.record();
        let available = record
            .tx
            .output_pts()
            .into_iter()
            .map(DependencyKey::Cell)
            .collect();
        let control = self
            .dependencies
            .plan_events(available, Vec::new(), DependencyCut(sequence))?
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
        let mut removed = HashSet::new();
        removed.reserve(removals.len());
        removed.extend(removals.iter().map(|removal| removal.hash().clone()));

        let candidate_footprint = &candidate.proof.payload().footprint;
        let mut candidate_inputs = HashSet::new();
        candidate_inputs.reserve(candidate_footprint.inputs().len());
        candidate_inputs.extend(candidate_footprint.inputs().iter().cloned());
        let capacity = removals.iter().try_fold(0usize, |total, removal| {
            let OwnedTx::Accepted(victim) = removal.before() else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            total
                .checked_add(victim.proof.payload().footprint.inputs().len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
        })?;
        let mut available = Vec::with_capacity(capacity);

        for removal in removals {
            let victim = match removal.before() {
                OwnedTx::Accepted(entry) => entry,
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            for input in victim.proof.payload().footprint.inputs() {
                if !candidate_inputs.contains(input)
                    && (victim.proof.is_chain_input(input)
                        || !removed.contains(&RawTxHash(input.tx_hash())))
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
        snapshots.reserve_exact(removals.len());
        for hash in removals.iter() {
            let entry = match owners.get(hash) {
                Some(OwnedTx::Accepted(entry)) => Ok((hash, entry)),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    Err(PlanError::Fault(AuthorityFault::MembershipProjection))
                }
            }?;
            snapshots.push(entry);
        }
        self.collect_released_administrative_inputs_from(snapshots, removals, Some(&owners))
            .map(|released| released.keys)
    }

    fn collect_released_administrative_inputs_from<'entry>(
        &self,
        snapshots: impl IntoIterator<Item = (&'entry RawTxHash, &'entry AcceptedEntry)>,
        removals: &AcceptedRemovalSet,
        owners: Option<&ShardedOwnerReadCut<'_>>,
    ) -> Result<AdministrativeReleasedInputs, PlanError> {
        let snapshots = snapshots.into_iter();
        let mut available = Vec::new();
        let mut parents = Vec::new();
        if let Some(capacity) = snapshots.size_hint().1 {
            available.reserve(capacity);
            parents.reserve(capacity);
        }
        for (hash, entry) in snapshots {
            available.reserve(entry.proof.payload().footprint.inputs().len());
            parents.reserve(entry.proof.payload().footprint.inputs().len());
            for input in entry.proof.payload().footprint.inputs() {
                match self.released_input_backing_in_final_owner_set(
                    entry, input, removals, hash, owners,
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
        removals: &AcceptedRemovalSet,
        victim: &RawTxHash,
        owners: Option<&ShardedOwnerReadCut<'_>>,
    ) -> Result<ReleasedInputBacking, PlanError> {
        let spender = match owners {
            Some(owners) => owners.membership_spender(input).cloned(),
            None => self.membership.spender(input),
        };
        if spender.as_ref() != Some(victim) {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        if removed_entry.proof.is_chain_input(input) {
            return Ok(ReleasedInputBacking::Chain);
        }
        let parent = RawTxHash(input.tx_hash());
        if removals.contains(&parent) {
            return Ok(ReleasedInputBacking::Unavailable);
        }
        let index: u32 = input.index().unpack();
        let accepted_parent = |parent: &AcceptedEntry| {
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
            }
        };
        if let Some(owners) = owners {
            return Ok(match owners.get(&parent) {
                Some(OwnedTx::Accepted(parent)) => accepted_parent(parent),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    ReleasedInputBacking::Unavailable
                }
            });
        }
        let owner = self.entries.get(&parent);
        Ok(match owner.as_deref() {
            Some(OwnedTx::Accepted(parent)) => accepted_parent(parent),
            Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                ReleasedInputBacking::Unavailable
            }
        })
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
        replacement: (&RawTxHash, Option<&OwnedTx>, &OwnedTx),
        removals: &[MembershipRemoval],
        sequence: ApplySequence,
    ) -> Result<DerivedOwnerDelta, PlanError> {
        let (key, existing, after) = replacement;
        let change_capacity = removals
            .len()
            .checked_add(1)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut changes = Vec::with_capacity(change_capacity);
        changes.push((key, existing, Some(after)));
        for removal in removals {
            changes.push((removal.hash(), Some(removal.before()), removal.after()));
        }
        let indexes = self
            .indexes_for_plan()
            .plan_replacements(changes.iter().copied())?;
        let sources = AuthoritySourceVersions::plan_template_selection_replacements(
            changes.iter().map(|(_, before, after)| (*before, *after)),
            sequence,
        );
        let template_sources = self.plan_owner_sources(changes.iter().copied())?;
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
    /// cohort. Structural failures remain explicit faults; the later resource
    /// projection decides whether the complete cohort is retainable.
    fn retain_replacement_history(
        &self,
        candidate: &AcceptedEntry,
        removals: &mut [MembershipRemoval],
        sequence: ApplySequence,
    ) -> Result<(), PlanError> {
        if !removals
            .iter()
            .any(|removal| removal.cause == RemovalCause::Replacement)
        {
            return Ok(());
        }
        let mut removed = HashSet::new();
        removed.reserve(removals.len());
        removed.extend(removals.iter().map(|removal| removal.hash().clone()));

        // ExpandedFootprint canonicalizes inputs into sorted unique order, so
        // RBF-only trigger derivation needs no second candidate-input index.
        let candidate_inputs = candidate.proof.payload().footprint.inputs();
        for removal in removals.iter_mut() {
            if removal.cause != RemovalCause::Replacement {
                continue;
            }
            let accepted = match removal.before() {
                OwnedTx::Accepted(entry) => entry,
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            let footprint = &accepted.proof.payload().footprint;
            let trigger_capacity = footprint
                .inputs()
                .len()
                .checked_add(footprint.dependencies().len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let mut trigger_keys = Vec::with_capacity(trigger_capacity);
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
                Err(ReplacementHistoryError::InvalidRecoveryTrigger) => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                Err(ReplacementHistoryError::ResourceArithmetic) => {
                    return Err(PlanError::Fault(AuthorityFault::CounterExhausted));
                }
            };
            removal.retain_replacement_history(history)?;
        }
        Ok(())
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
        let mut changes = Vec::with_capacity(capacity);
        changes.push((
            key.clone(),
            before.map(OwnedTx::charge_record),
            Some(after.charge_record()),
        ));
        for removal in removals {
            changes.push((
                removal.hash().clone(),
                Some(removal.before().charge_record()),
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
        changes.reserve_exact(capacity);
        changes.push((
            key.clone(),
            before.map(OwnedTx::charge_record),
            Some(after.charge_record()),
        ));
        for removal in removals {
            changes.push((
                removal.hash().clone(),
                Some(removal.before().charge_record()),
                removal.after().map(OwnedTx::charge_record),
            ));
        }
        self.resources_for_plan().plan_batch(changes)
    }

    fn membership_resource_error(error: ResourceError) -> PlanError {
        match error {
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

    /// Shared membership resource planning follows an earlier optimistic policy
    /// cut, so a changed owner charge makes that cut stale. Every other resource
    /// error retains its original classification.
    fn optimistic_membership_resource_error(error: ResourceError) -> PlanError {
        match error {
            ResourceError::ExistingChargeMismatch => {
                PlanError::Stale(StalePlan::AcceptedObservation)
            }
            error => Self::membership_resource_error(error),
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
        Self::collect_dependency_loss_keys_with(parents)
    }

    fn collect_dependency_loss_keys_with<'entry>(
        parents: impl IntoIterator<Item = &'entry OwnedTx>,
    ) -> Result<DependencyLossKeys, PlanError> {
        let mut keys = Vec::new();
        for parent in parents {
            let record = parent.record();
            let output_count = record.tx.data().raw().outputs().len();
            keys.reserve(output_count);
            keys.extend(record.tx.output_pts().into_iter().map(DependencyKey::Cell));
        }
        Ok(DependencyLossKeys { keys })
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

        self.reserve_primary_owner_insertions(std::iter::once(&key));
        let planned_charge = ChargeRecord::PreAccepted {
            resources: charge,
            residency_peer: admission.source.ingress_peer(),
            compute_peer: None,
        };
        let resource =
            self.resources_for_plan()
                .plan_replace(key.clone(), None, Some(planned_charge))?;

        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, arrival) = clocks.insertion()?;
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
            sequence,
            TransitionControls::none(),
            Some(resource),
        )
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

        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let promoted = if same_witness {
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
            promoted
        } else {
            let version = clocks.replacement()?;
            PreAcceptedEntry {
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
            }
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
        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let version = clocks.replacement()?;
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
            sequence,
        )
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
                let mut clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
                let sequence = clocks.sequence();
                let version = clocks.replacement()?;
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

        let mut clocks = ApplyClockReservation::begin(Arc::clone(&self.clocks))?;
        let (version, arrival) = match &existing {
            Some(owner) => {
                let version = clocks.replacement()?;
                (version, owner.record().arrival)
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
                    insertions.reserve_exact(1);
                    insertions.push((key.clone(), accepted.charge_record()));
                    Some(
                        self.resources_for_plan()
                            .plan_direct_accepted_insertion_batch(insertions)
                            .map_err(|error| match error {
                                DirectAcceptedInsertionError::Contended(wait) => {
                                    PlanError::ResourceContended(wait)
                                }
                                DirectAcceptedInsertionError::Resource(error) => {
                                    Self::optimistic_membership_resource_error(error)
                                }
                            })?,
                    )
                } else {
                    None
                };
                let (dependency, scheduler, owners) = if vacant_leaf {
                    let after = OwnedTx::Accepted(accepted.clone());
                    match self.plan_direct_absent_dependency_delta(&after, clocks.sequence())? {
                        Some(dependency) => (
                            Some(dependency),
                            Some(SchedulerDelta::shared_absent_accepted()),
                            Some(self.plan_direct_absent_owner_derivations(
                                &key,
                                &after,
                                clocks.sequence(),
                            )?),
                        ),
                        None => (None, None, None),
                    }
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

        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
            .map_err(PlanError::from)?;
        let (version, arrival) = clocks.insertion().map_err(PlanError::from)?;
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
        Ok(InternalPlugDisposition::Insert(PreparedApply::prepare(
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

    #[cfg(test)]
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
        } = self.evaluate_preaccepted_candidate(receipt)?;
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
        let outcome = self.evaluate_preaccepted_membership_policy(&key, preaccepted, &accepted)?;
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
        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let version = clocks.replacement()?;
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
        } = self.evaluate_preaccepted_candidate(receipt)?;
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
            self.reserve_primary_owner_insertions(std::iter::once(&key));
        }
        let PreparedMembership {
            mut removals,
            projection,
        } = prepared;
        let sequence = clocks.sequence();
        self.retain_replacement_history(&accepted, &mut removals, sequence)?;
        let mut retained_history = true;

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
                #[cfg(test)]
                if sparse_resource {
                    self.entries.enter_shared_ingress_probe(
                        super::shard::SharedIngressProbePhase::DirectMembershipBeforeResourcePlan,
                    );
                }
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
                            Err(error) => {
                                return Err(if sparse_resource {
                                    Self::optimistic_membership_resource_error(error)
                                } else {
                                    Self::membership_resource_error(error)
                                });
                            }
                        }
                    }
                    Err(error) => {
                        return Err(if sparse_resource {
                            Self::optimistic_membership_resource_error(error)
                        } else {
                            Self::membership_resource_error(error)
                        });
                    }
                }
            }
        };
        if retained_history {
            for removal in removals
                .iter_mut()
                .filter(|removal| removal.after().is_some())
            {
                let (version, arrival) = clocks.insertion()?;
                removal.assign_replacement_history_identity(version, arrival)?;
            }
        }
        let retirement_capacity = removals
            .len()
            .checked_add(usize::from(existing.is_some()))
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let retired = retired_buffer(retirement_capacity);
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
        let (owners, dependency) = match (prepared_owners, prepared_dependency, existing.as_ref()) {
            (Some(owners), Some(dependency), None) if removals.is_empty() => (owners, dependency),
            (None, None, existing) => {
                let owners = self.plan_membership_owner_derivations(
                    (&key, existing, &after),
                    &removals,
                    sequence,
                )?;
                let dependency =
                    self.plan_membership_dependency_delta(existing, &after, &removals, sequence)?;
                (owners, dependency)
            }
            (Some(_), _, _) | (_, Some(_), _) => {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
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
        let mut effects = Vec::with_capacity(effect_count);
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
            let OwnedTx::Accepted(removed) = removal.before() else {
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
        let mut hashes = Vec::with_capacity(self.entries.len());
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
        PreparedApply::prepare(
            self,
            DependencyAuthorityDelta::ClearPipeline(ClearPipelineDelta {
                generation,
                removal,
                effect,
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

        let effects = vec![CommittedEffect::PeerCohortRevoked(revocation)];
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
        })
    }

    fn plan_owner_removal_batch(
        &self,
        hashes: OwnerRemovalKeys,
        sequence: ApplySequence,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        let mut accepted_removals = Vec::new();
        accepted_removals.reserve_exact(hashes.len());
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
        let mut owner_snapshots = Vec::with_capacity(hashes.len());
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
        let mut resource_changes = Vec::with_capacity(hashes.len());
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
        let loss = Self::collect_dependency_loss_keys_with(owner_snapshots.iter())?;
        let dependency_control = dependencies_frontier
            .plan_events(available, loss.keys, DependencyCut(sequence))?
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
        let retired = retired_buffer(hashes.len());
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

    fn capture_accepted_administrative_removal(
        &self,
        root: &RawTxHash,
    ) -> Result<AcceptedAdministrativeRemoval, PlanError> {
        let closure = self.administrative_descendant_closure_witness(root)?;
        let (hashes, closure_witness) = closure.into_parts();

        let mut removal_set = Vec::new();
        removal_set.reserve_exact(hashes.len());
        removal_set.extend(hashes.iter().cloned());
        let accepted_removals = AcceptedRemovalSet::try_from_vec(removal_set)?;

        let mut owners = Vec::new();
        owners.reserve_exact(hashes.len());
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
            accepted.reserve_exact(owners.len());
            for (hash, owner) in hashes.iter().zip(&owners) {
                let OwnedTx::Accepted(entry) = owner else {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                };
                accepted.push((hash, entry));
            }
            self.collect_released_administrative_inputs_from(accepted, &accepted_removals, None)
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
                hashes.reserve_exact(1);
                owners.reserve_exact(1);
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
        Ok(Some(CompiledSharedOwnerRemoval {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal,
            publication,
            sequence,
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
        effects.reserve_exact(captured.snapshots.owners.len());
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
        Ok(Some(CompiledSharedOwnerRemoval {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal,
            publication: Some(publication),
            sequence,
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
        effects.reserve_exact(witness.len());
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
        hashes.reserve_exact(witness.len());
        let mut owners = Vec::new();
        owners.reserve_exact(witness.len());
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
        Ok(Some(CompiledSharedOwnerRemoval {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal,
            publication: Some(publication),
            sequence,
            control: witness,
        }))
    }

    pub(super) fn effect_publication_observation(&self) -> EffectPublicationObservation {
        self.effects.lock().publication_observation()
    }

    pub(super) fn apply_effect_settlement(
        &self,
        receipt: EffectReceipt,
    ) -> Result<(EffectSettlementApplied, EffectPublicationObservation), EffectSettlementFailure>
    {
        let mut effects = self.effects.lock();
        let before = effects.wake_projection();
        let retired_effect = match effects.settle(&receipt) {
            Ok(retired) => retired,
            Err(error) => {
                return Err(EffectSettlementFailure { error, receipt });
            }
        };
        let after = effects.wake_projection();
        let next = effects.publication_observation();
        drop(effects);
        Ok((
            EffectSettlementApplied {
                retired_effect,
                wake: super::effect::EffectWakeTransition::between(before, after),
            },
            next,
        ))
    }

    pub(super) fn close_effects(
        &mut self,
    ) -> Result<super::effect::EffectWakeTransition, EffectCloseError> {
        if self.resources.read(&self.entries).preaccepted().active_work != 0 {
            return Err(EffectCloseError::ActiveWork);
        }
        let mut effects = self.effects.lock();
        let before = effects.wake_projection();
        if !effects.close() {
            return Err(EffectCloseError::AlreadyClosed);
        }
        let after = effects.wake_projection();
        Ok(super::effect::EffectWakeTransition::between(before, after))
    }

    pub(super) fn effects_closed_and_drained(&self) -> bool {
        self.effects.lock().is_closed_and_drained()
    }

    pub(super) fn pending_recent_reject(&self, hash: &RawTxHash) -> Option<PendingRecentReject> {
        self.effects.lock().pending_recent_reject(hash)
    }

    #[cfg(test)]
    fn prepared_effect_only(&mut self, effect: EffectDelta) -> PreparedApply<'_> {
        PreparedApply::plain(
            self,
            PlainAuthorityDelta::Effect(EffectOnlyDelta { effect }),
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
                    owner_cuts.reserve_exact(1);
                    owner_cuts.push(IndependentOwnerCut {
                        key: owner.record().identity.raw.clone(),
                        expected: OwnerPrestate::from_owner(owner),
                        removal_revision: None,
                        action: IndependentOwnerAction::Observe,
                    });
                }
                let _clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
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
        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let version = clocks.replacement()?;
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
                let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
                    .map_err(PlanError::from)?;
                let sequence = clocks.sequence();
                let version = clocks.replacement().map_err(PlanError::from)?;
                let after = existing
                    .with_preaccepted_phase(phase, version, retained_charge)
                    .map_err(PlanError::Stale)?;
                let delta = self.compile_entry_delta_with_controls(
                    EntryTransition::Replace {
                        key: token.hash.clone(),
                        before: existing,
                        after,
                    },
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
        if matches!(error, PlanError::Stale(StalePlan::EffectSequence)) {
            return self.compute_settlement_changed_cut_failure(
                SettlementChangedCut::planning(StalePlan::EffectSequence),
                settlement,
            );
        }
        let ComputeSettlement { token, next } = settlement;
        let owner = self.settlement_failure_blame_peer(&token, &next);
        if matches!(error, PlanError::Stale(_)) {
            return match owner {
                Err(stale) => {
                    ComputeSettlementFailure::new(PlanError::Stale(stale), None, token, next)
                }
                Ok(blame_peer) => ComputeSettlementFailure::new(
                    PlanError::Fault(AuthorityFault::DependencyProjection),
                    blame_peer,
                    token,
                    next,
                ),
            };
        }
        ComputeSettlementFailure::new(error, owner.ok().flatten(), token, next)
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
        match self.settlement_failure_blame_peer(&token, &next) {
            Err(stale) => ComputeSettlementFailure::new(PlanError::Stale(stale), None, token, next),
            Ok(blame_peer) => {
                ComputeSettlementFailure::retry_exact(changed_cut, blame_peer, token, next)
            }
        }
    }

    fn settlement_failure_blame_peer(
        &self,
        token: &SettlementToken,
        next: &SettlementNext,
    ) -> Result<Option<ckb_network::PeerIndex>, StalePlan> {
        let owner = self.entries.get(&token.hash).ok_or(StalePlan::Missing)?;
        if owner.record().version != token.version {
            return Err(StalePlan::Version);
        }
        let OwnedTx::PreAccepted(preaccepted) = &*owner else {
            return Err(StalePlan::Phase);
        };
        if !matches!(preaccepted.phase, PreAcceptedPhase::Computing(_)) {
            return Err(StalePlan::Phase);
        }
        let malformed = match next {
            SettlementNext::Rejected(rejection) => rejection.is_malformed(),
            SettlementNext::VerificationRejected { rejection, .. } => rejection.is_malformed(),
            SettlementNext::QueuedVerify(_)
            | SettlementNext::Waiting(_)
            | SettlementNext::Ready(_)
            | SettlementNext::Retry => false,
        };
        Ok(if malformed {
            preaccepted.source.payload_blame_peer()
        } else {
            None
        })
    }

    #[cfg(test)]
    #[expect(
        clippy::result_large_err,
        reason = "the linear failure must return the exact unboxed settlement capability; boxing would allocate on effect/resource backpressure"
    )]
    pub(super) fn apply_settlement(
        &self,
        settlement: ComputeSettlement,
    ) -> Result<CommittedDelta, ComputeSettlementFailure> {
        let prepared = self.prepare_shared_compute_settlement(settlement)?;
        match prepared.apply() {
            SharedComputeSettlementOutcome::Committed(committed) => Ok(committed),
            SharedComputeSettlementOutcome::Failed {
                failure,
                effect_wake,
            } => {
                // Bare-authority tests have no runtime signal bus. Rollback
                // already returned journal capacity; this is only its prompt.
                let _ = effect_wake;
                Err(failure)
            }
        }
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
            sequence,
            TransitionControls::dependency(dependency),
            None,
        )?;
        Ok((delta, publication, sequence))
    }

    fn prepare_entry_delta(
        &mut self,
        transition: EntryTransition,
        sequence: ApplySequence,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            sequence,
            TransitionControls::none(),
            None,
        )
    }

    fn prepare_entry_delta_with_controls(
        &mut self,
        transition: EntryTransition,
        sequence: ApplySequence,
        controls: TransitionControls,
        explicit_resources: Option<ResourceBatchPlan>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.compile_entry_delta_with_controls(
            transition,
            sequence,
            controls,
            explicit_resources,
        )?;
        PreparedApply::prepare(self, DependencyAuthorityDelta::Entry(delta))
    }

    /// Compile the canonical one-owner transition without lending mutable
    /// authority to the planner. Exclusive callers wrap the result; shared
    /// callers lift the same delta into the existing independent batch engine.
    fn compile_entry_delta_with_controls(
        &self,
        transition: EntryTransition,
        sequence: ApplySequence,
        controls: TransitionControls,
        explicit_resources: Option<ResourceBatchPlan>,
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
            self.reserve_primary_owner_insertions(std::iter::once(&key));
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
