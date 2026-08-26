mod apply_seal;
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
    DirectAdmissionReceipt, DirectAdmissionRejection, FinalAdmissionPreparation,
    FinalAdmissionReceipt, FinalAdmissionRejection, FinalAdmissionRetry, FinalAdmissionSubject,
    FinalAdmissionWork,
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
    ChargeProjection, ChargeRecord, ChargedAdmission, ComputeGrant, ComputeReleaseError,
    ResourceBatchPlan, ResourceError, ResourceLedger, ResourceLimits, ResourcePlan, ResourceVector,
};
use super::scheduler::{
    FairFrontier, QueueLane, SchedulerBatchDelta, SchedulerDelta, SchedulerError,
    SchedulerWakeProjection, VerifyOrder,
};
#[cfg(test)]
use super::shard::ShardWriteSupport;
use super::shard::{ShardProposedCountPlanError, ShardedOwnerMap, ShardedOwnerReadGuard};
#[cfg(test)]
use super::source::AuthoritySourceVersionSnapshot;
use super::source::{AuthoritySourceVersions, PoolTemplateVersions, SourceVersionDelta};
use super::state::{
    AcceptedAtMillis, AcceptedEntry, AcceptedProvenance, AdmissionBasis, ApplySequence, Arrival,
    AsyncProcessStart, AuthorityClockBank, AuthorityClocks, ChainRevision, ChainViewId,
    DependencyCut, DependencyKey, DependencyOrigin, EntryVersion, KnownDependencies,
    MissingDependencies, OwnedTx, PayloadPolicy, PayloadPolicyEvolution, PoolGeneration,
    PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource, ProposalBase, QueuedWork, RawTxHash,
    RemoteDeadline, ReplacementHistoryEntry, ReplacementHistoryError, ResolvedFacts, TxRecord,
    ValidatedAdmission, VerifiedFacts,
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
#[cfg(test)]
pub(in crate::authority) use membership::StatusCounts;
pub(in crate::authority) use membership::{
    AcceptedOrderKey, AncestorAggregate, DescendantAggregate, EvictionOrderKey,
    MembershipProjection,
};
use membership::{AcceptedRemovalSet, MembershipRemoval, PreparedMembership, ProjectionDelta};
pub(in crate::authority) use settlement::{
    CoupledSettlementContinuation, IndependentCandidate, SettlementBatch, SettlementPlan,
};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::{sync::Arc, time::Instant};

pub(in crate::authority) use apply_seal::TxPoolAuthority;
use apply_seal::{ApplyToken, OwnerResourceUpdate, PreparedOwnerResourceDelta};

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
            effects: self.effects.operational_usage(),
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
            self.chain_view.clone(),
            &self.entries,
            &self.indexes,
            &self.membership,
            self.membership_config,
            self.source_versions.relay_parents(),
            &self.source_versions,
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
        let owners = self.entries.read_all();
        owners.template_sources(self.source_versions.template())
    }

    fn wake_projection(&self) -> AuthorityWakeProjection {
        AuthorityWakeProjection {
            scheduler: self.scheduler.wake_projection(),
            // Ordinary Apply supplies the exact net release bit from its
            // already-reserved resource delta. Keeping a neutral sentinel in
            // this O(1) projection avoids two fixed 64-shard aggregate scans
            // while preserving the existing before/after wake relation.
            active_work: 0,
            dependency_maintenance: self.dependencies.maintenance_pending(),
            effects: self.effects.wake_projection(),
            template: self.source_versions.template(),
        }
    }

    fn wake_projection_for_accepted_removal(
        &self,
        template_selection_changed: bool,
    ) -> AuthorityWakeProjection {
        let template_marker = ApplySequence(u128::from(template_selection_changed));
        AuthorityWakeProjection {
            scheduler: self.scheduler.wake_projection(),
            // Accepted removal cannot change preaccepted active work. Keeping
            // one equal sentinel on both sides avoids a 64-shard total scan in
            // the disjoint write route without fabricating a wake edge.
            active_work: 0,
            dependency_maintenance: self.dependencies.maintenance_pending(),
            effects: self.effects.wake_projection(),
            template: PoolTemplateVersions {
                proposals: template_marker,
                transactions: template_marker,
                chain: ApplySequence(0),
            },
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
    retired: RetiredOwners,
    retired_effect: Option<Arc<EffectBatch>>,
    retired_generation: Option<RetiredGeneration>,
    wake: AuthorityWakeTransition,
}

struct ApplyRetirement {
    async_process_observations: AsyncProcessObservations,
    removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
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
    entries: ShardedOwnerMap,
    _indexes: AuthorityIndexes,
    _resources: ResourceLedger,
    _membership: MembershipProjection,
    _scheduler: FairFrontier,
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
    retired: RetiredOwners,
    resource: ResourcePlan,
    scheduler: SchedulerDelta,
    dependency: DependencyDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
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

/// One mechanically commuting membership batch. The ordinary disjoint
/// admission run and the strictly proven leaf-RBF cohort share this exact
/// delta and Apply; policy remains in the canonical membership evaluator.
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
    removals: Vec<MembershipRemoval>,
    retired: RetiredOwners,
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
    entries: ShardedOwnerMap,
    indexes: AuthorityIndexes,
    resources: ResourceLedger,
    membership: MembershipProjection,
    scheduler: FairFrontier,
    dependencies: DependencyFrontier,
}

impl FreshGeneration {
    fn empty(
        resources: &ResourceLedger,
        scheduler: &FairFrontier,
        entries: &ShardedOwnerMap,
    ) -> Self {
        let entries = ShardedOwnerMap::new(entries.router());
        let indexes = AuthorityIndexes::for_entries(&entries);
        let membership = MembershipProjection::for_entries(&entries);
        let dependencies = DependencyFrontier::for_entries(&entries);
        Self {
            entries,
            indexes,
            resources: ResourceLedger::new(resources.limits()),
            membership,
            scheduler: FairFrontier::new(scheduler.verify_order()),
            dependencies,
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
    expected_versions: Vec<EntryVersion>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    membership: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    retired: RetiredOwners,
}

#[derive(Clone, Copy)]
enum OwnerRemovalSourceScope {
    Complete,
    TemplateSelectionOnly,
}

#[cfg(test)]
impl OwnerRemovalBatch {
    fn shard_support(
        &self,
    ) -> (
        super::shard_support::AuthorityShardSupport,
        super::shard_support::ExclusiveSupport,
    ) {
        let mut support = super::shard_support::AuthorityShardSupport::default();
        let mut exclusive = super::shard_support::ExclusiveSupport::default();
        for hash in &self.hashes {
            support.insert(b"owner-resource/owner", hash);
        }
        self.owners.indexes.extend_shard_support(&mut support);
        self.resources
            .extend_shard_support(&mut support, &mut exclusive);
        self.membership
            .extend_shard_support(&mut support, &mut exclusive);
        self.scheduler
            .extend_shard_support(&mut support, &mut exclusive);
        self.dependency
            .extend_shard_support(&mut support, &mut exclusive);
        (support, exclusive)
    }
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

impl AuthorityDelta {
    fn releases_preaccepted_active_work(&self) -> bool {
        match self {
            Self::Entry(delta) => delta.resource.releases_preaccepted_active_work(),
            Self::ComputeExchange(delta) => delta.releases_preaccepted_active_work(),
            Self::RetainedIngress(delta) => delta.releases_preaccepted_active_work(),
            Self::Membership(delta) => delta.resource.releases_preaccepted_active_work(),
            Self::Independent(delta) => delta.resource.releases_preaccepted_active_work(),
            Self::Dependency(_) | Self::Effect(_) => false,
            Self::ClearPipeline(delta) => {
                delta.removal.resources.releases_preaccepted_active_work()
            }
            Self::ClearPool(delta) => delta.compute_slot_released,
            Self::Admin(delta) => delta.removal.resources.releases_preaccepted_active_work(),
            Self::Chain(delta) => delta.resources.releases_preaccepted_active_work(),
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
    /// The real production discriminant is the only carrier witness.  Do not
    /// pair it with a second source label whose agreement would need tests.
    delta: AuthorityDelta,
}

pub(super) enum ConcurrentLocalRemovalFallback {
    Absent,
    RequiresExclusive,
}

pub(super) type ConcurrentLocalRemovalPlan<'authority> =
    Result<PreparedConcurrentLocalRemoval<'authority>, ConcurrentLocalRemovalFallback>;

#[must_use = "a concurrent local removal has no effect until explicitly applied"]
pub(super) struct PreparedConcurrentLocalRemoval<'authority> {
    authority: &'authority TxPoolAuthority,
    removal: OwnerRemovalBatch,
    clocks: AuthorityClocks,
}

pub(super) struct ConcurrentLocalRemovalStale;

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
        apply_seal::commit(self)
    }

    fn apply_with(self, token: &ApplyToken) -> CommittedDelta {
        let Self { authority, delta } = self;
        let compute_slot_released = delta.releases_preaccepted_active_work();
        let mut before = authority.wake_projection();
        let retirement = match delta {
            AuthorityDelta::Entry(delta) => Self::apply_entry(&mut *authority, token, delta),
            AuthorityDelta::ComputeExchange(delta) => {
                compute_exchange::apply_compute_exchange(&mut *authority, token, delta)
            }
            AuthorityDelta::RetainedIngress(delta) => {
                ingress::apply_retained_ingress(&mut *authority, token, delta)
            }
            AuthorityDelta::Membership(delta) => {
                Self::apply_membership(&mut *authority, token, delta)
            }
            AuthorityDelta::Independent(delta) => {
                Self::apply_independent(&mut *authority, token, delta)
            }
            AuthorityDelta::Dependency(delta) => {
                Self::apply_dependency(&mut *authority, token, delta)
            }
            AuthorityDelta::Effect(delta) => Self::apply_effect(&mut *authority, token, delta),
            AuthorityDelta::ClearPipeline(delta) => {
                Self::apply_clear_pipeline(&mut *authority, token, delta)
            }
            AuthorityDelta::ClearPool(delta) => {
                Self::apply_clear_pool(&mut *authority, token, delta)
            }
            AuthorityDelta::Admin(delta) => Self::apply_admin(&mut *authority, token, delta),
            AuthorityDelta::Chain(delta) => Self::apply_chain(&mut *authority, token, delta),
        };
        let after = authority.wake_projection();
        #[cfg(test)]
        assert!(
            authority.primary_projection_consistent(),
            "every committed Apply must preserve the production owner/projection relation"
        );
        before.active_work = usize::from(compute_slot_released);
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

    fn apply_entry(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: EntryDelta,
    ) -> ApplyRetirement {
        let mut retired = delta.retired;
        let proposed_counts = super::shard::ShardProposedCountPlan::default();
        let support = authority.entries.owner_resource_write_support(
            std::iter::once(&delta.key),
            &proposed_counts,
            delta.resource.shard_plan(),
        );
        let update = OwnerResourceUpdate::new(delta.key, delta.after);
        authority.commit_owner_resources(
            token,
            PreparedOwnerResourceDelta::single(update, delta.resource, support),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        authority.scheduler.apply(delta.scheduler);
        authority.dependencies.apply(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
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
        token: &ApplyToken,
        mut delta: MembershipDelta,
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
        let DerivedOwnerDelta { indexes, sources } = delta.owners;
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
            &mut retired,
        );
        let authority = authority.write(token);
        authority.source_versions.apply(sources);
        authority.scheduler.apply(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
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
        }
    }

    fn apply_independent(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        mut delta: IndependentDelta,
    ) -> ApplyRetirement {
        let proposed_counts = delta.projection.take_proposed_counts();
        let support = authority.entries.owner_resource_write_support(
            delta.updates.iter().map(|update| &update.key),
            &proposed_counts,
            delta.resource.shard_plan(),
        );
        let updates = delta
            .updates
            .into_iter()
            .map(|update| OwnerResourceUpdate::new(update.key, update.after));
        let mut retired = delta.retired;
        let DerivedOwnerDelta { indexes, sources } = delta.owners;
        authority.commit_owner_resources_indexes_membership(
            token,
            PreparedOwnerResourceDelta::batch(updates, delta.resource, proposed_counts, support),
            indexes,
            delta.projection,
            &mut retired,
        );
        let authority = authority.write(token);
        authority.source_versions.apply(sources);
        authority.scheduler.apply_batch(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: if delta.async_process_starts.is_empty() {
                AsyncProcessObservations::None
            } else {
                AsyncProcessObservations::Batch(delta.async_process_starts)
            },
            removals: delta.removals,
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_dependency(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: DependencyOnlyDelta,
    ) -> ApplyRetirement {
        let authority = authority.write(token);
        authority.dependencies.apply_control(delta.control);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect: None,
            retired_generation: None,
        }
    }

    fn apply_effect(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: EffectOnlyDelta,
    ) -> ApplyRetirement {
        let authority = authority.write(token);
        let retired_effect = authority.effects.apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_clear_pool(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: ClearPoolDelta,
    ) -> ApplyRetirement {
        let FreshGeneration {
            entries,
            indexes,
            resources,
            membership,
            scheduler,
            dependencies,
        } = delta.fresh;
        let (previous_entries, previous_resources) =
            authority.replace_owner_resources(token, entries, resources);
        let authority = authority.write(token);
        let retired_generation = RetiredGeneration {
            entries: previous_entries,
            _indexes: std::mem::replace(&mut authority.indexes, indexes),
            _resources: previous_resources,
            _membership: std::mem::replace(&mut authority.membership, membership),
            _scheduler: std::mem::replace(&mut authority.scheduler, scheduler),
            _dependencies: std::mem::replace(&mut authority.dependencies, dependencies),
        };
        authority.generation = delta.generation;
        authority.chain_view = delta.chain_view;
        authority.source_versions.apply(delta.sources);
        let retired_effect = authority.effects.apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired: RetiredOwners::default(),
            retired_effect,
            retired_generation: Some(retired_generation),
        }
    }

    fn apply_clear_pipeline(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: ClearPipelineDelta,
    ) -> ApplyRetirement {
        let retired = Self::apply_owner_removal(authority, token, delta.removal);
        let authority = authority.write(token);
        authority.generation = delta.generation;
        let retired_effect = authority.effects.apply(delta.effect);
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_admin(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        delta: AdminDelta,
    ) -> ApplyRetirement {
        let retired = Self::apply_owner_removal(authority, token, delta.removal);
        let authority = authority.write(token);
        let retired_effect = authority.effects.apply(delta.effect);
        match delta.control {
            AdminControl::PeerRevocation { marker } => authority.peer_bans.apply(marker),
            AdminControl::None => {}
        }
        let _reserved_clock_high_water = delta.clocks;
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
        token: &ApplyToken,
        removal: OwnerRemovalBatch,
    ) -> RetiredOwners {
        let OwnerRemovalBatch {
            hashes,
            expected_versions: _,
            owners,
            resources,
            mut membership,
            scheduler,
            dependency,
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
        authority.commit_owner_resources(
            token,
            PreparedOwnerResourceDelta::batch(updates, resources, proposed_counts, support),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.indexes.apply(owners.indexes);
        authority.source_versions.apply(owners.sources);
        authority.membership.apply(membership);
        authority.scheduler.apply_batch(scheduler);
        authority.dependencies.apply_batch(dependency);
        retired
    }

    fn apply_chain(
        authority: &mut TxPoolAuthority,
        token: &ApplyToken,
        mut delta: ChainDelta,
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
        authority.commit_owner_resources(
            token,
            PreparedOwnerResourceDelta::batch(updates, delta.resources, proposed_counts, support),
            &mut retired,
        );
        let authority = authority.write(token);
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        authority.membership.apply(delta.membership);
        authority.scheduler.apply_batch(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.chain_view = delta.view.clone();
        let _reserved_clock_high_water = delta.clocks;
        ApplyRetirement {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }
}

impl PreparedConcurrentLocalRemoval<'_> {
    pub(super) fn apply(self) -> Result<CommittedDelta, ConcurrentLocalRemovalStale> {
        apply_seal::commit_concurrent(self)
    }

    fn apply_with(self, token: &ApplyToken) -> Result<CommittedDelta, ConcurrentLocalRemovalStale> {
        let template_selection_changed = self.removal.owners.sources.template_selection_changed();
        let before = self.authority.wake_projection_for_accepted_removal(false);
        let retired = self
            .authority
            .commit_concurrent_owner_removal(token, self.removal)?;
        let _reserved_clock_high_water = self.clocks;
        let after = self
            .authority
            .wake_projection_for_accepted_removal(template_selection_changed);
        #[cfg(test)]
        assert!(
            self.authority.primary_projection_consistent(),
            "concurrent committed removal must preserve the production owner/projection relation"
        );
        Ok(CommittedDelta {
            async_process_observations: AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect: None,
            retired_generation: None,
            wake: AuthorityWakeTransition { before, after },
        })
    }
}

#[cfg(test)]
impl PreparedConcurrentLocalRemoval<'_> {
    pub(in crate::authority) fn physical_write_support(&self) -> ShardWriteSupport {
        let mut support = self.authority.entries.owner_resource_write_support(
            self.removal.hashes.iter(),
            self.removal.membership.proposed_count_plan(),
            self.removal.resources.shard_plan(),
        );
        support.include(
            self.removal
                .owners
                .indexes
                .sharded_write_support(&self.authority.entries),
        );
        support.include(
            self.removal
                .membership
                .sharded_write_support(&self.authority.entries),
        );
        support.include(
            self.removal
                .dependency
                .sharded_write_support(&self.authority.entries),
        );
        support
    }
}

#[cfg(test)]
impl PreparedApply<'_> {
    pub(in crate::authority) fn local_removal_shard_support(
        &self,
    ) -> Option<(
        super::shard_support::AuthorityShardSupport,
        super::shard_support::ExclusiveSupport,
    )> {
        let AuthorityDelta::Admin(delta) = &self.delta else {
            return None;
        };
        if !matches!(delta.control, AdminControl::None) {
            return None;
        }
        let (support, mut exclusive) = delta.removal.shard_support();
        exclusive.effect_log = delta.effect.has_exclusive_write();
        Some((support, exclusive))
    }

    pub(in crate::authority) fn entry_shard_support(
        &self,
    ) -> Option<(
        super::shard_support::AuthorityShardSupport,
        super::shard_support::ExclusiveSupport,
    )> {
        let AuthorityDelta::Entry(delta) = &self.delta else {
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
    ) -> Result<Self, ClockReservationError> {
        let (sequence, clocks) = bank
            .reserve_apply_owner_batch(expected, owners, insertions)
            .map_err(|_| ClockReservationError)?;
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
        let owners = self.entries.read_all();
        let capacity = removals.iter().try_fold(0usize, |total, hash| {
            let entry = match owners.get(hash) {
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
            let entry = match owners.get(hash) {
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
                if !final_owners.contains_removed(&spender) {
                    return Ok(false);
                }
            }
            ReleasedInputContext::Administrative { victim } => {
                if self.membership.spender(input) != Some(victim.clone()) {
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
        let owner = self.entries.get(&parent);
        let Some(OwnedTx::Accepted(parent)) = owner.as_deref() else {
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
        if removals.is_empty() {
            let (_entries, indexes, source_versions) = self.owner_derivation_parts();
            let indexes = indexes.plan_replace(key, existing, Some(after))?;
            let sources = source_versions
                .plan_replacements(std::iter::once((existing, Some(after))), sequence);
            return Ok(DerivedOwnerDelta { indexes, sources });
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
        let indexes = indexes.plan_replacements(changes)?;
        Ok(DerivedOwnerDelta { indexes, sources })
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
        self.resources_for_plan().plan_batch(changes)
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
            let origin_keys = dependencies.keys_for_origin(&origin);
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
        // Closing the effect authority freezes every new owner transition.
        // Reject it before allocating an owner identity or Apply stamp.
        self.effects.ensure_open()?;
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

    fn plan_single_effect(
        &mut self,
        policy: EffectPolicy,
        effect: CommittedEffect,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let publication = self
            .effects
            .build_single_publication(policy, effect)
            .map_err(PlanError::from)?;
        self.effects.preflight_publication(&publication)?;
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
            })),
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

    pub(super) fn plan_direct_admission(
        &mut self,
        receipt: DirectAdmissionReceipt,
    ) -> Result<DirectAdmissionDisposition<'_>, PlanError> {
        self.effects.ensure_open()?;
        let key = receipt.key().clone();
        let existing = self.entries.get(&key).as_deref().cloned();
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
            let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
            let sequence = clocks.sequence();
            let effect = self
                .effects_for_plan()
                .plan_publication(&publication, sequence)
                .map_err(PlanError::from)?;
            let plan = self.prepared_effect_only(effect, clocks);
            return Ok(DirectAdmissionDisposition::Duplicate(
                PreparedDirectDuplicate { key, plan },
            ));
        }
        self.validate_direct_acceptance_evidence(&receipt)?;

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
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
                let effect_clocks =
                    ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
                let effect = self
                    .effects_for_plan()
                    .plan_publication(&publication, effect_clocks.sequence())
                    .map_err(PlanError::from)?;
                let plan = self.prepared_effect_only(effect, effect_clocks);
                return Ok(DirectAdmissionDisposition::Rejected(
                    PreparedDirectRejection { reason, plan },
                ));
            }
            Err(error) => return Err(error),
        };
        let delta = self.compile_membership_delta(MembershipCompilation {
            key,
            existing,
            accepted,
            prepared,
            clocks,
            effects: MembershipEffects::Publish(EffectPolicy::Trusted),
            async_process_start,
        })?;
        Ok(DirectAdmissionDisposition::Accepted(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
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
        })?;
        Ok(InternalPlugDisposition::Insert(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
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
        let mut accepted = AcceptedEntry {
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
        let evaluation = self.evaluate_preaccepted_membership(&key, preaccepted, &accepted)?;
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
            async_process_start,
        } = compilation;
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
        self.append_admission_effects(&mut effects, accepted, removals, projection)?;
        let publication = self
            .effects
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
        self.effects.preflight_publication(publication)?;
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
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self.effects.plan_generation_reset(sequence)?;
        let sources = self.source_versions.plan_generation_replacement(sequence);
        let fresh = FreshGeneration::empty(&self.resources, &self.scheduler, &self.entries);
        let compute_slot_released = self.resources.read(&self.entries).preaccepted().active_work
            > fresh.preaccepted_active_work();
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::ClearPool(ClearPoolDelta {
                generation,
                chain_view,
                fresh,
                sources,
                effect,
                clocks: clocks.finish(),
                compute_slot_released,
            }),
        })
    }

    fn plan_administrative_removal(
        &mut self,
        hashes: Vec<RawTxHash>,
        plan: AdminPlan,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.compile_administrative_removal(hashes, plan)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Admin(delta),
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
                    let OwnedTx::PreAccepted(entry) = &*owner else {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    };
                    if entry.source.ingress_peer() != Some(revocation.peer()) {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    }
                }
                AdminPlan::RemoteExpiry { cutoff } => match &*owner {
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
                    if !matches!(&*owner, OwnedTx::Accepted(_)) {
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
                match &*root_owner {
                    OwnedTx::Accepted(_) => {
                        if hashes.iter().any(|hash| {
                            !matches!(
                                self.entries.get(hash).as_deref(),
                                Some(OwnedTx::Accepted(_))
                            )
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
                let owner = self.entries.get(root);
                let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
                    return Err(PlanError::Stale(StalePlan::Phase));
                };
                if entry.accepted_at > *cutoff {
                    return Err(PlanError::Stale(StalePlan::SourceVersion));
                }
            }
            AdminPlan::PeerRevocation { .. } | AdminPlan::RemoteExpiry { .. } => {}
        }

        let (control, publication) = match plan {
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
                (AdminControl::PeerRevocation { marker }, Some(publication))
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
                (AdminControl::None, Some(publication))
            }
            AdminPlan::LocalRemoval { root } => {
                let root_owner = self
                    .entries
                    .get(&root)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                let publication =
                    match CommittedRemoteIngressRelease::removed_owner(root, &root_owner) {
                        Some(release) => Some(
                            self.effects
                                .build_single_publication(
                                    EffectPolicy::Trusted,
                                    CommittedEffect::RemoteIngressReleased(release),
                                )
                                .map_err(PlanError::from)?,
                        ),
                        None => None,
                    };
                (AdminControl::None, publication)
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
                    let OwnedTx::Accepted(entry) = &*owner else {
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
                (AdminControl::None, Some(publication))
            }
        };

        if let Some(publication) = publication.as_ref() {
            self.effects.preflight_publication(publication)?;
        }
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
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

        let removal = self.plan_owner_removal_batch(hashes, sequence)?;
        Ok(AdminDelta {
            control,
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
        self.compile_owner_removal_batch(
            hashes,
            accepted_removals,
            available,
            sequence,
            OwnerRemovalSourceScope::Complete,
        )
    }

    /// Compile one no-scan Accepted removal after the caller proved an empty
    /// tx-pool causal neighborhood and chain-backed inputs. The ordinary
    /// administrative compiler retains its all-owner projected-final-set
    /// calculation for arbitrary descendant closures.
    fn plan_closed_accepted_owner_removal_batch(
        &self,
        hashes: OwnerRemovalKeys,
        available: Vec<DependencyKey>,
        sequence: ApplySequence,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        let accepted_removals = AcceptedRemovalSet::try_from_vec(hashes.iter().cloned().collect())?;
        self.compile_owner_removal_batch(
            hashes,
            accepted_removals,
            available,
            sequence,
            OwnerRemovalSourceScope::TemplateSelectionOnly,
        )
    }

    fn compile_owner_removal_batch(
        &self,
        hashes: OwnerRemovalKeys,
        accepted_removals: AcceptedRemovalSet,
        mut available: Vec<DependencyKey>,
        sequence: ApplySequence,
        source_scope: OwnerRemovalSourceScope,
    ) -> Result<OwnerRemovalBatch, PlanError> {
        if hashes.iter().any(|hash| !self.entries.contains_key(hash)) {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        let membership = self.prepare_chain_projection(&accepted_removals, &HashMap::new())?;

        let (
            entries,
            resources_ledger,
            scheduler_frontier,
            dependencies_frontier,
            source_versions,
            indexes,
        ) = self.concurrent_owner_removal_plan_parts();
        available.retain(|key| dependencies_frontier.has_waiter_outside(key, &hashes));
        let mut owner_snapshots = Vec::new();
        owner_snapshots
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in hashes.iter() {
            owner_snapshots.push(
                entries
                    .get(hash)
                    .as_deref()
                    .cloned()
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
                .zip(&owner_snapshots)
                .map(|(hash, owner)| (hash.clone(), owner.charge_record())),
        );
        let resources = resources_ledger.plan_removal_batch(resource_changes)?;
        let scheduler = scheduler_frontier
            .plan_batch(owner_snapshots.iter().map(|owner| (Some(owner), None)))?;
        let lost =
            Self::collect_dependency_loss_keys_from(dependencies_frontier, owner_snapshots.iter())?
                .keys;
        let dependency_control = dependencies_frontier
            .plan_events(available, lost, DependencyCut(sequence))?
            .unwrap_or_default();
        let dependency = dependencies_frontier
            .plan_replacements(owner_snapshots.iter().map(|owner| (Some(owner), None)))?
            .with_control(dependency_control);
        let replacements = || owner_snapshots.iter().map(|owner| (Some(owner), None));
        let sources = match source_scope {
            OwnerRemovalSourceScope::Complete => {
                source_versions.plan_replacements(replacements(), sequence)
            }
            OwnerRemovalSourceScope::TemplateSelectionOnly => {
                AuthoritySourceVersions::plan_template_selection_replacements(
                    replacements(),
                    sequence,
                )
            }
        };
        let indexes = indexes.plan_replacements(
            hashes
                .iter()
                .zip(&owner_snapshots)
                .map(|(hash, owner)| (hash, Some(owner), None)),
        )?;
        let owners = DerivedOwnerDelta { indexes, sources };
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
        let marker = self
            .peer_bans_for_plan()
            .plan_record(peer, Instant::now())?;
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
        let accepted = matches!(&*owner, OwnedTx::Accepted(_));
        drop(owner);
        let hashes = if accepted {
            self.administrative_descendant_closure(root)?
        } else {
            let mut hashes = Vec::new();
            hashes
                .try_reserve_exact(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            hashes.push(root.clone());
            hashes
        };
        self.plan_administrative_removal(hashes, AdminPlan::LocalRemoval { root: root.clone() })
            .map(Some)
    }

    pub(super) fn plan_concurrent_local_removal(
        &self,
        root: &RawTxHash,
    ) -> Result<ConcurrentLocalRemovalPlan<'_>, PlanError> {
        let Some(owner) = self.entries.get(root) else {
            return Ok(Err(ConcurrentLocalRemovalFallback::Absent));
        };
        let entry = match &*owner {
            OwnedTx::Accepted(entry) => entry.clone(),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => {
                return Ok(Err(ConcurrentLocalRemovalFallback::RequiresExclusive));
            }
        };
        // The projection keys below route independently from the owner key.
        // Release the point owner guard before taking any of those shard reads;
        // the concurrent Apply path later revalidates the exact owner version
        // after acquiring its complete sorted write support.  Keeping the
        // guard here would invert that order against a writer holding a lower
        // projection shard while waiting for this owner shard.
        drop(owner);
        if self
            .membership
            .parents(root)
            .is_none_or(|parents| !parents.is_empty())
            || self
                .membership
                .children(root)
                .is_none_or(|children| !children.is_empty())
            || entry
                .proof
                .payload()
                .footprint()
                .inputs()
                .iter()
                .any(|input| {
                    !entry.proof.is_chain_input(input)
                        || self.membership.spender(input).as_ref() != Some(root)
                })
            || entry
                .proof
                .payload()
                .footprint()
                .dependencies()
                .iter()
                .any(|dependency| {
                    self.membership
                        .dependency_reader_row_facts(dependency, root)
                        .is_none_or(|(reader_count, contains_root)| {
                            reader_count != 1 || !contains_root
                        })
                })
            || entry
                .proof
                .payload()
                .dependencies()
                .keys()
                .iter()
                .any(|key| {
                    self.dependencies
                        .consumers_for(key)
                        .is_none_or(|consumers| consumers.len() != 1 || !consumers.contains(root))
                })
        {
            return Ok(Err(ConcurrentLocalRemovalFallback::RequiresExclusive));
        }
        let mut available = Vec::new();
        available
            .try_reserve_exact(entry.proof.payload().footprint().inputs().len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        available.extend(
            entry
                .proof
                .payload()
                .footprint()
                .inputs()
                .iter()
                .cloned()
                .map(DependencyKey::Cell),
        );
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let removal = self.plan_closed_accepted_owner_removal_batch(
            OwnerRemovalKeys::new(vec![root.clone()])?,
            available,
            clocks.sequence(),
        )?;
        if !removal.scheduler.is_empty()
            || !removal
                .dependency
                .closed_removal_compatible(&self.dependencies)
            || !removal.owners.sources.is_template_selection_only()
        {
            return Ok(Err(ConcurrentLocalRemovalFallback::RequiresExclusive));
        }
        Ok(Ok(PreparedConcurrentLocalRemoval {
            authority: self,
            removal,
            clocks: clocks.finish(),
        }))
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
        let owner = self.entries.get(&due.hash);
        let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        };
        if entry.accepted_at != due.accepted_at || entry.accepted_at > cutoff {
            return Err(PlanError::Fault(AuthorityFault::IndexProjection));
        }
        drop(owner);
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
            let OwnedTx::PreAccepted(entry) = &*owner else {
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
        let clocks = match ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks)) {
            Ok(clocks) => clocks,
            Err(_) => {
                return Err(EffectSettlementFailure {
                    error: EffectSettlementError::CounterExhausted,
                    settlement,
                });
            }
        };
        Ok(EffectSettlementCommit::Applied(
            self.prepared_effect_only(effect, clocks).apply(),
        ))
    }

    pub(super) fn plan_effect_close(&mut self) -> Result<PreparedApply<'_>, EffectCloseError> {
        if self.resources.read(&self.entries).preaccepted().active_work != 0 {
            return Err(EffectCloseError::ActiveWork);
        }
        let effect = self.effects.plan_close().map_err(|error| match error {
            EffectClosePlanError::AlreadyClosed => EffectCloseError::AlreadyClosed,
        })?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
            .map_err(|_| EffectCloseError::CounterExhausted)?;
        Ok(self.prepared_effect_only(effect, clocks))
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
    ) -> PreparedApply<'_> {
        PreparedApply {
            authority: self,
            delta: AuthorityDelta::Effect(EffectOnlyDelta {
                effect,
                clocks: clocks.finish(),
            }),
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
        let owner = hash.as_ref().and_then(|hash| self.entries.get(hash));
        let action = ticket.action(&self.dependencies, owner.as_deref())?;
        drop(owner);
        let control = self.dependencies.plan_maintenance(ticket)?.into_control();
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        match action {
            DependencyMaintenanceAction::Advance => {
                return Ok(Some(PreparedApply {
                    authority: self,
                    delta: AuthorityDelta::Dependency(DependencyOnlyDelta {
                        control,
                        clocks: clocks.finish(),
                    }),
                }));
            }
            DependencyMaintenanceAction::Requeue => {}
        }

        let hash = hash.ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let existing = self
            .entries
            .get(&hash)
            .as_deref()
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
        self.prepare_entry_delta_with_dependency(
            EntryTransition::Replace {
                key: hash,
                before: existing,
                after,
            },
            clocks.finish(),
            sequence,
            control,
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

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))
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
            .resources_for_plan()
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
            .indexes_for_plan()
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
                retired: RetiredOwners::default(),
                resource,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                clocks: clocks.finish(),
            }),
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
        next: SettlementNext,
    ) -> Result<SettlementClassification, PlanError> {
        let raw_charge = preaccepted.original_charge();
        let dependency_cut = active.dependency_cut;
        if !self
            .dependencies
            .proof_is_current(preaccepted.dependencies(), dependency_cut)
        {
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
                if self.dependencies.resolution_is_current(
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
                if self.dependencies.resolution_is_current(
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
                        if self.dependencies.resolution_is_current(
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
        self.effects.preflight_publication(&publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let dependency = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;
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

    fn prepare_entry_delta_with_dependency(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        dependency_control: DependencyControlDelta,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            TransitionControls::dependency(dependency_control),
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
        self.effects.ensure_open()?;
        let TransitionControls {
            dependency: dependency_control,
            effect,
        } = controls;
        let (key, expected, after, primary_insertions) = match transition {
            EntryTransition::Insert { key, after } => (key, None, Some(after), 1),
            EntryTransition::Replace { key, before, after } => (key, Some(before), Some(after), 0),
            EntryTransition::Remove { key, before } => (key, Some(before), None, 0),
        };
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
        let indexes =
            self.indexes_for_plan()
                .plan_replace(&key, expected.as_ref(), after.as_ref())?;
        let owners = DerivedOwnerDelta { indexes, sources };
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Entry(EntryDelta {
                key,
                after,
                owners,
                retired: RetiredOwners::default(),
                resource,
                scheduler,
                dependency,
                effect,
                clocks,
            }),
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
