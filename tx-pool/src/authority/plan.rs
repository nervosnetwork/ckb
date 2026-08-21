mod chain_transition;
mod compute_exchange;
mod ingress;
mod membership;
mod settlement;

pub(in crate::authority) use self::compute_exchange::{
    CommittedComputeExchange, ComputeExchangeAssignment, ComputeExchangeCompletion,
    ComputeExchangeDeferred, ComputeExchangeDeferredRoute, ComputeExchangePlanFailure,
    ComputeExchangeRecoveries, ComputeExchangeRecoverySink, ComputeExchangeSettled,
};
pub(in crate::authority) use self::ingress::CommittedRetainedAdmissionBatch;

#[cfg(test)]
#[path = "tests/support/plan.rs"]
pub(in crate::authority) mod test_support;
#[cfg(test)]
use self::test_support::DependencyLossWork;

use super::ban::{PeerBanDelta, PeerBanError, PeerBanRegistry};
use super::chain::{
    DirectAdmissionReceipt, DirectAdmissionRejection, FinalAdmissionReceipt,
    FinalAdmissionRejection, FinalAdmissionRetry, FinalAdmissionSubject, FinalAdmissionWork,
    ReadyPayloadRelation,
};
use super::dependency::{
    DependencyBatchDelta, DependencyControlDelta, DependencyDelta, DependencyError,
    DependencyFrontier, DependencyMaintenanceAction, StableDependencyError,
};
use super::effect::{
    CommittedAcceptance, CommittedConflictOwner, CommittedEffect, CommittedEntrySnapshot,
    CommittedPeerCohortRevocation, CommittedRejection, CommittedRemoteIngressRelease, EffectBatch,
    EffectBuildError, EffectClosePlanError, EffectConfigError, EffectDelta, EffectError,
    EffectLimits, EffectLog, EffectPolicy, EffectPublication, EffectPublicationObservation,
    EffectSettlement, EffectSettlementPlan, EffectSettlementPlanError, EffectWakeProjection,
    ParentTransactionRequest, PendingRecentReject, RejectionAudience,
};
use super::indexes::{AuthorityIndexes, IndexDelta, IndexError, StableIndexError};
use super::ingress::{DirectCommand, RetainedIngressKind, RetainedIngressRejection};
use super::read::AuthorityReadView;
pub(in crate::authority) use super::rejection::MembershipReject;
use super::rejection::{
    CommittedPublicReject, DirectRejectionValidity, DirectTransactionRejection,
};
use super::resources::{
    ChargeRecord, ChargedAdmission, ComputeGrant, ComputeReleaseError, ResourceBatchPlan,
    ResourceError, ResourceLedger, ResourceLimits, ResourcePlan, ResourceVector,
};
use super::scheduler::{
    FairFrontier, QueueLane, SchedulerBatchDelta, SchedulerDelta, SchedulerError,
    SchedulerWakeProjection, VerifyOrder,
};
use super::source::{AuthoritySourceVersions, PoolTemplateVersions, SourceVersionDelta};
use super::state::{
    AcceptedAtMillis, AcceptedEntry, AcceptedProvenance, AdmissionBasis, ApplySequence, Arrival,
    AsyncProcessStart, AuthorityClocks, ChainRevision, ChainViewId, DependencyCut, DependencyKey,
    DependencyOrigin, EntryVersion, KnownDependencies, MissingDependencies, OwnedTx, PayloadPolicy,
    PayloadPolicyEvolution, PoolGeneration, PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource,
    ProposalBase, QueuedWork, RawTxHash, RemoteDeadline, ReplacementHistoryEntry,
    ReplacementHistoryError, ResolvedFacts, TxRecord, ValidatedAdmission,
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
pub(in crate::authority) use membership::MembershipConfig;
pub(in crate::authority) use membership::RemovalCause;
pub(in crate::authority) use membership::{
    AcceptedOrderKey, AncestorAggregate, DescendantAggregate, EvictionOrderKey,
    MembershipProjection,
};
use membership::{AcceptedRemovalSet, MembershipRemoval, PreparedMembership, ProjectionDelta};
pub(in crate::authority) use settlement::{IndependentCandidate, SettlementBatch, SettlementPlan};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::{sync::Arc, time::Instant};

#[derive(Debug)]
pub(super) struct TxPoolAuthority {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    entries: HashMap<RawTxHash, OwnedTx>,
    indexes: AuthorityIndexes,
    source_versions: AuthoritySourceVersions,
    resources: ResourceLedger,
    membership: MembershipProjection,
    scheduler: FairFrontier,
    dependencies: DependencyFrontier,
    effects: EffectLog,
    peer_bans: PeerBanRegistry,
    membership_config: MembershipConfig,
    clocks: AuthorityClocks,
}

impl TxPoolAuthority {
    pub(super) fn from_runtime(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effect_limits: EffectLimits,
        membership_config: MembershipConfig,
        chain_view: ChainViewId,
    ) -> Result<Self, AuthorityConfigError> {
        Ok(Self::assemble(
            limits,
            verify_order,
            EffectLog::new(effect_limits).map_err(AuthorityConfigError::Effect)?,
            membership_config,
            chain_view,
        ))
    }

    fn assemble(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effects: EffectLog,
        membership_config: MembershipConfig,
        chain_view: ChainViewId,
    ) -> Self {
        Self {
            generation: PoolGeneration(0),
            chain_view,
            entries: HashMap::new(),
            indexes: AuthorityIndexes::default(),
            source_versions: AuthoritySourceVersions::initial(),
            resources: ResourceLedger::new(limits),
            membership: MembershipProjection::default(),
            scheduler: FairFrontier::new(verify_order),
            dependencies: DependencyFrontier::default(),
            effects,
            peer_bans: PeerBanRegistry::default(),
            membership_config,
            clocks: AuthorityClocks::first(),
        }
    }

    pub(super) fn entry(&self, hash: &RawTxHash) -> Option<&OwnedTx> {
        self.entries.get(hash)
    }

    pub(super) fn operational_metrics(&self) -> crate::metrics::OperationalMetrics {
        let total = self.resources.preaccepted();
        let remote = self.resources.remote();
        let conflict = self.resources.replacement_history();
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
            effects: self.effects.operational_usage(),
        }
    }

    pub(super) fn accepted_spender(
        &self,
        input: &ckb_types::packed::OutPoint,
    ) -> Option<&RawTxHash> {
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
        DependencyCut(ApplySequence(self.clocks.next_sequence.0.saturating_sub(1)))
    }

    /// Borrow one immutable authority cut for every query projection. The view
    /// exposes neither the primary owner enum nor independently captured
    /// accepted/preaccepted collections.
    pub(super) fn read_view(&self) -> AuthorityReadView<'_> {
        AuthorityReadView::new(
            self.chain_view.clone(),
            &self.entries,
            &self.indexes,
            &self.membership,
            self.resources.accepted(),
            self.membership_config,
            self.source_versions.relay_parents(),
            self.source_versions.template(),
        )
    }

    /// Bounded strongest-first Ready identities for the runtime's sealed
    /// validation capture. Raw identities never cross the authority module.
    pub(in crate::authority) fn ready_candidates(
        &self,
    ) -> Result<Vec<(RawTxHash, EntryVersion)>, PlanError> {
        self.scheduler.ready().map_err(PlanError::from)
    }

    /// Compare an earlier Ready cut with the scheduler's current sole order
    /// authority without materializing another list under the authority guard.
    pub(in crate::authority) fn ready_common_prefix_len<'a>(
        &self,
        captured: impl IntoIterator<Item = (&'a RawTxHash, EntryVersion)>,
    ) -> usize {
        self.scheduler.ready_common_prefix_len(captured)
    }

    pub(in crate::authority) fn template_source_versions(&self) -> PoolTemplateVersions {
        self.source_versions.template()
    }

    fn wake_projection(&self) -> AuthorityWakeProjection {
        AuthorityWakeProjection {
            scheduler: self.scheduler.wake_projection(),
            active_work: self.resources.preaccepted().active_work,
            dependency_maintenance: self.dependencies.maintenance_pending(),
            effects: self.effects.wake_projection(),
            template: self.source_versions.template(),
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
    SourceVersion,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlanError {
    Duplicate,
    PayloadVariant,
    Membership(MembershipReject),
    Backpressure(Backpressure),
    Stale(StalePlan),
    Fault(AuthorityFault),
    EffectClosed,
}

/// A compute settlement that could not be committed still owns the exact
/// work capability. Callers may turn it into a deterministic cancellation,
/// or discard it only after proving the reported error makes the work stale.
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
/// selects deterministic cancellation followed by the generation terminal.
/// Every other planning outcome is structural in this context. Keeping that
/// distinction at the producer prevents a future `PlanError` variant from
/// silently becoming an unbounded worker retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ComputeSettlementRecovery {
    Obsolete(StalePlan),
    CancelAfterAllocation,
    WaitEffectCapacity,
    Structural(PlanError),
}

impl ComputeSettlementRecovery {
    fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Stale(stale) => Self::Obsolete(stale),
            PlanError::Backpressure(Backpressure::Allocation) => Self::CancelAfterAllocation,
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::WaitEffectCapacity,
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

    /// Discard an expensive result before reacquiring the authority guard and
    /// retain only the versioned identity required to requeue its owner.
    pub(super) fn discard_result_for_cancellation(self) -> ComputeCancellation {
        let Self {
            token,
            next,
            recovery: _,
        } = self;
        drop(next);
        ComputeCancellation { token }
    }
}

#[derive(Debug)]
#[must_use = "compute cancellation owns the only capability that can release active work"]
pub(super) struct ComputeCancellation {
    token: SettlementToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ComputeCancellationError {
    Obsolete(StalePlan),
    Fault(AuthorityFault),
    EffectClosed,
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
            | ResourceError::AttributionMismatch => Self::Fault(AuthorityFault::ResourceProjection),
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
            DependencyError::SurvivingAcceptedConsumer => {
                Self::Fault(AuthorityFault::DependencyProjection)
            }
        }
    }
}

impl From<EffectError> for PlanError {
    fn from(error: EffectError) -> Self {
        match error {
            EffectError::Full => Self::Backpressure(Backpressure::EffectCapacity),
            EffectError::Allocation => Self::Backpressure(Backpressure::Allocation),
            EffectError::Closed => Self::EffectClosed,
            EffectError::Projection => Self::Fault(AuthorityFault::EffectProjection),
        }
    }
}

impl From<PeerBanError> for PlanError {
    fn from(error: PeerBanError) -> Self {
        match error {
            PeerBanError::Allocation => Self::Backpressure(Backpressure::Allocation),
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
    One(AsyncProcessStart),
    Batch(Vec<AsyncProcessStart>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuthorityWakeProjection {
    scheduler: SchedulerWakeProjection,
    active_work: usize,
    dependency_maintenance: bool,
    effects: EffectWakeProjection,
    template: PoolTemplateVersions,
}

/// Exact before/after runnable projection produced by one committed Apply.
///
/// It carries no authority state and cannot select work. The runtime consumes
/// it only after the store guard and retirement payloads have been released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AuthorityWakeTransition {
    before: AuthorityWakeProjection,
    after: AuthorityWakeProjection,
}

impl AuthorityWakeTransition {
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
        !self.before.dependency_maintenance && self.after.dependency_maintenance
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

    pub(super) fn template_source_advanced(self) -> bool {
        self.before.template != self.after.template
    }
}

#[derive(Debug)]
#[must_use = "a committed delta owns post-Apply retirement and timing evidence"]
pub(super) struct CommittedDelta {
    async_process_observations: AsyncProcessObservations,
    /// Removal observations remain available to production property tests and are destroyed
    /// with the retirement carrier before post-commit publication.
    pub(super) removals: Vec<MembershipRemoval>,
    retired: Vec<OwnedTx>,
    retired_effect: Option<Arc<EffectBatch>>,
    retired_generation: Option<RetiredGeneration>,
    wake: AuthorityWakeTransition,
}

struct ApplyRetirement {
    async_process_observations: AsyncProcessObservations,
    removals: Vec<MembershipRemoval>,
    retired: Vec<OwnedTx>,
    retired_effect: Option<Arc<EffectBatch>>,
    retired_generation: Option<RetiredGeneration>,
}

/// Lock-external capability produced only after every retired authority value
/// has been destroyed. The runtime must consume it to publish derived wake
/// hints and asynchronous timing evidence.
#[must_use = "a post-commit receipt must be published after retirement"]
pub(super) struct AuthorityPostCommit {
    async_process_observations: AsyncProcessObservations,
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
    entries: HashMap<RawTxHash, OwnedTx>,
    _indexes: AuthorityIndexes,
    _resources: ResourceLedger,
    _membership: MembershipProjection,
    _scheduler: FairFrontier,
    _dependencies: DependencyFrontier,
}

enum EntryRetirement {
    InlineDrop,
    Outside(Vec<OwnedTx>),
}

/// Destruction policy for the owner replaced at the candidate key.
///
/// Ordinary final admission shares its transaction and resolved facts with
/// the Ready predecessor, so replacing that small shell is bounded. A direct
/// trusted admission may replace a distinct witness payload and must carry
/// the previous owner out of the authority guard.
#[derive(Clone, Copy)]
enum ChangedOwnerRetirement {
    VacantOrSharedShellInline,
    OutsideGuard,
}

/// Closed Plan-to-Apply carrier for every owner that must be destroyed after
/// the authority guard opens. The variant owns both the changed-owner drop
/// policy and the Vec whose capacity was fallibly reserved for that policy.
enum MembershipRetirement {
    Inline(Vec<OwnedTx>),
    Outside(Vec<OwnedTx>),
}

impl CommittedDelta {
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
            wake,
        } = self;
        drop(removals);
        drop(retired);
        drop(retired_effect);
        drop(retired_generation);
        AuthorityPostCommit {
            async_process_observations,
            wake,
        }
    }
}

impl AuthorityPostCommit {
    /// Publish the legacy asynchronous processing histogram only from the
    /// closed receipt of a successful membership Apply. Timing evidence is
    /// removed from Accepted ownership before this receipt is built, so a
    /// stale plan, retry, cancellation or journal replay cannot double-count.
    pub(in crate::authority) fn publish_metrics_and_take_wake(self) -> AuthorityWakeTransition {
        let Some(metrics) = ckb_metrics::handle() else {
            return self.wake;
        };
        let mut observe = |started_at: &AsyncProcessStart| {
            metrics
                .ckb_tx_pool_async_process
                .observe(started_at.elapsed_seconds());
        };
        match &self.async_process_observations {
            AsyncProcessObservations::None => {}
            AsyncProcessObservations::One(started_at) => observe(started_at),
            AsyncProcessObservations::Batch(started_at) => {
                started_at.iter().for_each(&mut observe);
            }
        }
        self.wake
    }
}

struct EntryDelta {
    key: RawTxHash,
    after: Option<OwnedTx>,
    owners: DerivedOwnerDelta,
    retirement: EntryRetirement,
    resource: ResourcePlan,
    scheduler: SchedulerDelta,
    dependency: DependencyDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
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
        retirement: EntryReplacementRetirement,
    },
    Remove {
        key: RawTxHash,
        before: OwnedTx,
    },
}

struct DerivedOwnerDelta {
    indexes: IndexDelta,
    sources: SourceVersionDelta,
}

struct TransitionControls {
    dependency: DependencyControlDelta,
    effect: EffectDelta,
}

impl TransitionControls {
    fn none() -> Self {
        Self {
            dependency: DependencyControlDelta::default(),
            effect: EffectDelta::default(),
        }
    }

    fn dependency(dependency: DependencyControlDelta) -> Self {
        Self {
            dependency,
            ..Self::none()
        }
    }

    fn effect(effect: EffectDelta) -> Self {
        Self {
            effect,
            ..Self::none()
        }
    }

    fn dependency_and_effect(dependency: DependencyControlDelta, effect: EffectDelta) -> Self {
        Self { dependency, effect }
    }
}

#[derive(Clone, Copy)]
enum EntryReplacementRetirement {
    SharedShellInline,
    OutsideGuard,
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
    changed_after: OwnedTx,
    retirement: MembershipRetirement,
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
    changed_retirement: ChangedOwnerRetirement,
    async_process_start: Option<AsyncProcessStart>,
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
    after: OwnedTx,
}

struct IndependentDelta {
    updates: Vec<IndependentUpdate>,
    owners: DerivedOwnerDelta,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    async_process_starts: Vec<AsyncProcessStart>,
}

struct DependencyOnlyDelta {
    control: DependencyControlDelta,
    clocks: AuthorityClocks,
}

struct EffectOnlyDelta {
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

struct FreshGeneration {
    entries: HashMap<RawTxHash, OwnedTx>,
    indexes: AuthorityIndexes,
    resources: ResourceLedger,
    membership: MembershipProjection,
    scheduler: FairFrontier,
    dependencies: DependencyFrontier,
}

impl FreshGeneration {
    fn empty(resources: &ResourceLedger, scheduler: &FairFrontier) -> Self {
        Self {
            entries: HashMap::new(),
            indexes: AuthorityIndexes::default(),
            resources: ResourceLedger::new(resources.limits()),
            membership: MembershipProjection::default(),
            scheduler: FairFrontier::new(scheduler.verify_order()),
            dependencies: DependencyFrontier::default(),
        }
    }
}

struct ClearPoolDelta {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    fresh: FreshGeneration,
    sources: SourceVersionDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

struct ClearPipelineDelta {
    generation: PoolGeneration,
    removal: OwnerRemovalBatch,
    effect: EffectDelta,
    clocks: AuthorityClocks,
}

enum AdminPlan {
    PeerRevocation {
        marker: PeerBanDelta,
        revocation: CommittedPeerCohortRevocation,
    },
    RemoteExpiry {
        cutoff: RemoteDeadline,
    },
    LocalRemoval {
        root: RawTxHash,
    },
    AcceptedExpiry {
        root: RawTxHash,
        cutoff: AcceptedAtMillis,
    },
}

#[cfg(any(test, feature = "internal"))]
impl AdminPlan {
    const fn apply_origin(&self) -> AuthorityApplyOrigin {
        match self {
            Self::PeerRevocation { .. } => AuthorityApplyOrigin::PeerRevocation,
            Self::RemoteExpiry { .. } => AuthorityApplyOrigin::RemoteExpiry,
            Self::LocalRemoval { .. } => AuthorityApplyOrigin::LocalRemoval,
            Self::AcceptedExpiry { .. } => AuthorityApplyOrigin::AcceptedExpiry,
        }
    }
}

enum AdminControl {
    PeerRevocation { marker: PeerBanDelta },
    None,
}

struct AdminDelta {
    control: AdminControl,
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

    fn truncate(&mut self, len: usize) {
        self.0.truncate(len);
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
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    membership: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    retired: Vec<OwnedTx>,
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
    retired: Vec<OwnedTx>,
    clocks: AuthorityClocks,
}

enum AuthorityDelta {
    Entry(EntryDelta),
    ComputeExchange(compute_exchange::ComputeExchangeDelta),
    RetainedIngress(ingress::RetainedIngressDelta),
    Membership(MembershipDelta),
    Independent(IndependentDelta),
    Dependency(DependencyOnlyDelta),
    Effect(EffectOnlyDelta),
    ClearPipeline(ClearPipelineDelta),
    ClearPool(ClearPoolDelta),
    Admin(AdminDelta),
    Chain(ChainDelta),
}

/// Compiler-bound outer carrier for one authoritative Apply.
///
/// This is a proof witness over the real production delta enum, not a second
/// transition engine.  It deliberately says nothing about `Q_H`, semantic
/// support or architecture completeness; those remain separate `rho` and
/// representation obligations.  Adding a production `AuthorityDelta` variant
/// must extend this exhaustive match before the internal proof universe can
/// compile again.
#[cfg(any(test, feature = "internal"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityApplyCarrier {
    Entry,
    ComputeExchange,
    RetainedIngress,
    Membership,
    Independent,
    Dependency,
    Effect,
    ClearPipeline,
    ClearPool,
    Administrative,
    Chain,
}

/// Production event source attached to every test/internal `PreparedApply`.
///
/// Unlike a route inventory, this witness is a required field at the real
/// Plan constructor. Internal compilation therefore rejects a new Apply
/// producer which omits its source declaration, and every executed internal
/// Apply checks that declaration against the real carrier. The value is
/// absent from default production builds and owns no policy or transition
/// result.
#[cfg(any(test, feature = "internal"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityApplyOrigin {
    RetainedAdmissionOwner,
    ProposalPromotionOwner,
    ReplacementHistoryPromotionOwner,
    FinalReresolutionOwner,
    PreacceptedTerminalizationOwner,
    ComputeSettlementOwner,
    ComputeRejectionOwner,
    ComputeCancellationOwner,
    DependencyMaintenanceOwner,
    FinalCandidateMembership,
    DirectAdmissionMembership,
    InternalPlugMembership,
    IndependentSettlement,
    RetainedIngressBatch,
    ComputeExchange,
    ComputeExchangePeerRevocation,
    RetainedIngressEffect,
    SingleEffectPublication,
    DirectDuplicateEffect,
    DirectMembershipRejectionEffect,
    EffectSettlement,
    EffectClose,
    #[cfg(test)]
    FixtureEffectPublication,
    #[cfg(test)]
    FixtureGenerationResetEffect,
    ClearPipeline,
    ClearPool,
    LocalRemoval,
    AcceptedExpiry,
    RemoteExpiry,
    PeerRevocation,
    DependencyAdvance,
    ChainTransition,
    ChainGenerationReplacement,
    #[cfg(test)]
    FixtureEntry,
    #[cfg(test)]
    FixtureMembership,
    #[cfg(test)]
    FixtureDependency,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityPrimarySupport {
    NoPrimaryOwner,
    Owners(Vec<RawTxHash>),
    GenerationAndOwners(Vec<RawTxHash>),
    WholeGeneration,
    ChainCutOwners(Vec<RawTxHash>),
}

#[cfg(any(test, feature = "internal"))]
impl AuthorityApplyOrigin {
    pub(in crate::authority) const fn carrier(self) -> AuthorityApplyCarrier {
        match self {
            Self::RetainedAdmissionOwner
            | Self::ProposalPromotionOwner
            | Self::ReplacementHistoryPromotionOwner
            | Self::FinalReresolutionOwner
            | Self::PreacceptedTerminalizationOwner
            | Self::ComputeSettlementOwner
            | Self::ComputeRejectionOwner
            | Self::ComputeCancellationOwner
            | Self::DependencyMaintenanceOwner => AuthorityApplyCarrier::Entry,
            #[cfg(test)]
            Self::FixtureEntry => AuthorityApplyCarrier::Entry,
            Self::FinalCandidateMembership
            | Self::DirectAdmissionMembership
            | Self::InternalPlugMembership => AuthorityApplyCarrier::Membership,
            #[cfg(test)]
            Self::FixtureMembership => AuthorityApplyCarrier::Membership,
            Self::IndependentSettlement => AuthorityApplyCarrier::Independent,
            Self::RetainedIngressBatch => AuthorityApplyCarrier::RetainedIngress,
            Self::ComputeExchange => AuthorityApplyCarrier::ComputeExchange,
            Self::SingleEffectPublication
            | Self::RetainedIngressEffect
            | Self::DirectDuplicateEffect
            | Self::DirectMembershipRejectionEffect
            | Self::EffectSettlement
            | Self::EffectClose => AuthorityApplyCarrier::Effect,
            #[cfg(test)]
            Self::FixtureEffectPublication | Self::FixtureGenerationResetEffect => {
                AuthorityApplyCarrier::Effect
            }
            Self::ClearPipeline => AuthorityApplyCarrier::ClearPipeline,
            Self::ClearPool => AuthorityApplyCarrier::ClearPool,
            Self::ComputeExchangePeerRevocation
            | Self::LocalRemoval
            | Self::AcceptedExpiry
            | Self::RemoteExpiry
            | Self::PeerRevocation => AuthorityApplyCarrier::Administrative,
            Self::DependencyAdvance => AuthorityApplyCarrier::Dependency,
            #[cfg(test)]
            Self::FixtureDependency => AuthorityApplyCarrier::Dependency,
            Self::ChainTransition => AuthorityApplyCarrier::Chain,
            Self::ChainGenerationReplacement => AuthorityApplyCarrier::ClearPool,
        }
    }
}

#[cfg(any(test, feature = "internal"))]
impl AuthorityDelta {
    pub(in crate::authority) const fn apply_carrier(&self) -> AuthorityApplyCarrier {
        match self {
            Self::Entry(_) => AuthorityApplyCarrier::Entry,
            Self::ComputeExchange(_) => AuthorityApplyCarrier::ComputeExchange,
            Self::RetainedIngress(_) => AuthorityApplyCarrier::RetainedIngress,
            Self::Membership(_) => AuthorityApplyCarrier::Membership,
            Self::Independent(_) => AuthorityApplyCarrier::Independent,
            Self::Dependency(_) => AuthorityApplyCarrier::Dependency,
            Self::Effect(_) => AuthorityApplyCarrier::Effect,
            Self::ClearPipeline(_) => AuthorityApplyCarrier::ClearPipeline,
            Self::ClearPool(_) => AuthorityApplyCarrier::ClearPool,
            Self::Admin(_) => AuthorityApplyCarrier::Administrative,
            Self::Chain(_) => AuthorityApplyCarrier::Chain,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn primary_support(&self) -> AuthorityPrimarySupport {
        fn owners(mut keys: Vec<RawTxHash>) -> AuthorityPrimarySupport {
            keys.sort_unstable();
            AuthorityPrimarySupport::Owners(keys)
        }

        match self {
            Self::Entry(delta) => owners(vec![delta.key.clone()]),
            Self::ComputeExchange(delta) => owners(delta.primary_keys_for_claim()),
            Self::RetainedIngress(delta) => owners(delta.primary_keys_for_claim()),
            Self::Membership(delta) => {
                let mut keys = Vec::with_capacity(delta.removals.len().saturating_add(1));
                keys.push(delta.changed_key.clone());
                keys.extend(delta.removals.iter().map(|removal| removal.hash.clone()));
                owners(keys)
            }
            Self::Independent(delta) => owners(
                delta
                    .updates
                    .iter()
                    .map(|update| update.key.clone())
                    .collect(),
            ),
            Self::Dependency(_) | Self::Effect(_) => AuthorityPrimarySupport::NoPrimaryOwner,
            Self::ClearPipeline(delta) => {
                let mut keys = delta.removal.hashes.clone();
                keys.sort_unstable();
                AuthorityPrimarySupport::GenerationAndOwners(keys)
            }
            Self::ClearPool(_) => AuthorityPrimarySupport::WholeGeneration,
            Self::Admin(delta) => owners(delta.removal.hashes.clone()),
            Self::Chain(delta) => {
                let mut keys: Vec<_> = delta
                    .updates
                    .iter()
                    .map(|update| update.key.clone())
                    .collect();
                keys.sort_unstable();
                AuthorityPrimarySupport::ChainCutOwners(keys)
            }
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

struct OwnerLocalSettlement {
    phase: PreAcceptedPhase,
    charge: ResourceVector,
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

#[must_use = "a prepared authority transition has no effect until explicitly applied"]
pub(super) struct PreparedApply<'authority> {
    authority: &'authority mut TxPoolAuthority,
    delta: AuthorityDelta,
    #[cfg(any(test, feature = "internal"))]
    origin: AuthorityApplyOrigin,
}

#[must_use = "candidate disposition must be applied exactly once"]
pub(super) enum CandidateDispositionPlan<'authority> {
    Accepted(PreparedApply<'authority>),
    Rejected(PreparedCandidateRejection<'authority>),
}

/// Closed final-validation disposition. A caller cannot turn a lock-external
/// validation failure into an ad-hoc retry or forget the matching committed
/// rejection effect.
#[must_use = "final admission disposition must be applied exactly once"]
pub(super) enum FinalAdmissionDispositionPlan<'authority> {
    Candidate(CandidateDispositionPlan<'authority>),
    ValidationRejected(PreparedValidationRejection<'authority>),
    Reresolve(PreparedApply<'authority>),
}

/// Complete synchronous trusted-admission result. Every branch owns the one
/// Apply that commits its externally visible outcome; the caller cannot
/// mutate membership and publish success/rejection independently.
#[must_use = "direct admission disposition must be applied exactly once"]
pub(super) enum DirectAdmissionDisposition<'authority> {
    Accepted(PreparedApply<'authority>),
    Duplicate(PreparedDirectDuplicate<'authority>),
    Rejected(PreparedDirectRejection<'authority>),
}

/// Feature-internal synthetic admission has only two legal committed
/// outcomes. A duplicate is a true no-op; insertion still owns the ordinary
/// atomic membership Apply.
#[cfg(any(test, feature = "internal"))]
#[must_use = "internal plug disposition must be applied or returned as a no-op"]
#[expect(
    clippy::large_enum_variant,
    reason = "the internal-only capability is consumed immediately; boxing would add allocation without reducing any retained production structure"
)]
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
/// evaluator result through `prepare_membership_candidate` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DirectAdmissionEvaluation {
    Accepted(EntryCompleted),
    Duplicate(RawTxHash),
    Rejected(MembershipReject),
}

#[must_use = "direct duplicate outcome must be applied exactly once"]
pub(super) struct PreparedDirectDuplicate<'authority> {
    key: RawTxHash,
    plan: PreparedApply<'authority>,
}

impl PreparedDirectDuplicate<'_> {
    pub(super) fn apply(self) -> (RawTxHash, CommittedDelta) {
        (self.key, self.plan.apply())
    }
}

#[must_use = "direct rejection outcome must be applied exactly once"]
pub(super) struct PreparedDirectRejection<'authority> {
    reason: MembershipReject,
    plan: PreparedApply<'authority>,
}

impl PreparedDirectRejection<'_> {
    pub(super) fn apply(self) -> (MembershipReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
}

/// A final candidate rejection whose public outcome is already part of the
/// same prepared authority Apply that removes the owner and releases charge.
/// The inner transition is private, so a caller cannot apply terminalization
/// while forgetting the matching journal publication.
#[must_use = "candidate rejection must be applied exactly once"]
pub(super) struct PreparedCandidateRejection<'authority> {
    reason: MembershipReject,
    plan: PreparedApply<'authority>,
}

#[must_use = "validation rejection must be applied exactly once"]
pub(super) struct PreparedValidationRejection<'authority> {
    reason: CommittedPublicReject,
    plan: PreparedApply<'authority>,
}

impl PreparedValidationRejection<'_> {
    pub(super) fn apply(self) -> (CommittedPublicReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
}

impl PreparedCandidateRejection<'_> {
    pub(super) fn apply(self) -> (MembershipReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
}

impl PreparedApply<'_> {
    pub(super) fn apply(self) -> CommittedDelta {
        #[cfg(any(test, feature = "internal"))]
        assert_eq!(
            self.origin.carrier(),
            self.delta.apply_carrier(),
            "the production Plan source must agree with its real Apply carrier"
        );
        let Self {
            authority,
            delta,
            #[cfg(any(test, feature = "internal"))]
                origin: _,
        } = self;
        let before = authority.wake_projection();
        let retirement = match delta {
            AuthorityDelta::Entry(delta) => Self::apply_entry(&mut *authority, delta),
            AuthorityDelta::ComputeExchange(delta) => {
                compute_exchange::apply_compute_exchange(&mut *authority, delta)
            }
            AuthorityDelta::RetainedIngress(delta) => {
                ingress::apply_retained_ingress(&mut *authority, delta)
            }
            AuthorityDelta::Membership(delta) => Self::apply_membership(&mut *authority, delta),
            AuthorityDelta::Independent(delta) => Self::apply_independent(&mut *authority, delta),
            AuthorityDelta::Dependency(delta) => Self::apply_dependency(&mut *authority, delta),
            AuthorityDelta::Effect(delta) => Self::apply_effect(&mut *authority, delta),
            AuthorityDelta::ClearPipeline(delta) => {
                Self::apply_clear_pipeline(&mut *authority, delta)
            }
            AuthorityDelta::ClearPool(delta) => Self::apply_clear_pool(&mut *authority, delta),
            AuthorityDelta::Admin(delta) => Self::apply_admin(&mut *authority, delta),
            AuthorityDelta::Chain(delta) => Self::apply_chain(&mut *authority, delta),
        };
        let after = authority.wake_projection();
        let ApplyRetirement {
            async_process_observations,
            removals,
            retired,
            retired_effect,
            retired_generation,
        } = retirement;
        CommittedDelta {
            async_process_observations,
            removals,
            retired,
            retired_effect,
            retired_generation,
            wake: AuthorityWakeTransition { before, after },
        }
    }

    fn apply_entry(authority: &mut TxPoolAuthority, delta: EntryDelta) -> ApplyRetirement {
        let previous = match delta.after {
            Some(entry) => authority.entries.insert(delta.key.clone(), entry),
            None => authority.entries.remove(&delta.key),
        };
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        let retired = match (delta.retirement, previous) {
            (EntryRetirement::Outside(mut retired), Some(owner)) => {
                retired.push(owner);
                retired
            }
            (EntryRetirement::Outside(retired), None) => retired,
            (EntryRetirement::InlineDrop, previous) => {
                drop(previous);
                Vec::new()
            }
        };
        authority.resources.apply(delta.resource);
        authority.scheduler.apply(delta.scheduler);
        authority.dependencies.apply(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_membership(
        authority: &mut TxPoolAuthority,
        mut delta: MembershipDelta,
    ) -> ApplyRetirement {
        let mut retirement = delta.retirement;
        for removal in &mut delta.removals {
            let previous = match removal.take_after() {
                Some(after) => authority.entries.insert(removal.hash.clone(), after),
                None => authority.entries.remove(&removal.hash),
            };
            if let Some(owner) = previous {
                match &mut retirement {
                    MembershipRetirement::Inline(retired)
                    | MembershipRetirement::Outside(retired) => retired.push(owner),
                }
            }
        }
        let previous = authority
            .entries
            .insert(delta.changed_key, delta.changed_after);
        let retired = match (retirement, previous) {
            (MembershipRetirement::Inline(retired), previous) => {
                drop(previous);
                retired
            }
            (MembershipRetirement::Outside(mut retired), Some(owner)) => {
                // The carrier was reserved while planning; Apply performs no
                // allocation and the last payload reference dies only after
                // the authority guard is released.
                retired.push(owner);
                retired
            }
            (MembershipRetirement::Outside(retired), None) => retired,
        };
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        authority.resources.apply_batch(delta.resource);
        authority.membership.apply(delta.projection);
        authority.scheduler.apply(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: delta.async_process_start.map_or(
                AsyncProcessObservations::None,
                AsyncProcessObservations::One,
            ),
            removals: delta.removals,
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_independent(
        authority: &mut TxPoolAuthority,
        delta: IndependentDelta,
    ) -> ApplyRetirement {
        for update in delta.updates {
            // Independent acceptance also replaces a pre-accepted shell whose
            // immutable transaction and resolved facts are shared by `after`.
            drop(authority.entries.insert(update.key, update.after));
        }
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        authority.resources.apply_batch(delta.resource);
        authority.membership.apply(delta.projection);
        authority.scheduler.apply_batch(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: if delta.async_process_starts.is_empty() {
                AsyncProcessObservations::None
            } else {
                AsyncProcessObservations::Batch(delta.async_process_starts)
            },
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_dependency(
        authority: &mut TxPoolAuthority,
        delta: DependencyOnlyDelta,
    ) -> ApplyRetirement {
        authority.dependencies.apply_control(delta.control);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect: None,
            retired_generation: None,
        }
    }

    fn apply_effect(authority: &mut TxPoolAuthority, delta: EffectOnlyDelta) -> ApplyRetirement {
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_clear_pool(authority: &mut TxPoolAuthority, delta: ClearPoolDelta) -> ApplyRetirement {
        let FreshGeneration {
            entries,
            indexes,
            resources,
            membership,
            scheduler,
            dependencies,
        } = delta.fresh;
        let retired_generation = RetiredGeneration {
            entries: std::mem::replace(&mut authority.entries, entries),
            _indexes: std::mem::replace(&mut authority.indexes, indexes),
            _resources: std::mem::replace(&mut authority.resources, resources),
            _membership: std::mem::replace(&mut authority.membership, membership),
            _scheduler: std::mem::replace(&mut authority.scheduler, scheduler),
            _dependencies: std::mem::replace(&mut authority.dependencies, dependencies),
        };
        authority.generation = delta.generation;
        authority.chain_view = delta.chain_view;
        authority.source_versions.apply(delta.sources);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect,
            retired_generation: Some(retired_generation),
        }
    }

    fn apply_clear_pipeline(
        authority: &mut TxPoolAuthority,
        delta: ClearPipelineDelta,
    ) -> ApplyRetirement {
        let retired = Self::apply_owner_removal(authority, delta.removal);
        authority.generation = delta.generation;
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_admin(authority: &mut TxPoolAuthority, delta: AdminDelta) -> ApplyRetirement {
        let retired = Self::apply_owner_removal(authority, delta.removal);
        let retired_effect = authority.effects.apply(delta.effect);
        match delta.control {
            AdminControl::PeerRevocation { marker } => authority.peer_bans.apply(marker),
            AdminControl::None => {}
        }
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_owner_removal(
        authority: &mut TxPoolAuthority,
        removal: OwnerRemovalBatch,
    ) -> Vec<OwnedTx> {
        let mut retired = removal.retired;
        for hash in &removal.hashes {
            if let Some(owner) = authority.entries.remove(hash) {
                retired.push(owner);
            }
        }
        authority.indexes.apply(removal.owners.indexes);
        authority.source_versions.apply(removal.owners.sources);
        authority.resources.apply_batch(removal.resources);
        authority.membership.apply(removal.membership);
        authority.scheduler.apply_batch(removal.scheduler);
        authority.dependencies.apply_batch(removal.dependency);
        retired
    }

    fn apply_chain(authority: &mut TxPoolAuthority, delta: ChainDelta) -> ApplyRetirement {
        let mut retired = delta.retired;
        for update in delta.updates {
            let previous = match update.after {
                Some(after) => authority.entries.insert(update.key, after),
                None => authority.entries.remove(&update.key),
            };
            if let Some(previous) = previous {
                retired.push(previous);
            }
        }
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        authority.resources.apply_batch(delta.resources);
        authority.membership.apply(delta.membership);
        authority.scheduler.apply_batch(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.chain_view = delta.view.clone();
        authority.clocks = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
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

/// A discardable Plan capability for prospective owner identities.
///
/// An ordered batch may need versions and arrivals before resource projection
/// proves that any owner or effect survives. This phase deliberately cannot
/// expose an Apply sequence or finish into authoritative clocks. It must be
/// consumed by [`Self::commit`] before a nonempty transition can be built.
pub(in crate::authority) struct ClockPlanReservation {
    clocks: AuthorityClocks,
}

/// One prospective owner-identity branch borrowed from a live clock Plan.
///
/// Reservations change only this stack value. Dropping it is a true no-op;
/// [`Self::adopt`] is the only transition that advances the borrowed parent.
/// The exclusive borrow prevents stale or cross-parent adoption.
pub(in crate::authority) struct OwnerClockBranch<'parent> {
    parent: &'parent mut ClockPlanReservation,
    next_version: EntryVersion,
    next_arrival: Arrival,
}

impl OwnerClockBranch<'_> {
    pub(in crate::authority) fn replacement(
        mut self,
    ) -> Result<(EntryVersion, Self), ClockReservationError> {
        let version = self.next_version;
        let next_version = version
            .0
            .checked_add(1)
            .map(EntryVersion)
            .ok_or(ClockReservationError)?;
        self.next_version = next_version;
        Ok((version, self))
    }

    pub(in crate::authority) fn insertion(
        mut self,
    ) -> Result<(EntryVersion, Arrival, Self), ClockReservationError> {
        let version = self.next_version;
        let next_version = version
            .0
            .checked_add(1)
            .map(EntryVersion)
            .ok_or(ClockReservationError)?;
        let arrival = self.next_arrival;
        let next_arrival = arrival
            .0
            .checked_add(1)
            .map(Arrival)
            .ok_or(ClockReservationError)?;
        self.next_version = next_version;
        self.next_arrival = next_arrival;
        Ok((version, arrival, self))
    }

    pub(in crate::authority) fn replacements(
        mut self,
        members: NonZeroUsize,
    ) -> Result<(impl Iterator<Item = EntryVersion> + use<>, Self), ClockReservationError> {
        let member_count = u128::try_from(members.get()).map_err(|_| ClockReservationError)?;
        let first_version = self.next_version.0;
        let next_version = first_version
            .checked_add(member_count)
            .map(EntryVersion)
            .ok_or(ClockReservationError)?;
        self.next_version = next_version;
        Ok(((first_version..next_version.0).map(EntryVersion), self))
    }

    pub(in crate::authority) fn adopt(self) {
        self.parent.clocks.next_version = self.next_version;
        self.parent.clocks.next_arrival = self.next_arrival;
    }
}

impl ClockPlanReservation {
    pub(in crate::authority) const fn begin(clocks: AuthorityClocks) -> Self {
        Self { clocks }
    }

    pub(in crate::authority) fn commit(
        mut self,
    ) -> Result<ApplyClockReservation, ClockReservationError> {
        let sequence = self.clocks.next_sequence;
        self.clocks.next_sequence = sequence
            .0
            .checked_add(1)
            .map(ApplySequence)
            .ok_or(ClockReservationError)?;
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
        self.clocks.next_version = owner_clocks.next_version;
        self.clocks.next_arrival = owner_clocks.next_arrival;
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
        clocks: AuthorityClocks,
    ) -> Result<Self, ClockReservationError> {
        ClockPlanReservation::begin(clocks).commit()
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

    pub(in crate::authority) const fn finish(self) -> AuthorityClocks {
        self.plan.clocks
    }
}

fn retired_buffer(capacity: usize) -> Result<Vec<OwnedTx>, PlanError> {
    let mut retired = Vec::new();
    retired
        .try_reserve(capacity)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    Ok(retired)
}

impl TxPoolAuthority {
    fn reserve_primary_owner_insertions(&mut self, additional: usize) -> Result<(), PlanError> {
        if additional == 0 {
            return Ok(());
        }
        self.entries
            .try_reserve(additional)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))
    }

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
                                self.entries.get(&RawTxHash(out_point.tx_hash())),
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
        let OwnedTx::PreAccepted(preaccepted) = existing else {
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

    fn plan_membership_dependency_delta(
        &self,
        existing: Option<&OwnedTx>,
        after: &OwnedTx,
        removals: &[MembershipRemoval],
        sequence: ApplySequence,
    ) -> Result<DependencyBatchDelta, PlanError> {
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
            let removed = self
                .entries
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
        Ok(delta.with_control(control))
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
            let victim = match self.entries.get(&removal.hash) {
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
            let victim = match self.entries.get(&removal.hash) {
                Some(OwnedTx::Accepted(entry)) => entry,
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            for input in victim.proof.payload().footprint.inputs() {
                if self.released_input_survives_final_owner_set(
                    victim,
                    input,
                    final_owners,
                    ReleasedInputContext::Replacement {
                        candidate_inputs: &candidate_inputs,
                    },
                )? {
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
        let capacity = removals.iter().try_fold(0usize, |total, hash| {
            let entry = match self.entries.get(hash) {
                Some(OwnedTx::Accepted(entry)) => entry,
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            total
                .checked_add(entry.proof.payload().footprint.inputs().len())
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
        })?;
        let mut available = Vec::new();
        available
            .try_reserve(capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        let final_owners = ProjectedFinalOwnerSet {
            removed: ProjectedRemovalSet::Administrative(removals),
        };
        for hash in removals.iter() {
            let entry = match self.entries.get(hash) {
                Some(OwnedTx::Accepted(entry)) => entry,
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            };
            for input in entry.proof.payload().footprint.inputs() {
                if self.released_input_survives_final_owner_set(
                    entry,
                    input,
                    final_owners,
                    ReleasedInputContext::Administrative { victim: hash },
                )? {
                    available.push(DependencyKey::Cell(input.clone()));
                }
            }
        }
        Ok(available)
    }

    /// Decide one removed input from the projected final membership set. The
    /// context owns only the distinct spender premise; backing-cell survival
    /// has one implementation for replacement and administrative cohorts.
    fn released_input_survives_final_owner_set(
        &self,
        removed_entry: &AcceptedEntry,
        input: &OutPoint,
        final_owners: ProjectedFinalOwnerSet<'_>,
        context: ReleasedInputContext<'_>,
    ) -> Result<bool, PlanError> {
        match context {
            ReleasedInputContext::Replacement { candidate_inputs } => {
                if candidate_inputs.contains(input) {
                    return Ok(false);
                }
                let spender = self
                    .membership
                    .spender(input)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                if !final_owners.contains_removed(spender) {
                    return Ok(false);
                }
            }
            ReleasedInputContext::Administrative { victim } => {
                if self.membership.spender(input) != Some(victim) {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
            }
        }
        if removed_entry.proof.is_chain_input(input) {
            return Ok(true);
        }
        let parent = RawTxHash(input.tx_hash());
        if final_owners.contains_removed(&parent) {
            return Ok(false);
        }
        let Some(OwnedTx::Accepted(parent)) = self.entries.get(&parent) else {
            return Ok(false);
        };
        let index: u32 = input.index().unpack();
        Ok(usize::try_from(index)
            .ok()
            .is_some_and(|index| index < parent.record.tx.outputs().len()))
    }

    fn plan_membership_owner_derivations(
        &mut self,
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
        let source_versions = self.source_versions;
        let Self {
            entries, indexes, ..
        } = self;
        if removals.is_empty() {
            let indexes = indexes.plan_replace(key, existing, Some(after))?;
            let sources = source_versions
                .plan_replacements(std::iter::once((existing, Some(after))), sequence);
            return Ok(DerivedOwnerDelta { indexes, sources });
        }
        let mut changes = Vec::new();
        changes
            .try_reserve(change_capacity)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        changes.push((key, existing, Some(after)));
        for removal in removals {
            let removed = entries
                .get(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            changes.push((&removal.hash, Some(removed), removal.after()));
        }
        let sources = source_versions.plan_replacements(
            changes.iter().map(|(_, before, after)| (*before, *after)),
            sequence,
        );
        let indexes = indexes.plan_replacements(changes)?;
        Ok(DerivedOwnerDelta { indexes, sources })
    }

    /// Build the optional Accepted-victim continuation as one all-or-none
    /// cohort. Allocation pressure drops the optional history before any
    /// authoritative mutation; structural failures remain explicit faults.
    fn retain_replacement_history<'clock>(
        &self,
        candidate: &AcceptedEntry,
        removals: &mut [MembershipRemoval],
        sequence: ApplySequence,
        mut clocks: OwnerClockBranch<'clock>,
    ) -> Result<(bool, OwnerClockBranch<'clock>), PlanError> {
        if !removals
            .iter()
            .any(|removal| removal.cause == RemovalCause::Replacement)
        {
            return Ok((true, clocks));
        }
        let mut removed = HashSet::new();
        if removed.try_reserve(removals.len()).is_err() {
            return Ok((false, clocks));
        }
        removed.extend(removals.iter().map(|removal| removal.hash.clone()));

        // ExpandedFootprint canonicalizes inputs into sorted unique order, so
        // RBF-only trigger derivation needs no second candidate-input index.
        let candidate_inputs = candidate.proof.payload().footprint.inputs();
        for removal in removals.iter_mut() {
            if removal.cause != RemovalCause::Replacement {
                continue;
            }
            let accepted = match self.entries.get(&removal.hash) {
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
                return Ok((false, clocks));
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
                    return Ok((false, clocks));
                }
                Err(
                    super::state::DependencySetError::Empty
                    | super::state::DependencySetError::TooMany
                    | super::state::DependencySetError::Arithmetic,
                ) => return Err(PlanError::Fault(AuthorityFault::MembershipProjection)),
            };
            let (version, arrival, next_clocks) = clocks.insertion()?;
            clocks = next_clocks;
            let history = match ReplacementHistoryEntry::from_accepted(
                accepted,
                recovery_triggers,
                self.resources
                    .replacement_history_charge(
                        &accepted.record.tx,
                        accepted.proof.payload().dependencies().len(),
                    )
                    .map_err(Self::membership_resource_error)?,
                version,
                arrival,
                DependencyCut(sequence),
            ) {
                Ok(history) => history,
                Err(ReplacementHistoryError::ResourceAllocation) => {
                    return Ok((false, clocks));
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
        Ok((true, clocks))
    }

    fn plan_membership_resources(
        &mut self,
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
        self.resources.plan_batch(changes)
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
            | ResourceError::AttributionMismatch => {
                PlanError::Fault(AuthorityFault::ResourceProjection)
            }
        }
    }

    fn plan_dependency_loss<'entry>(
        &self,
        parents: impl IntoIterator<Item = &'entry OwnedTx>,
        sequence: ApplySequence,
    ) -> Result<DependencyControlDelta, PlanError> {
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
        let mut keys = Vec::new();
        #[cfg(test)]
        let mut work = DependencyLossWork::default();
        for parent in parents {
            let record = parent.record();
            let output_count = record.tx.data().raw().outputs().len();
            let origin = DependencyOrigin::Transaction(record.identity.raw.clone());
            let origin_keys = self.dependencies.keys_for_origin(&origin);
            let origin_count = origin_keys.map_or(0, |keys| keys.len());
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

    pub(super) fn plan_retained_ingress_rejection(
        &mut self,
        rejection: RetainedIngressRejection,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let (kind, tx, reason) = rejection.into_parts();
        match kind {
            RetainedIngressKind::Remote(peer) if reason.is_malformed() => {
                self.plan_peer_revocation(peer, RawTxHash(tx.hash()), reason)
            }
            RetainedIngressKind::Remote(peer) => self.plan_single_effect(
                EffectPolicy::Remote,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx,
                    audience: RejectionAudience::from_ingress(Some(peer)),
                    reason,
                }),
            ),
            RetainedIngressKind::Proposal => self.plan_single_effect(
                EffectPolicy::Trusted,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx,
                    audience: RejectionAudience::from_ingress(None),
                    reason,
                }),
            ),
        }
    }

    fn plan_charged_admission(
        &mut self,
        admission: ChargedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let key = admission.admission().identity.raw.clone();
        if let Some(existing) = self.entries.get(&key).cloned() {
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

        let clocks = ApplyClockReservation::begin(self.clocks)?;
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
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::RetainedAdmissionOwner,
        )
    }

    fn plan_single_effect(
        &mut self,
        policy: EffectPolicy,
        effect: CommittedEffect,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let publication = self
            .effects
            .build_single_publication(policy, effect)
            .map_err(PlanError::from)?;
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let effect = self.effects.plan_publication(&publication, sequence)?;
        Ok(self.prepared_effect_only(
            effect,
            clocks,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::SingleEffectPublication,
        ))
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

        let clocks = ApplyClockReservation::begin(self.clocks)?;
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
        // A distinct trusted payload retires the exact old owner outside the
        // authority guard. A checked-out worker still holds the old version
        // and can only return a typed stale completion.
        let retirement = if same_witness {
            EntryReplacementRetirement::SharedShellInline
        } else {
            EntryReplacementRetirement::OutsideGuard
        };
        self.prepare_entry_delta_with_controls(
            EntryTransition::Replace {
                key,
                before: existing,
                after: OwnedTx::PreAccepted(promoted),
                retirement,
            },
            clocks.finish(),
            sequence,
            TransitionControls::none(),
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::ProposalPromotionOwner,
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
        let clocks = ApplyClockReservation::begin(self.clocks)?;
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
        self.prepare_entry_delta_with_controls(
            EntryTransition::Replace {
                key,
                before: existing,
                after: OwnedTx::PreAccepted(promoted),
                retirement: EntryReplacementRetirement::SharedShellInline,
            },
            clocks.finish(),
            sequence,
            TransitionControls::none(),
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::ReplacementHistoryPromotionOwner,
        )
    }

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
        let OwnedTx::PreAccepted(preaccepted) = existing else {
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

    fn plan_final_reresolution(
        &mut self,
        retry: FinalAdmissionRetry,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let subject = retry.into_subject();
        let preaccepted = self.final_admission_subject_owner(&subject)?;
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let mut requeued = preaccepted.clone();
        requeued.record.version = version;
        requeued.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
        requeued.charge = preaccepted.original_charge();
        self.prepare_entry_delta_with_controls(
            EntryTransition::Replace {
                key: subject.key().clone(),
                before: OwnedTx::PreAccepted(preaccepted),
                after: OwnedTx::PreAccepted(requeued),
                retirement: EntryReplacementRetirement::OutsideGuard,
            },
            clocks.finish(),
            sequence,
            TransitionControls::none(),
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::FinalReresolutionOwner,
        )
    }

    /// Compile the only final-membership command exposed to production.  Both
    /// success and policy rejection are complete owner/resource/effect Plans;
    /// the caller cannot receive a bare membership error after validation and
    /// then independently decide whether to terminalize or publish it.
    fn plan_candidate_disposition(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<CandidateDispositionPlan<'_>, PlanError> {
        let key = receipt.key().clone();
        let expected = receipt.expected();
        match self.prepare_accept_delta(receipt) {
            Ok(delta) => Ok(CandidateDispositionPlan::Accepted(PreparedApply {
                authority: self,
                delta: AuthorityDelta::Membership(delta),
                #[cfg(any(test, feature = "internal"))]
                origin: AuthorityApplyOrigin::FinalCandidateMembership,
            })),
            Err(PlanError::Membership(reason)) => {
                let existing = self
                    .entries
                    .get(&key)
                    .ok_or(PlanError::Stale(StalePlan::Missing))?;
                let OwnedTx::PreAccepted(preaccepted) = existing else {
                    return Err(PlanError::Stale(StalePlan::Phase));
                };
                let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
                let publication = self
                    .effects
                    .build_single_publication(
                        policy,
                        CommittedEffect::Rejected(CommittedRejection::Membership {
                            tx: Arc::clone(&preaccepted.record.tx),
                            audience: RejectionAudience::from_source(preaccepted.source),
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

    pub(super) fn plan_direct_admission(
        &mut self,
        receipt: DirectAdmissionReceipt,
    ) -> Result<DirectAdmissionDisposition<'_>, PlanError> {
        self.effects.ensure_open()?;
        let key = receipt.key().clone();
        let existing = self.entries.get(&key).cloned();
        if matches!(&existing, Some(OwnedTx::Accepted(_))) {
            let publication = self
                .effects
                .build_single_publication(
                    EffectPolicy::Trusted,
                    CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                        tx_hash: key.clone(),
                        requesting_peer: None,
                    }),
                )
                .map_err(PlanError::from)?;
            let clocks = ApplyClockReservation::begin(self.clocks)?;
            let sequence = clocks.sequence();
            let effect = self
                .effects
                .plan_publication(&publication, sequence)
                .map_err(PlanError::from)?;
            let plan = self.prepared_effect_only(
                effect,
                clocks,
                #[cfg(any(test, feature = "internal"))]
                AuthorityApplyOrigin::DirectDuplicateEffect,
            );
            return Ok(DirectAdmissionDisposition::Duplicate(
                PreparedDirectDuplicate { key, plan },
            ));
        }
        self.validate_direct_acceptance_evidence(&receipt)?;

        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let (version, arrival, clocks) = match &existing {
            Some(owner) => {
                let (version, clocks) = clocks.replacement()?;
                (version, owner.record().arrival, clocks)
            }
            None => clocks.insertion()?,
        };
        let (accepted, async_process_start) =
            Self::direct_candidate(receipt, existing.as_ref(), version, arrival);
        let prepared = match self.prepare_membership_candidate(&key, &accepted) {
            Ok(prepared) => prepared,
            Err(PlanError::Membership(reason)) => {
                let publication = self
                    .effects
                    .build_single_publication(
                        EffectPolicy::Trusted,
                        CommittedEffect::Rejected(CommittedRejection::Membership {
                            tx: Arc::clone(&accepted.record.tx),
                            audience: RejectionAudience::default(),
                            reason: reason.clone(),
                        }),
                    )
                    .map_err(PlanError::from)?;
                let effect_clocks = ApplyClockReservation::begin(self.clocks)?;
                let effect = self
                    .effects
                    .plan_publication(&publication, effect_clocks.sequence())
                    .map_err(PlanError::from)?;
                let plan = self.prepared_effect_only(
                    effect,
                    effect_clocks,
                    #[cfg(any(test, feature = "internal"))]
                    AuthorityApplyOrigin::DirectMembershipRejectionEffect,
                );
                return Ok(DirectAdmissionDisposition::Rejected(
                    PreparedDirectRejection { reason, plan },
                ));
            }
            Err(error) => return Err(error),
        };
        let retirement = if existing.is_some() {
            ChangedOwnerRetirement::OutsideGuard
        } else {
            ChangedOwnerRetirement::VacantOrSharedShellInline
        };
        let delta = self.compile_membership_delta(MembershipCompilation {
            key,
            existing,
            accepted,
            prepared,
            clocks,
            effects: MembershipEffects::Publish(EffectPolicy::Trusted),
            changed_retirement: retirement,
            async_process_start,
        })?;
        Ok(DirectAdmissionDisposition::Accepted(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
            #[cfg(any(test, feature = "internal"))]
            origin: AuthorityApplyOrigin::DirectAdmissionMembership,
        }))
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
        self.effects.ensure_open().map_err(PlanError::from)?;
        let key = receipt.key().clone();
        if self.entries.contains_key(&key) {
            return Ok(InternalPlugDisposition::Duplicate);
        }
        self.validate_direct_acceptance_evidence(&receipt)?;

        let clocks = ApplyClockReservation::begin(self.clocks).map_err(PlanError::from)?;
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
            changed_retirement: ChangedOwnerRetirement::VacantOrSharedShellInline,
            async_process_start,
        })?;
        Ok(InternalPlugDisposition::Insert(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
            origin: AuthorityApplyOrigin::InternalPlugMembership,
        }))
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
        let (accepted, _async_process_start) = Self::direct_candidate(
            receipt,
            None,
            self.clocks.next_version,
            self.clocks.next_arrival,
        );
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

    /// Commit an owner-free ingress/resolve/verify rejection only while its
    /// sealed validity evidence still matches this authority cut.
    pub(super) fn plan_direct_transaction_rejection(
        &mut self,
        rejection: DirectTransactionRejection,
    ) -> Result<PreparedValidationRejection<'_>, PlanError> {
        let (tx, command, reason, validity) = rejection.into_parts();
        if command != DirectCommand::Local {
            return Err(PlanError::Fault(AuthorityFault::EffectProjection));
        }
        self.validate_direct_rejection_validity(&validity)?;
        let plan = self.plan_single_effect(
            EffectPolicy::Trusted,
            CommittedEffect::Rejected(CommittedRejection::Validation {
                tx,
                audience: RejectionAudience::default(),
                reason: reason.clone(),
            }),
        )?;
        Ok(PreparedValidationRejection { reason, plan })
    }

    /// Commit a final direct-validation rejection only if the exact positive
    /// dependency proof consumed by the validator remains current.
    pub(super) fn plan_direct_validation_rejection(
        &mut self,
        rejection: DirectAdmissionRejection,
    ) -> Result<PreparedValidationRejection<'_>, PlanError> {
        let (subject, reason) = rejection.into_parts();
        self.validate_direct_admission_subject(&subject)?;
        let tx = subject.into_transaction();
        let plan = self.plan_single_effect(
            EffectPolicy::Trusted,
            CommittedEffect::Rejected(CommittedRejection::Validation {
                tx,
                audience: RejectionAudience::default(),
                reason: reason.clone(),
            }),
        )?;
        Ok(PreparedValidationRejection { reason, plan })
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

    fn prepare_accept_delta(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<MembershipDelta, PlanError> {
        self.effects.ensure_open()?;
        let changed_retirement = match receipt.payload_relation() {
            ReadyPayloadRelation::Shared => ChangedOwnerRetirement::VacantOrSharedShellInline,
            ReadyPayloadRelation::LocationRefreshed => ChangedOwnerRetirement::OutsideGuard,
        };
        let key = receipt.key().clone();
        let expected = receipt.expected();
        let existing = self
            .entries
            .get(&key)
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
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let (version, clocks) = clocks.replacement()?;
        let mut record = preaccepted.record.clone();
        record.version = version;
        let accepted = AcceptedEntry {
            record,
            provenance: preaccepted.source.accepted_provenance(),
            proof,
            proposal,
            accepted_at,
        };
        let prepared = self.prepare_membership(&key, preaccepted, &accepted)?;
        let effect_policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        self.compile_membership_delta(MembershipCompilation {
            key,
            existing: Some(existing),
            accepted,
            prepared,
            clocks,
            effects: MembershipEffects::Publish(effect_policy),
            changed_retirement,
            async_process_start,
        })
    }

    fn compile_membership_delta(
        &mut self,
        compilation: MembershipCompilation,
    ) -> Result<MembershipDelta, PlanError> {
        let MembershipCompilation {
            key,
            existing,
            accepted,
            prepared,
            mut clocks,
            effects,
            changed_retirement,
            async_process_start,
        } = compilation;
        if existing.is_none() {
            self.reserve_primary_owner_insertions(1)?;
        }
        let PreparedMembership {
            mut removals,
            projection,
        } = prepared;
        let sequence = clocks.sequence();
        let history_clocks = clocks.owner_branch();
        let (mut retained_history, history_clocks) =
            self.retain_replacement_history(&accepted, &mut removals, sequence, history_clocks)?;
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
        let resource =
            match self.plan_membership_resources(&key, existing.as_ref(), &after, &removals) {
                Ok(resource) => resource,
                Err(ResourceError::PreAcceptedLimit | ResourceError::ReplacementHistoryLimit) => {
                    removals.iter_mut().for_each(MembershipRemoval::terminalize);
                    retained_history = false;
                    self.plan_membership_resources(&key, existing.as_ref(), &after, &removals)
                        .map_err(Self::membership_resource_error)?
                }
                Err(error) => return Err(Self::membership_resource_error(error)),
            };
        if retained_history {
            history_clocks.adopt();
        }
        let retirement = match changed_retirement {
            ChangedOwnerRetirement::VacantOrSharedShellInline => {
                MembershipRetirement::Inline(retired_buffer(removals.len())?)
            }
            ChangedOwnerRetirement::OutsideGuard => {
                let capacity = removals
                    .len()
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                MembershipRetirement::Outside(retired_buffer(capacity)?)
            }
        };
        let scheduler = self
            .scheduler
            .plan_replace(existing.as_ref(), Some(&after), None)?;
        let dependency =
            self.plan_membership_dependency_delta(existing.as_ref(), &after, &removals, sequence)?;
        let owners = self.plan_membership_owner_derivations(
            &key,
            existing.as_ref(),
            &after,
            &removals,
            sequence,
        )?;
        Ok(MembershipDelta {
            changed_key: key.clone(),
            changed_after: after,
            retirement,
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
        &mut self,
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
            let OwnedTx::Accepted(removed) = owner else {
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
        let publication = self
            .effects
            .build_publication(policy, effects)
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.effects
            .plan_publication(&publication, sequence)
            .map_err(PlanError::from)
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

    fn plan_preaccepted_terminalization(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(_) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let dependency_control = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self.effects.plan_publication(publication, sequence)?;
        self.prepare_entry_delta_with_controls(
            EntryTransition::Remove {
                key: key.clone(),
                before: existing,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency_and_effect(dependency_control, effect),
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::PreacceptedTerminalizationOwner,
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
        hashes.extend(
            self.entries
                .iter()
                .filter(|(_, owner)| {
                    matches!(
                        owner,
                        OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)
                    )
                })
                .map(|(hash, _)| hash.clone()),
        );
        hashes.sort_unstable();
        let generation = next_generation(self.generation)?;
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let effect = self.effects.plan_generation_reset(sequence)?;
        let removal = self.plan_owner_removal_batch(OwnerRemovalKeys::new(hashes)?, sequence)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::ClearPipeline(ClearPipelineDelta {
                generation,
                removal,
                effect,
                clocks: clocks.finish(),
            }),
            #[cfg(any(test, feature = "internal"))]
            origin: AuthorityApplyOrigin::ClearPipeline,
        })
    }

    /// Replace the complete pool generation and install exactly the validated
    /// next chain view. This is an O(1) authority swap; active capabilities
    /// are invalidated by missing ownership, not by a drain protocol.
    pub(super) fn plan_clear_pool(
        &mut self,
        tip_hash: ckb_types::packed::Byte32,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let chain_view = ChainViewId::new(next_chain_revision(self.chain_revision())?, tip_hash);
        let generation = next_generation(self.generation)?;
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let effect = self.effects.plan_generation_reset(sequence)?;
        let sources = self.source_versions.plan_generation_replacement(sequence);
        let fresh = FreshGeneration::empty(&self.resources, &self.scheduler);
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::ClearPool(ClearPoolDelta {
                generation,
                chain_view,
                fresh,
                sources,
                effect,
                clocks: clocks.finish(),
            }),
            #[cfg(any(test, feature = "internal"))]
            origin: AuthorityApplyOrigin::ClearPool,
        })
    }

    fn plan_administrative_removal(
        &mut self,
        hashes: Vec<RawTxHash>,
        plan: AdminPlan,
    ) -> Result<PreparedApply<'_>, PlanError> {
        #[cfg(any(test, feature = "internal"))]
        let origin = plan.apply_origin();
        let delta = self.compile_administrative_removal(hashes, plan)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Admin(delta),
            #[cfg(any(test, feature = "internal"))]
            origin,
        })
    }

    /// Compile the complete administrative transition into an owned delta.
    /// The authority borrow becomes linear only when the caller wraps this
    /// delta in `PreparedApply`; domain planners may therefore adjudicate a
    /// bounded alternative without creating a nested transaction or keeping
    /// an earlier mutable borrow alive.
    fn compile_administrative_removal(
        &mut self,
        hashes: Vec<RawTxHash>,
        plan: AdminPlan,
    ) -> Result<AdminDelta, PlanError> {
        self.effects.ensure_open()?;
        let mut hashes = OwnerRemovalKeys::new(hashes)?;
        for hash in hashes.iter() {
            let owner = self
                .entries
                .get(hash)
                .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?;
            match &plan {
                AdminPlan::PeerRevocation { revocation, .. } => {
                    let OwnedTx::PreAccepted(entry) = owner else {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    };
                    if entry.source.ingress_peer() != Some(revocation.peer()) {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    }
                }
                AdminPlan::RemoteExpiry { cutoff } => match owner {
                    OwnedTx::PreAccepted(entry) => match entry.source {
                        PreAcceptedSource::Remote(remote)
                            if remote.residency.expires_at <= *cutoff => {}
                        PreAcceptedSource::Remote(_)
                        | PreAcceptedSource::Proposal { .. }
                        | PreAcceptedSource::Recovery(_) => {
                            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                        }
                    },
                    OwnedTx::Accepted(_) | OwnedTx::ReplacementHistory(_) => {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    }
                },
                AdminPlan::LocalRemoval { root } => {
                    if !hashes.iter().any(|hash| hash == root) {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                }
                AdminPlan::AcceptedExpiry { .. } => {
                    if !matches!(owner, OwnedTx::Accepted(_)) {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                }
            }
        }

        match &plan {
            AdminPlan::LocalRemoval { root } => {
                let root_owner = self
                    .entries
                    .get(root)
                    .ok_or(PlanError::Stale(StalePlan::Missing))?;
                match root_owner {
                    OwnedTx::Accepted(_) => {
                        if hashes.iter().any(|hash| {
                            !matches!(self.entries.get(hash), Some(OwnedTx::Accepted(_)))
                        }) {
                            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                        }
                    }
                    OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {
                        if &*hashes != std::slice::from_ref(root) {
                            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                        }
                    }
                }
            }
            AdminPlan::AcceptedExpiry { root, cutoff } => {
                let Some(OwnedTx::Accepted(entry)) = self.entries.get(root) else {
                    return Err(PlanError::Stale(StalePlan::Phase));
                };
                if entry.accepted_at > *cutoff {
                    return Err(PlanError::Stale(StalePlan::SourceVersion));
                }
            }
            AdminPlan::PeerRevocation { .. } | AdminPlan::RemoteExpiry { .. } => {}
        }

        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let (control, effect) = match plan {
            AdminPlan::PeerRevocation { marker, revocation } => {
                let mut effects = Vec::new();
                effects
                    .try_reserve(1)
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                effects.push(CommittedEffect::PeerCohortRevoked(revocation));
                let publication = self
                    .effects
                    .build_publication(EffectPolicy::CriticalDetail, effects)
                    .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
                let effect = self.effects.plan_publication(&publication, sequence)?;
                (AdminControl::PeerRevocation { marker }, effect)
            }
            AdminPlan::RemoteExpiry { .. } => {
                let mut effects = Vec::new();
                effects
                    .try_reserve(hashes.len())
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                for hash in hashes.iter() {
                    effects.push(CommittedEffect::RemoteExpired {
                        tx_hash: hash.clone(),
                    });
                }
                let prefix = self
                    .effects
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
                hashes.truncate(selected.get());
                let effect = self.effects.plan_publication(&publication, sequence)?;
                (AdminControl::None, effect)
            }
            AdminPlan::LocalRemoval { root } => {
                let root_owner = self
                    .entries
                    .get(&root)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                let effect = match CommittedRemoteIngressRelease::removed_owner(root, root_owner) {
                    Some(release) => {
                        let publication = self
                            .effects
                            .build_single_publication(
                                EffectPolicy::Trusted,
                                CommittedEffect::RemoteIngressReleased(release),
                            )
                            .map_err(PlanError::from)?;
                        self.effects.plan_publication(&publication, sequence)?
                    }
                    None => EffectDelta::default(),
                };
                (AdminControl::None, effect)
            }
            AdminPlan::AcceptedExpiry { .. } => {
                let mut effects = Vec::new();
                effects
                    .try_reserve(hashes.len())
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                for hash in hashes.iter() {
                    let owner = self
                        .entries
                        .get(hash)
                        .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                    let OwnedTx::Accepted(entry) = owner else {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    };
                    effects.push(CommittedEffect::Rejected(CommittedRejection::Expired {
                        entry: self.committed_entry_before(entry)?,
                    }));
                }
                let publication = self
                    .effects
                    .build_publication(EffectPolicy::CriticalDetail, effects)
                    .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
                let effect = self.effects.plan_publication(&publication, sequence)?;
                (AdminControl::None, effect)
            }
        };

        let removal = self.plan_owner_removal_batch(hashes, sequence)?;
        Ok(AdminDelta {
            control,
            removal,
            effect,
            clocks: clocks.finish(),
        })
    }

    fn plan_owner_removal_batch(
        &mut self,
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
                .filter(|hash| matches!(self.entries.get(*hash), Some(OwnedTx::Accepted(_))))
                .cloned(),
        );
        let accepted_removals = AcceptedRemovalSet::try_from_vec(accepted_removals)?;
        if hashes.iter().any(|hash| !self.entries.contains_key(hash)) {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        let membership = self.prepare_chain_projection(&accepted_removals, &HashMap::new())?;
        let available = self.collect_released_administrative_inputs(&accepted_removals)?;

        let mut owner_refs = Vec::new();
        owner_refs
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in hashes.iter() {
            owner_refs.push(
                self.entries
                    .get(hash)
                    .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?,
            );
        }
        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        resource_changes.extend(
            hashes
                .iter()
                .zip(&owner_refs)
                .map(|(hash, owner)| (hash.clone(), Some(owner.charge_record()), None)),
        );
        let resources = self.resources.plan_batch(resource_changes)?;
        let scheduler = self
            .scheduler
            .plan_batch(owner_refs.iter().copied().map(|owner| (Some(owner), None)))?;
        let lost = self
            .collect_dependency_loss_keys(owner_refs.iter().copied())?
            .keys;
        let dependency_control = self
            .dependencies
            .plan_events(available, lost, DependencyCut(sequence))?
            .unwrap_or_default();
        let dependency = self
            .dependencies
            .plan_replacements(owner_refs.iter().copied().map(|owner| (Some(owner), None)))?
            .with_control(dependency_control);
        let sources = self.source_versions.plan_replacements(
            owner_refs.iter().copied().map(|owner| (Some(owner), None)),
            sequence,
        );
        let indexes = self.indexes.plan_replacements(
            hashes
                .iter()
                .zip(&owner_refs)
                .map(|(hash, owner)| (hash, Some(*owner), None)),
        )?;
        let owners = DerivedOwnerDelta { indexes, sources };
        let retired = retired_buffer(hashes.len())?;
        Ok(OwnerRemovalBatch {
            hashes: hashes.into_inner(),
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
    fn plan_peer_revocation(
        &mut self,
        peer: ckb_network::PeerIndex,
        tx_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.compile_peer_revocation(peer, tx_hash, reason)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Admin(delta),
            #[cfg(any(test, feature = "internal"))]
            origin: AuthorityApplyOrigin::PeerRevocation,
        })
    }

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
        let marker = self.peer_bans.plan_record(peer, Instant::now())?;
        let revocation = CommittedPeerCohortRevocation::malformed(marker.lease(), tx_hash, reason)
            .ok_or(PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.compile_administrative_removal(
            hashes,
            AdminPlan::PeerRevocation { marker, revocation },
        )
    }

    /// Remove one explicit owner. Accepted roots include their complete
    /// descendant closure; transient/history roots remove only the named
    /// owner and publish dependency loss for bounded maintenance.
    pub(super) fn plan_local_removal(
        &mut self,
        root: &RawTxHash,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let Some(owner) = self.entries.get(root) else {
            return Ok(None);
        };
        let hashes = match owner {
            OwnedTx::Accepted(_) => self.administrative_descendant_closure(root)?,
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {
                let mut hashes = Vec::new();
                hashes
                    .try_reserve_exact(1)
                    .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
                hashes.push(root.clone());
                hashes
            }
        };
        self.plan_administrative_removal(hashes, AdminPlan::LocalRemoval { root: root.clone() })
            .map(Some)
    }

    /// Expire the oldest due Accepted root and its complete descendant
    /// closure in one Apply. One root per maintenance step keeps effect and
    /// graph work bounded by the accepted component invariant.
    pub(super) fn plan_accepted_expiry(
        &mut self,
        cutoff: AcceptedAtMillis,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let Some(due) = self.indexes.due_accepted(cutoff, 1)?.pop() else {
            return Ok(None);
        };
        let Some(OwnedTx::Accepted(entry)) = self.entries.get(&due.hash) else {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        };
        if entry.accepted_at != due.accepted_at || entry.accepted_at > cutoff {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        let hashes = self.administrative_descendant_closure(&due.hash)?;
        self.plan_administrative_removal(
            hashes,
            AdminPlan::AcceptedExpiry {
                root: due.hash,
                cutoff,
            },
        )
        .map(Some)
    }

    /// Remove up to `limit` Remote owners whose retained residency lease has
    /// elapsed. Computing owners are removed with their exact charge; a later
    /// move-only completion observes stale ownership. Expiry never waits for a
    /// worker or expands the due-prefix scan by the active population.
    pub(super) fn plan_remote_expiry(
        &mut self,
        cutoff: RemoteDeadline,
        limit: NonZeroUsize,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let due = self.indexes.due_remote(cutoff, limit.get())?;
        if due.is_empty() {
            return Ok(None);
        }
        let mut hashes = Vec::new();
        hashes
            .try_reserve(limit.get())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for candidate in due {
            let owner = self
                .entries
                .get(&candidate.hash)
                .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?;
            let OwnedTx::PreAccepted(entry) = owner else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            let PreAcceptedSource::Remote(remote) = entry.source else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            if remote.residency.expires_at != candidate.expires_at {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
            hashes.push(candidate.hash);
        }
        if hashes.is_empty() {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        self.plan_administrative_removal(hashes, AdminPlan::RemoteExpiry { cutoff })
            .map(Some)
    }

    pub(super) fn effect_publication_observation(&self) -> EffectPublicationObservation {
        self.effects.publication_observation()
    }

    pub(super) fn apply_effect_settlement(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<EffectSettlementCommit, EffectSettlementFailure> {
        let plan = match self.effects.plan_settlement(&settlement) {
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
            return Ok(EffectSettlementCommit::Superseded(settlement));
        };
        let clocks = match ApplyClockReservation::begin(self.clocks) {
            Ok(clocks) => clocks,
            Err(_) => {
                return Err(EffectSettlementFailure {
                    error: EffectSettlementError::CounterExhausted,
                    settlement,
                });
            }
        };
        Ok(EffectSettlementCommit::Applied(
            self.prepared_effect_only(
                effect,
                clocks,
                #[cfg(any(test, feature = "internal"))]
                AuthorityApplyOrigin::EffectSettlement,
            )
            .apply(),
        ))
    }

    pub(super) fn plan_effect_close(&mut self) -> Result<PreparedApply<'_>, EffectCloseError> {
        if self.resources.preaccepted().active_work != 0 {
            return Err(EffectCloseError::ActiveWork);
        }
        let effect = self.effects.plan_close().map_err(|error| match error {
            EffectClosePlanError::AlreadyClosed => EffectCloseError::AlreadyClosed,
        })?;
        let clocks = ApplyClockReservation::begin(self.clocks)
            .map_err(|_| EffectCloseError::CounterExhausted)?;
        Ok(self.prepared_effect_only(
            effect,
            clocks,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::EffectClose,
        ))
    }

    pub(super) fn effects_closed_and_drained(&self) -> bool {
        self.effects.is_closed_and_drained()
    }

    pub(super) fn pending_recent_reject(&self, hash: &RawTxHash) -> Option<PendingRecentReject> {
        self.effects.pending_recent_reject(hash)
    }

    fn prepared_effect_only(
        &mut self,
        effect: EffectDelta,
        clocks: ApplyClockReservation,
        #[cfg(any(test, feature = "internal"))] origin: AuthorityApplyOrigin,
    ) -> PreparedApply<'_> {
        PreparedApply {
            authority: self,
            delta: AuthorityDelta::Effect(EffectOnlyDelta {
                effect,
                clocks: clocks.finish(),
            }),
            #[cfg(any(test, feature = "internal"))]
            origin,
        }
    }

    pub(super) fn plan_dependency_maintenance(
        &mut self,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        self.effects.ensure_open()?;
        let Some(ticket) = self.dependencies.next_maintenance()? else {
            return Ok(None);
        };
        let hash = ticket.hash().cloned();
        let action = ticket.action(
            &self.dependencies,
            hash.as_ref().and_then(|hash| self.entries.get(hash)),
        )?;
        let control = self.dependencies.plan_maintenance(ticket)?.into_control();
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        match action {
            DependencyMaintenanceAction::Advance => {
                return Ok(Some(PreparedApply {
                    authority: self,
                    delta: AuthorityDelta::Dependency(DependencyOnlyDelta {
                        control,
                        clocks: clocks.finish(),
                    }),
                    #[cfg(any(test, feature = "internal"))]
                    origin: AuthorityApplyOrigin::DependencyAdvance,
                }));
            }
            DependencyMaintenanceAction::Requeue => {}
        }

        let hash = hash.ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let existing = self
            .entries
            .get(&hash)
            .cloned()
            .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
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
        self.prepare_entry_delta_with_controls(
            EntryTransition::Replace {
                key: hash,
                before: existing,
                after,
                retirement: EntryReplacementRetirement::SharedShellInline,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency(control),
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::DependencyMaintenanceOwner,
        )
        .map(Some)
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

    /// Discharge checked-out work after allocator pressure made its original
    /// result uncommittable. This Plan intentionally has no effect, index,
    /// dependency, primary-owner, or peer-row insertion path. Its closed error
    /// type therefore cannot turn resource pressure into an unbounded retry.
    pub(super) fn apply_compute_cancellation(
        &mut self,
        cancellation: ComputeCancellation,
    ) -> Result<CommittedDelta, ComputeCancellationError> {
        let token = cancellation.token;
        let existing = self
            .entries
            .get(&token.hash)
            .ok_or(ComputeCancellationError::Obsolete(StalePlan::Missing))?
            .clone();
        if existing.record().version != token.version {
            return Err(ComputeCancellationError::Obsolete(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(ComputeCancellationError::Obsolete(StalePlan::Phase));
        };
        let PreAcceptedPhase::Computing(_) = &preaccepted.phase else {
            return Err(ComputeCancellationError::Obsolete(StalePlan::Phase));
        };
        if preaccepted.charge.active_work != 1 {
            return Err(ComputeCancellationError::Fault(
                AuthorityFault::ResourceProjection,
            ));
        }
        if self.effects.is_closed() {
            return Err(ComputeCancellationError::EffectClosed);
        }

        let clocks = ApplyClockReservation::begin(self.clocks)
            .map_err(|_| ComputeCancellationError::Fault(AuthorityFault::CounterExhausted))?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks
            .replacement()
            .map_err(|_| ComputeCancellationError::Fault(AuthorityFault::CounterExhausted))?;
        let after = existing
            .with_preaccepted_phase(
                PreAcceptedPhase::Queued(QueuedWork::Resolve),
                version,
                preaccepted.original_charge(),
            )
            .map_err(|_| ComputeCancellationError::Fault(AuthorityFault::MembershipProjection))?;
        let resource = self
            .resources
            .plan_compute_release(
                token.hash.clone(),
                existing.charge_record(),
                after.charge_record(),
            )
            .map_err(|error| match error {
                ComputeReleaseError::Arithmetic | ComputeReleaseError::Projection => {
                    ComputeCancellationError::Fault(AuthorityFault::ResourceProjection)
                }
            })?;
        let scheduler = self
            .scheduler
            .plan_replace(Some(&existing), Some(&after), None)
            .map_err(|_| ComputeCancellationError::Fault(AuthorityFault::SchedulerProjection))?;
        let dependency = self
            .dependencies
            .plan_stable_replace(&existing, &after)
            .map_err(|error| match error {
                StableDependencyError::Projection => {
                    ComputeCancellationError::Fault(AuthorityFault::DependencyProjection)
                }
            })?;
        let indexes = self
            .indexes
            .plan_stable_replace(&token.hash, &existing, &after)
            .map_err(|error| match error {
                StableIndexError::Projection => {
                    ComputeCancellationError::Fault(AuthorityFault::IndexProjection)
                }
            })?;
        let sources = self
            .source_versions
            .plan_replacements(std::iter::once((Some(&existing), Some(&after))), sequence);
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Entry(EntryDelta {
                key: token.hash,
                after: Some(after),
                owners: DerivedOwnerDelta { indexes, sources },
                retirement: EntryRetirement::InlineDrop,
                resource,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                clocks: clocks.finish(),
            }),
            #[cfg(any(test, feature = "internal"))]
            origin: AuthorityApplyOrigin::ComputeCancellationOwner,
        }
        .apply())
    }

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
    /// projections. They are the closed commutative domain consumed by the
    /// compute exchange. Results which may publish an effect, revoke a peer,
    /// or terminalize dependency owners remain `NonLocal` and retain their
    /// exact move-only evidence for the existing cohort planner.
    fn classify_settlement(
        &self,
        preaccepted: &PreAcceptedEntry,
        active: &super::state::ActiveWork,
        next: SettlementNext,
    ) -> Result<SettlementClassification, PlanError> {
        let raw_charge = preaccepted.original_charge();
        let dependency_cut = active.dependency_cut;
        if !self
            .dependencies
            .proof_is_current(preaccepted.dependencies(), dependency_cut)
        {
            return Ok(SettlementClassification::OwnerLocal(OwnerLocalSettlement {
                phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                charge: raw_charge,
            }));
        }

        let chain_state_is_current = self.chain_view.has_same_chain_state(&active.chain_view);
        let local = |phase, charge| {
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
                    return Ok(local(
                        PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        raw_charge,
                    ));
                }
                let dependencies = resolved.payload().dependencies().clone();
                let retained_charge = active
                    .grant
                    .retained_charge(
                        resolved.payload().resolved_resident_bytes(),
                        dependencies.len(),
                    )
                    .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                if self.dependencies.resolution_is_current(
                    preaccepted.dependencies(),
                    &dependencies,
                    dependency_cut,
                ) {
                    Ok(local(
                        PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
                        retained_charge,
                    ))
                } else {
                    Ok(local(
                        PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        raw_charge,
                    ))
                }
            }
            SettlementNext::Waiting(missing) => {
                let dependencies = missing.dependencies().clone();
                if chain_state_is_current
                    && self.dependencies.missing_result_is_current(
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
                    Ok(local(
                        PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        raw_charge,
                    ))
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
                if self.dependencies.resolution_is_current(
                    preaccepted.dependencies(),
                    &dependencies,
                    dependency_cut,
                ) {
                    Ok(local(PreAcceptedPhase::Ready(verified), retained_charge))
                } else {
                    Ok(local(
                        PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        raw_charge,
                    ))
                }
            }
            SettlementNext::Rejected(rejection) => {
                if chain_state_is_current || rejection.remains_valid_after_chain_change() {
                    Ok(SettlementClassification::NonLocal(
                        NonLocalSettlement::Rejected(rejection),
                    ))
                } else {
                    Ok(local(
                        PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        raw_charge,
                    ))
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
                            Ok(local(
                                PreAcceptedPhase::Queued(QueuedWork::Resolve),
                                raw_charge,
                            ))
                        }
                    }
                    PayloadPolicyEvolution::RemoteToTrusted => {
                        if !chain_state_is_current {
                            return Ok(local(
                                PreAcceptedPhase::Queued(QueuedWork::Resolve),
                                raw_charge,
                            ));
                        }
                        let dependencies = resolved.payload().dependencies().clone();
                        let retained_charge = active
                            .grant
                            .retained_charge(
                                resolved.payload().resolved_resident_bytes(),
                                dependencies.len(),
                            )
                            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
                        if self.dependencies.resolution_is_current(
                            preaccepted.dependencies(),
                            &dependencies,
                            dependency_cut,
                        ) {
                            Ok(local(
                                PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
                                retained_charge,
                            ))
                        } else {
                            Ok(local(
                                PreAcceptedPhase::Queued(QueuedWork::Resolve),
                                raw_charge,
                            ))
                        }
                    }
                    PayloadPolicyEvolution::Invalid => {
                        Err(PlanError::Fault(AuthorityFault::MembershipProjection))
                    }
                }
            }
            SettlementNext::Retry => Ok(local(
                PreAcceptedPhase::Queued(QueuedWork::Resolve),
                raw_charge,
            )),
        }
    }

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
        let disposition = match self.classify_settlement(preaccepted, active, next)? {
            SettlementClassification::OwnerLocal(OwnerLocalSettlement { phase, charge }) => {
                SettlementDisposition::Retain { phase, charge }
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
                                self.effects.build_single_publication(
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
        let (phase, retained_charge, resource) = match self.resources.plan_replace(
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
        let clocks = ApplyClockReservation::begin(self.clocks).map_err(PlanError::from)?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement().map_err(PlanError::from)?;
        let after = existing
            .with_preaccepted_phase(phase, version, retained_charge)
            .map_err(PlanError::Stale)?;
        let effect = publication
            .as_ref()
            .map_or_else(
                || Ok(EffectDelta::default()),
                |publication| self.effects.plan_publication(publication, sequence),
            )
            .map_err(PlanError::from)?;
        self.prepare_entry_delta_with_controls(
            EntryTransition::Replace {
                key: token.hash.clone(),
                before: existing,
                after,
                retirement: EntryReplacementRetirement::SharedShellInline,
            },
            clocks.finish(),
            sequence,
            TransitionControls::effect(effect),
            Some(resource),
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::ComputeSettlementOwner,
        )
        .map_err(PrepareSettlementError::from)
    }

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
        let audience = RejectionAudience::from_source(preaccepted.source);
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
        let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        let publication = self
            .effects
            .build_single_publication(
                policy,
                CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience,
                    reason,
                }),
            )
            .map_err(PlanError::from)?;
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let dependency = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self.effects.plan_publication(&publication, sequence)?;
        let key = preaccepted.record.identity.raw.clone();
        self.prepare_entry_delta_with_controls(
            EntryTransition::Remove {
                key,
                before: existing,
            },
            clocks.finish(),
            sequence,
            TransitionControls::dependency_and_effect(dependency, effect),
            None,
            #[cfg(any(test, feature = "internal"))]
            AuthorityApplyOrigin::ComputeRejectionOwner,
        )
    }

    fn prepare_entry_delta_with_controls(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        controls: TransitionControls,
        explicit_resources: Option<ResourcePlan>,
        #[cfg(any(test, feature = "internal"))] origin: AuthorityApplyOrigin,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.ensure_open()?;
        let TransitionControls {
            dependency: dependency_control,
            effect,
        } = controls;
        let (key, expected, after, primary_insertions, retirement) = match transition {
            EntryTransition::Insert { key, after } => {
                (key, None, Some(after), 1, EntryRetirement::InlineDrop)
            }
            EntryTransition::Replace {
                key,
                before,
                after,
                retirement,
            } => {
                let retirement = match retirement {
                    EntryReplacementRetirement::SharedShellInline => EntryRetirement::InlineDrop,
                    EntryReplacementRetirement::OutsideGuard => {
                        EntryRetirement::Outside(retired_buffer(1)?)
                    }
                };
                (key, Some(before), Some(after), 0, retirement)
            }
            EntryTransition::Remove { key, before } => (
                key,
                Some(before),
                None,
                0,
                EntryRetirement::Outside(retired_buffer(1)?),
            ),
        };
        let expected_charge = expected.as_ref().map(OwnedTx::charge_record);
        let after_charge = after.as_ref().map(OwnedTx::charge_record);
        self.reserve_primary_owner_insertions(primary_insertions)?;
        let resource = match explicit_resources {
            Some(resources) => resources,
            None => self
                .resources
                .plan_replace(key.clone(), expected_charge, after_charge)?,
        };
        let scheduler = self
            .scheduler
            .plan_replace(expected.as_ref(), after.as_ref(), None)?;
        let dependency = self
            .dependencies
            .plan_replace(expected.as_ref(), after.as_ref())?
            .with_control(dependency_control);
        let sources = self.source_versions.plan_replacements(
            std::iter::once((expected.as_ref(), after.as_ref())),
            sequence,
        );
        let indexes = self
            .indexes
            .plan_replace(&key, expected.as_ref(), after.as_ref())?;
        let owners = DerivedOwnerDelta { indexes, sources };
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Entry(EntryDelta {
                key,
                after,
                owners,
                retirement,
                resource,
                scheduler,
                dependency,
                effect,
                clocks,
            }),
            #[cfg(any(test, feature = "internal"))]
            origin,
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
