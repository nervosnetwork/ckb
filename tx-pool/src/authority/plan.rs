mod membership;
mod settlement;

use super::dependency::{
    DependencyBatchDelta, DependencyControlDelta, DependencyDelta, DependencyError,
    DependencyEvent, DependencyFrontier, DependencySnapshot,
};
use super::effect::{
    CommittedEffect, EffectBatch, EffectBuildError, EffectConfigError, EffectDelta, EffectError,
    EffectLease, EffectLimits, EffectLog, EffectObservation, EffectPolicy, EffectPublication,
    EffectSettlement, EffectSnapshot,
};
use super::resources::{
    ActiveWorkAvailability, ChargeRecord, ResourceBatchPlan, ResourceError, ResourceLedger,
    ResourceLimits, ResourcePlan, ResourceSnapshot, ResourceVector,
};
use super::scheduler::{
    CheckoutTicket, FairFrontier, QueueLane, SchedulerBatchDelta, SchedulerDelta, SchedulerError,
    SchedulerSnapshot, VerifyOrder,
};
use super::state::{
    AcceptedEntry, AcceptedStatus, AdmissionBasis, AdmissionClass, ApplySequence, Arrival,
    AuthorityClocks, ChainEpoch, ComputeAttribution, ComputeGrant, ComputedOutcome, DependencyCut,
    DependencyKey, DependencyOrigin, EntryVersion, IngressAttribution, KnownDependencies, OwnedTx,
    PayloadBlame, PreAcceptedEntry, PreAcceptedPhase, ProposalId, QueuedWork, RawTxHash,
    RejectionKind, TxIdentity, TxRecord, ValidatedAdmission, WaitCondition,
};
use super::work::{CheckedOutWork, ComputeSettlement, LeaseToken, SettlementNext, SettlementToken};
pub(in crate::authority) use membership::IndependentCoupling;
#[cfg(test)]
pub(in crate::authority) use membership::{
    DescendantAggregate, EvictionOrderKey, MembershipSnapshot,
};
use membership::{
    MembershipConfig, MembershipProjection, MembershipRemoval, PreparedMembership, ProjectionDelta,
};
pub(in crate::authority) use membership::{MembershipReject, RemovalCause, StatusCounts};
pub(in crate::authority) use settlement::{
    CandidateBatchError, IndependentCandidate, SettlementBatch, SettlementPlan,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum OwnerPhaseSnapshot {
    PreAccepted {
        phase: PreAcceptedPhase,
        dependencies: KnownDependencies,
        original_charge: ResourceVector,
    },
    Accepted {
        status: AcceptedStatus,
        verified: super::state::VerifiedFacts,
        dependencies: KnownDependencies,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnerSnapshot {
    identity: TxIdentity,
    ingress: IngressAttribution,
    blame: PayloadBlame,
    class: AdmissionClass,
    version: EntryVersion,
    arrival: Arrival,
    charge: ChargeRecord,
    phase: OwnerPhaseSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuthoritySnapshot {
    chain_epoch: ChainEpoch,
    entries: HashMap<RawTxHash, OwnerSnapshot>,
    by_proposal: HashMap<ProposalId, RawTxHash>,
    resources: ResourceSnapshot,
    membership: MembershipSnapshot,
    scheduler: SchedulerSnapshot,
    dependencies: DependencySnapshot,
    effects: EffectSnapshot,
    clocks: AuthorityClocks,
}

#[derive(Debug)]
pub(super) struct TxPoolAuthority {
    chain_epoch: ChainEpoch,
    entries: HashMap<RawTxHash, OwnedTx>,
    by_proposal: HashMap<ProposalId, RawTxHash>,
    resources: ResourceLedger,
    membership: MembershipProjection,
    scheduler: FairFrontier,
    dependencies: DependencyFrontier,
    effects: EffectLog,
    membership_config: MembershipConfig,
    clocks: AuthorityClocks,
}

impl TxPoolAuthority {
    pub(super) fn new(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effect_limits: EffectLimits,
    ) -> Result<Self, AuthorityConfigError> {
        Ok(Self::assemble(
            limits,
            verify_order,
            EffectLog::new(effect_limits).map_err(AuthorityConfigError::Effect)?,
        ))
    }

    fn assemble(limits: ResourceLimits, verify_order: VerifyOrder, effects: EffectLog) -> Self {
        Self {
            chain_epoch: ChainEpoch(0),
            entries: HashMap::new(),
            by_proposal: HashMap::new(),
            resources: ResourceLedger::new(limits),
            membership: MembershipProjection::default(),
            scheduler: FairFrontier::new(verify_order),
            dependencies: DependencyFrontier::default(),
            effects,
            membership_config: MembershipConfig::testing_default(),
            clocks: AuthorityClocks::first(),
        }
    }

    #[cfg(test)]
    pub(super) fn with_replacement(
        limits: ResourceLimits,
        minimum_rate: ckb_types::core::FeeRate,
    ) -> Self {
        let mut authority = Self::for_foundation(limits);
        authority.membership_config = MembershipConfig::testing_with_replacement(minimum_rate);
        authority
    }

    #[cfg(test)]
    pub(super) fn for_foundation(limits: ResourceLimits) -> Self {
        Self::assemble(limits, VerifyOrder::Arrival, EffectLog::for_foundation())
    }

    #[cfg(test)]
    pub(super) fn for_foundation_with_order(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
    ) -> Self {
        Self::assemble(limits, verify_order, EffectLog::for_foundation())
    }

    #[cfg(test)]
    pub(super) fn for_foundation_with_effect_limits(
        limits: ResourceLimits,
        effect_limits: EffectLimits,
    ) -> Result<Self, AuthorityConfigError> {
        Self::new(limits, VerifyOrder::Arrival, effect_limits)
    }

    pub(super) fn entry(&self, hash: &RawTxHash) -> Option<&OwnedTx> {
        self.entries.get(hash)
    }

    pub(super) fn owner_count(&self) -> usize {
        self.entries.len()
    }

    pub(super) fn charged_count(&self) -> usize {
        self.resources.charge_count()
    }

    pub(super) fn resources(&self) -> &ResourceLedger {
        &self.resources
    }

    pub(super) fn membership_counts(&self) -> StatusCounts {
        self.membership.counts()
    }

    pub(super) fn accepted_spender(
        &self,
        input: &ckb_types::packed::OutPoint,
    ) -> Option<&RawTxHash> {
        self.membership.spender(input)
    }

    pub(super) fn accepted_parents(
        &self,
        hash: &RawTxHash,
    ) -> Option<&std::collections::HashSet<RawTxHash>> {
        self.membership.parents(hash)
    }

    pub(super) fn accepted_children(
        &self,
        hash: &RawTxHash,
    ) -> Option<&std::collections::HashSet<RawTxHash>> {
        self.membership.children(hash)
    }

    pub(super) fn chain_epoch(&self) -> ChainEpoch {
        self.chain_epoch
    }

    pub(super) fn clocks(&self) -> AuthorityClocks {
        self.clocks
    }

    #[cfg(test)]
    pub(super) fn entries_for_reference(&self) -> &HashMap<RawTxHash, OwnedTx> {
        &self.entries
    }

    #[cfg(test)]
    pub(super) fn membership_snapshot_for_reference(&self) -> MembershipSnapshot {
        self.membership.snapshot()
    }

    #[cfg(test)]
    pub(super) fn ready_for_reference(&self) -> Vec<(RawTxHash, EntryVersion)> {
        self.scheduler.ready()
    }

    #[cfg(test)]
    pub(super) fn force_chain_epoch(&mut self, epoch: ChainEpoch) {
        self.chain_epoch = epoch;
    }

    #[cfg(test)]
    pub(super) fn force_next_sequence(&mut self, sequence: ApplySequence) {
        self.clocks.next_sequence = sequence;
    }

    pub(super) fn normalized_snapshot(&self) -> AuthoritySnapshot {
        let entries = self
            .entries
            .iter()
            .map(|(hash, owner)| {
                let record = owner.record();
                let phase = match owner {
                    OwnedTx::PreAccepted(entry) => OwnerPhaseSnapshot::PreAccepted {
                        phase: entry.phase.clone(),
                        dependencies: entry.dependencies().clone(),
                        original_charge: entry.original_charge(),
                    },
                    OwnedTx::Accepted(entry) => OwnerPhaseSnapshot::Accepted {
                        status: entry.status,
                        verified: entry.verified.clone(),
                        dependencies: entry.verified.payload().dependencies().clone(),
                    },
                };
                (
                    hash.clone(),
                    OwnerSnapshot {
                        identity: record.identity.clone(),
                        ingress: record.ingress,
                        blame: record.blame,
                        class: record.class,
                        version: record.version,
                        arrival: record.arrival,
                        charge: owner.charge_record(),
                        phase,
                    },
                )
            })
            .collect();
        AuthoritySnapshot {
            chain_epoch: self.chain_epoch,
            entries,
            by_proposal: self.by_proposal.clone(),
            resources: self.resources.snapshot(),
            membership: self.membership.snapshot(),
            scheduler: self.scheduler.snapshot(),
            dependencies: self.dependencies.snapshot(),
            effects: self.effects.snapshot(),
            clocks: self.clocks,
        }
    }

    pub(super) fn primary_projection_consistent(&self) -> bool {
        self.entries.len() == self.resources.charge_count()
            && self.entries.len() == self.by_proposal.len()
            && self.entries.iter().all(|(hash, owner)| {
                self.resources.charge(hash) == Some(owner.charge_record())
                    && self.by_proposal.get(&owner.record().identity.proposal) == Some(hash)
                    && &owner.record().identity.raw == hash
            })
            && self.resources.semantically_matches(&self.entries)
            && self.scheduler.semantically_matches(&self.entries)
            && self.dependencies.semantically_matches(&self.entries)
            && self
                .effects
                .semantically_consistent(self.clocks.next_sequence)
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
    ActiveWorkDrain,
    EffectCapacity,
    Allocation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StalePlan {
    Missing,
    Version,
    Phase,
    ChainEpoch,
    Lease,
    Dependency,
    EffectLease,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityFault {
    CounterExhausted,
    ResourceProjection,
    MembershipProjection,
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
/// lease capability. Callers may turn it into a deterministic cancellation,
/// or discard it only after proving the reported error makes the lease stale.
#[derive(Debug)]
#[must_use = "a failed compute settlement still owns the active lease capability"]
pub(super) struct ComputeSettlementFailure {
    error: PlanError,
    token: SettlementToken,
}

impl ComputeSettlementFailure {
    pub(super) fn error(&self) -> &PlanError {
        &self.error
    }

    pub(super) fn into_cancellation(self) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::InternalFailure),
        }
    }
}

/// Effect settlement has the same linear handoff rule as compute: planning
/// failure must return the publisher capability instead of silently leaving
/// an active effect without its only completion.
#[derive(Debug)]
#[must_use = "a failed effect settlement still owns the active effect capability"]
pub(super) struct EffectSettlementFailure {
    error: PlanError,
    settlement: EffectSettlement,
}

impl EffectSettlementFailure {
    pub(super) fn error(&self) -> &PlanError {
        &self.error
    }

    pub(super) fn into_settlement(self) -> EffectSettlement {
        self.settlement
    }
}

impl From<ResourceError> for PlanError {
    fn from(error: ResourceError) -> Self {
        match error {
            ResourceError::PreAcceptedLimit => Self::Backpressure(Backpressure::TotalResources),
            ResourceError::RemoteLimit => Self::Backpressure(Backpressure::RemoteResources),
            ResourceError::PeerLimit(_) => Self::Backpressure(Backpressure::PeerResources),
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

impl From<SchedulerError> for PlanError {
    fn from(_: SchedulerError) -> Self {
        Self::Fault(AuthorityFault::SchedulerProjection)
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
            EffectError::Closed => Self::EffectClosed,
            EffectError::StaleLease => Self::Stale(StalePlan::EffectLease),
            EffectError::Projection => Self::Fault(AuthorityFault::EffectProjection),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CommittedChange {
    pub(super) sequence: ApplySequence,
    pub(super) changed: RawTxHash,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum CommittedChanges {
    One(CommittedChange),
    IndependentRun(Vec<CommittedChange>),
    DependencyControl(ApplySequence),
    EffectControl(ApplySequence),
}

#[derive(Debug, Default)]
pub(super) enum CommittedHandoff {
    #[default]
    None,
    Compute(CheckedOutWork),
    Effect(EffectLease),
}

#[derive(Debug)]
#[must_use = "a committed delta contains the only post-Apply work/effect handoff"]
pub(super) struct CommittedDelta {
    pub(super) changes: CommittedChanges,
    pub(super) handoff: CommittedHandoff,
    pub(super) removals: Vec<MembershipRemoval>,
    retired: Vec<OwnedTx>,
    retired_effect: Option<Arc<EffectBatch>>,
}

enum EntryRetirement {
    InlineDrop,
    Outside(Vec<OwnedTx>),
}

impl CommittedDelta {
    pub(in crate::authority) fn retired_len(&self) -> usize {
        self.retired.len()
    }

    pub(in crate::authority) fn handoff_is_none(&self) -> bool {
        matches!(self.handoff, CommittedHandoff::None)
    }

    /// Move compute work out while retaining every retirement carrier in this
    /// delta. Runtime callers must keep the delta alive until the authority
    /// guard has opened, then let its retired payloads fall out of scope.
    pub(in crate::authority) fn take_work(&mut self) -> Option<CheckedOutWork> {
        let handoff = std::mem::take(&mut self.handoff);
        match handoff {
            CommittedHandoff::Compute(work) => Some(work),
            other => {
                self.handoff = other;
                None
            }
        }
    }

    /// Test fixtures own no runtime authority guard, so they may consume the
    /// complete delta for concise capability extraction. This API is absent
    /// from production builds; production must retain the retirement carrier.
    #[cfg(test)]
    pub(in crate::authority) fn into_work(mut self) -> Option<CheckedOutWork> {
        self.take_work()
    }

    /// Move the publisher capability out without destroying retired effect or
    /// transaction payloads under the future authority guard.
    pub(in crate::authority) fn take_effect_lease(&mut self) -> Option<EffectLease> {
        let handoff = std::mem::take(&mut self.handoff);
        match handoff {
            CommittedHandoff::Effect(lease) => Some(lease),
            other => {
                self.handoff = other;
                None
            }
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn into_effect_lease(mut self) -> Option<EffectLease> {
        self.take_effect_lease()
    }

    pub(in crate::authority) fn retired_effect_len(&self) -> usize {
        usize::from(self.retired_effect.is_some())
    }
}

struct EntryDelta {
    key: RawTxHash,
    old_proposal: Option<ProposalId>,
    after: Option<OwnedTx>,
    retirement: EntryRetirement,
    resource: ResourcePlan,
    scheduler: SchedulerDelta,
    dependency: DependencyDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    sequence: ApplySequence,
}

struct EntryTransition {
    key: RawTxHash,
    before: Option<OwnedTx>,
    after: Option<OwnedTx>,
}

#[derive(Default)]
struct TransitionControls {
    dependency: DependencyControlDelta,
    effect: EffectDelta,
}

struct WorkHandoff {
    work: CheckedOutWork,
    origin: CheckoutOrigin,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the unit variant is test-only; boxing the production reservation would add a hot-path allocation"
)]
enum CheckoutOrigin {
    Scheduled {
        ticket: CheckoutTicket,
        reservation: CheckoutReservation,
    },
    #[cfg(test)]
    Foundation,
}

struct CheckoutReservation {
    resources: ResourcePlan,
    grant: ComputeGrant,
    after_charge: ResourceVector,
}

#[expect(
    clippy::large_enum_variant,
    reason = "boxing the move-only reservation would add a hot-path checkout allocation"
)]
enum CheckoutResource {
    Reserved(CheckoutReservation),
    SkipOwner,
    Stop,
}

struct CheckoutSearch {
    selected: Option<(CheckoutTicket, CheckoutReservation)>,
    #[cfg(test)]
    probes: usize,
}

struct DependencyLossKeys {
    keys: Vec<DependencyKey>,
    #[cfg(test)]
    work: DependencyLossWork,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DependencyLossWork {
    pub(super) output_keys: usize,
    pub(super) indexed_origin_keys: usize,
}

#[cfg(test)]
impl DependencyLossWork {
    pub(super) fn total(self) -> Option<usize> {
        self.output_keys.checked_add(self.indexed_origin_keys)
    }
}

struct MembershipDelta {
    changed_key: RawTxHash,
    changed_after: OwnedTx,
    removals: Vec<MembershipRemoval>,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    scheduler: SchedulerDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    retired: Vec<OwnedTx>,
    clocks: AuthorityClocks,
    committed: CommittedChanges,
}

struct IndependentUpdate {
    key: RawTxHash,
    after: OwnedTx,
}

struct IndependentDelta {
    updates: Vec<IndependentUpdate>,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    committed: Vec<CommittedChange>,
}

struct DependencyOnlyDelta {
    control: DependencyControlDelta,
    clocks: AuthorityClocks,
    sequence: ApplySequence,
}

struct EffectOnlyDelta {
    effect: EffectDelta,
    clocks: AuthorityClocks,
    sequence: ApplySequence,
}

enum AuthorityDelta {
    Entry(EntryDelta),
    Membership(MembershipDelta),
    Independent(IndependentDelta),
    Dependency(DependencyOnlyDelta),
    Effect(EffectOnlyDelta),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MissingResolutionDisposition {
    Wait,
    RejectUnavailable,
}

#[must_use = "a prepared authority transition has no effect until explicitly applied"]
pub(super) struct PreparedApply<'authority> {
    authority: &'authority mut TxPoolAuthority,
    delta: AuthorityDelta,
    handoff: CommittedHandoff,
}

impl PreparedApply<'_> {
    pub(super) fn apply(self) -> CommittedDelta {
        let Self {
            authority,
            delta,
            handoff,
        } = self;
        match delta {
            AuthorityDelta::Entry(delta) => Self::apply_entry(authority, delta, handoff),
            AuthorityDelta::Membership(delta) => Self::apply_membership(authority, delta),
            AuthorityDelta::Independent(delta) => Self::apply_independent(authority, delta),
            AuthorityDelta::Dependency(delta) => Self::apply_dependency(authority, delta),
            AuthorityDelta::Effect(delta) => Self::apply_effect(authority, delta, handoff),
        }
    }

    fn apply_entry(
        authority: &mut TxPoolAuthority,
        delta: EntryDelta,
        handoff: CommittedHandoff,
    ) -> CommittedDelta {
        if let Some(proposal) = delta.old_proposal {
            authority.by_proposal.remove(&proposal);
        }
        let previous = match delta.after {
            Some(entry) => {
                let proposal = entry.record().identity.proposal.clone();
                authority.by_proposal.insert(proposal, delta.key.clone());
                authority.entries.insert(delta.key.clone(), entry)
            }
            None => authority.entries.remove(&delta.key),
        };
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
        CommittedDelta {
            changes: CommittedChanges::One(CommittedChange {
                sequence: delta.sequence,
                changed: delta.key,
            }),
            handoff,
            removals: Vec::new(),
            retired,
            retired_effect,
        }
    }

    fn apply_membership(authority: &mut TxPoolAuthority, delta: MembershipDelta) -> CommittedDelta {
        let mut retired = delta.retired;
        for removal in &delta.removals {
            if let Some(owner) = authority.entries.remove(&removal.hash) {
                retired.push(owner);
            }
            authority.by_proposal.remove(&removal.proposal);
        }
        // The candidate's accepted owner shares its immutable transaction and
        // resolved facts with the pre-accepted predecessor. Only removed
        // victims can carry the last large owner, so their destruction is
        // handed out in `retired`; replacing this small shell cannot allocate.
        drop(
            authority
                .entries
                .insert(delta.changed_key, delta.changed_after),
        );
        authority.resources.apply_batch(delta.resource);
        authority.membership.apply(delta.projection);
        authority.scheduler.apply(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: delta.committed,
            handoff: CommittedHandoff::None,
            removals: delta.removals,
            retired,
            retired_effect,
        }
    }

    fn apply_independent(
        authority: &mut TxPoolAuthority,
        delta: IndependentDelta,
    ) -> CommittedDelta {
        for update in delta.updates {
            // Independent acceptance also replaces a pre-accepted shell whose
            // immutable transaction and resolved facts are shared by `after`.
            drop(authority.entries.insert(update.key, update.after));
        }
        authority.resources.apply_batch(delta.resource);
        authority.membership.apply(delta.projection);
        authority.scheduler.apply_batch(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: CommittedChanges::IndependentRun(delta.committed),
            handoff: CommittedHandoff::None,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect,
        }
    }

    fn apply_dependency(
        authority: &mut TxPoolAuthority,
        delta: DependencyOnlyDelta,
    ) -> CommittedDelta {
        authority.dependencies.apply_control(delta.control);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: CommittedChanges::DependencyControl(delta.sequence),
            handoff: CommittedHandoff::None,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect: None,
        }
    }

    fn apply_effect(
        authority: &mut TxPoolAuthority,
        delta: EffectOnlyDelta,
        handoff: CommittedHandoff,
    ) -> CommittedDelta {
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: CommittedChanges::EffectControl(delta.sequence),
            handoff,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect,
        }
    }
}

fn next_version(version: EntryVersion) -> Result<EntryVersion, PlanError> {
    version
        .0
        .checked_add(1)
        .map(EntryVersion)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_arrival(arrival: Arrival) -> Result<Arrival, PlanError> {
    arrival
        .0
        .checked_add(1)
        .map(Arrival)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_lease(
    lease: super::state::ComputeLeaseId,
) -> Result<super::state::ComputeLeaseId, PlanError> {
    lease
        .0
        .checked_add(1)
        .map(super::state::ComputeLeaseId)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn next_sequence(sequence: ApplySequence) -> Result<ApplySequence, PlanError> {
    sequence
        .0
        .checked_add(1)
        .map(ApplySequence)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
}

fn retired_buffer(capacity: usize) -> Result<Vec<OwnedTx>, PlanError> {
    let mut retired = Vec::new();
    retired
        .try_reserve(capacity)
        .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
    Ok(retired)
}

impl TxPoolAuthority {
    fn missing_resolution_disposition(
        &self,
        class: AdmissionClass,
        missing: &super::state::MissingDependencies,
    ) -> MissingResolutionDisposition {
        if matches!(class, AdmissionClass::Remote(_)) {
            return MissingResolutionDisposition::Wait;
        }

        let has_unavailable_dependency = missing.keys().iter().any(|key| match key {
            DependencyKey::Cell(out_point) => !matches!(
                self.entries.get(&RawTxHash(out_point.tx_hash())),
                Some(OwnedTx::PreAccepted(_))
            ),
            DependencyKey::Header(_) => true,
        });
        if has_unavailable_dependency {
            MissingResolutionDisposition::RejectUnavailable
        } else {
            MissingResolutionDisposition::Wait
        }
    }

    fn validate_acceptance_evidence(
        &self,
        preaccepted: &PreAcceptedEntry,
        verified: &super::state::VerifiedFacts,
    ) -> Result<(), PlanError> {
        if verified.chain_epoch() != self.chain_epoch {
            return Err(PlanError::Stale(StalePlan::ChainEpoch));
        }
        if verified.witness() != &preaccepted.record.identity.witness
            || verified.payload().identity() != &preaccepted.record.identity
        {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let dependencies = verified.payload().dependencies();
        if !self
            .dependencies
            .proof_is_current(dependencies, verified.dependency_cut())
        {
            return Err(PlanError::Stale(StalePlan::Dependency));
        }
        Ok(())
    }

    fn plan_membership_dependency_delta(
        &self,
        existing: &OwnedTx,
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
        changes.push((Some(existing), Some(after)));
        let mut removed_entries = Vec::new();
        removed_entries
            .try_reserve(removals.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for removal in removals {
            let removed = self
                .entries
                .get(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            changes.push((Some(removed), None));
            removed_entries.push(removed);
        }
        let control = self.plan_dependency_loss(removed_entries, sequence)?;
        let delta = self.dependencies.plan_replacements(changes)?;
        Ok(delta.with_control(control))
    }

    fn plan_dependency_loss<'entry>(
        &self,
        parents: impl IntoIterator<Item = &'entry OwnedTx>,
        sequence: ApplySequence,
    ) -> Result<DependencyControlDelta, PlanError> {
        let loss = self.collect_dependency_loss_keys(parents)?;
        Ok(self
            .dependencies
            .plan_event(
                loss.keys,
                DependencyEvent::DefinitiveLoss(DependencyCut(sequence)),
            )?
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

    #[cfg(test)]
    pub(super) fn dependency_loss_work_for_foundation(
        &self,
        parents: &[RawTxHash],
    ) -> Result<DependencyLossWork, PlanError> {
        let parents = parents
            .iter()
            .map(|hash| {
                self.entries
                    .get(hash)
                    .ok_or(PlanError::Stale(StalePlan::Missing))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.collect_dependency_loss_keys(parents)?.work)
    }

    pub(super) fn plan_admission(
        &mut self,
        admission: ValidatedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.resources.validate_admission(admission.charge)?;
        let key = admission.identity.raw.clone();
        if let Some(existing) = self.entries.get(&key).cloned() {
            return self.plan_existing_admission(key, existing, admission);
        }
        if self.by_proposal.contains_key(&admission.identity.proposal) {
            return Err(PlanError::Backpressure(Backpressure::ProposalCollision));
        }

        let version = self.clocks.next_version;
        let arrival = self.clocks.next_arrival;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_arrival: next_arrival(arrival)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let dependencies = admission.dependencies;
        let record = TxRecord {
            tx: admission.tx,
            identity: admission.identity,
            ingress: admission.ingress,
            blame: admission.blame,
            class: admission.class,
            version,
            arrival,
        };
        let after = OwnedTx::PreAccepted(PreAcceptedEntry {
            record,
            basis: AdmissionBasis::new(dependencies, admission.charge),
            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
            charge: admission.charge,
        });
        self.prepare_entry_delta(
            EntryTransition {
                key,
                before: None,
                after: Some(after),
            },
            clocks,
            sequence,
            None,
        )
    }

    fn plan_existing_admission(
        &mut self,
        key: RawTxHash,
        existing: OwnedTx,
        admission: ValidatedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        if existing.record().identity.witness != admission.identity.witness {
            return Err(PlanError::PayloadVariant);
        }
        let AdmissionClass::Proposal(proposal) = admission.class else {
            return Err(PlanError::Duplicate);
        };
        let OwnedTx::PreAccepted(entry) = &existing else {
            return Err(PlanError::Duplicate);
        };
        if entry.record.class == AdmissionClass::Proposal(proposal) {
            return Err(PlanError::Duplicate);
        }

        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut promoted = entry.clone();
        promoted.record.class = AdmissionClass::Proposal(proposal);
        self.prepare_entry_delta(
            EntryTransition {
                key,
                before: Some(existing),
                after: Some(OwnedTx::PreAccepted(promoted)),
            },
            clocks,
            sequence,
            None,
        )
    }

    pub(super) fn plan_accept_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.ensure_open()?;
        let existing = self
            .entries
            .get(key)
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Computed(super::state::ComputedOutcome::Verified(verified)) =
            &preaccepted.phase
        else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        self.validate_acceptance_evidence(preaccepted, verified)?;
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut record = preaccepted.record.clone();
        record.version = version;
        let accepted = AcceptedEntry {
            record,
            status,
            verified: verified.clone(),
        };
        let PreparedMembership {
            removals,
            resource,
            projection,
        } = self.prepare_membership(key, preaccepted, &accepted)?;
        let retired = retired_buffer(removals.len())?;
        let after = OwnedTx::Accepted(accepted);
        let scheduler = self
            .scheduler
            .plan_replace(Some(&existing), Some(&after), None)?;
        let dependency =
            self.plan_membership_dependency_delta(&existing, &after, &removals, sequence)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(MembershipDelta {
                changed_key: key.clone(),
                changed_after: after,
                removals,
                resource,
                projection,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                retired,
                clocks,
                committed: CommittedChanges::One(CommittedChange {
                    sequence,
                    changed: key.clone(),
                }),
            }),
            handoff: CommittedHandoff::None,
        })
    }

    pub(super) fn plan_status_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.ensure_open()?;
        let existing = self
            .entries
            .get(key)
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::Accepted(before) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if before.status == status {
            return Err(PlanError::Duplicate);
        }

        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut after = before.clone();
        after.record.version = version;
        after.status = status;
        let projection = self.prepare_status_change(key, before, &after)?;
        let after = OwnedTx::Accepted(after);
        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve(1)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        resource_changes.push((
            key.clone(),
            Some(existing.charge_record()),
            Some(after.charge_record()),
        ));
        let resource = self.resources.plan_batch(resource_changes)?;
        let scheduler = self
            .scheduler
            .plan_replace(Some(&existing), Some(&after), None)?;
        let dependency = self.plan_membership_dependency_delta(&existing, &after, &[], sequence)?;
        let retired = Vec::new();
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(MembershipDelta {
                changed_key: key.clone(),
                changed_after: after,
                removals: Vec::new(),
                resource,
                projection,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                retired,
                clocks,
                committed: CommittedChanges::One(CommittedChange {
                    sequence,
                    changed: key.clone(),
                }),
            }),
            handoff: CommittedHandoff::None,
        })
    }

    pub(super) fn plan_terminalize_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_terminalize_with_publication(key, expected, None)
    }

    pub(super) fn plan_terminalize_with_effect_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_terminalize_with_publication(key, expected, Some(publication))
    }

    fn plan_terminalize_with_publication(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        publication: Option<&EffectPublication>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let existing = self
            .entries
            .get(key)
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if matches!(preaccepted.phase, PreAcceptedPhase::Computing(_)) {
            return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
        }
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let dependency_control = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = publication.map_or_else(
            || Ok(EffectDelta::default()),
            |publication| self.effects.plan_publication(publication, sequence),
        )?;
        self.prepare_entry_delta_with_controls(
            EntryTransition {
                key: key.clone(),
                before: Some(existing),
                after: None,
            },
            clocks,
            sequence,
            None,
            TransitionControls {
                dependency: dependency_control,
                effect,
            },
            None,
        )
    }

    pub(super) fn effect_publication_for_foundation(
        &self,
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
    ) -> Result<EffectPublication, EffectBuildError> {
        self.effects.build_publication(policy, effects)
    }

    pub(super) fn plan_effect_publication_for_foundation(
        &mut self,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let sequence = self.clocks.next_sequence;
        let effect = self.effects.plan_publication(publication, sequence)?;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::None)
    }

    pub(super) fn plan_generation_reset_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let sequence = self.clocks.next_sequence;
        let effect = self.effects.plan_generation_reset(sequence)?;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::None)
    }

    pub(super) fn plan_effect_checkout_for_foundation(
        &mut self,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let Some((effect, lease)) = self.effects.plan_checkout()? else {
            return Ok(None);
        };
        let sequence = self.clocks.next_sequence;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::Effect(lease))
            .map(Some)
    }

    pub(super) fn apply_effect_settlement_for_foundation(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<CommittedDelta, EffectSettlementFailure> {
        let effect = match self.effects.plan_settlement(&settlement) {
            Ok(effect) => effect,
            Err(error) => {
                return Err(EffectSettlementFailure {
                    error: error.into(),
                    settlement,
                });
            }
        };
        let sequence = self.clocks.next_sequence;
        match self.prepare_effect_only(effect, sequence, CommittedHandoff::None) {
            Ok(plan) => Ok(plan.apply()),
            Err(error) => Err(EffectSettlementFailure { error, settlement }),
        }
    }

    pub(super) fn plan_effect_close_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        if self.resources.preaccepted().active_work != 0 {
            return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
        }
        let effect = self.effects.plan_close()?;
        let sequence = self.clocks.next_sequence;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::None)
    }

    pub(super) fn effects_closed_and_drained_for_foundation(&self) -> bool {
        self.effects.is_closed_and_drained()
    }

    pub(super) fn effect_observation_for_foundation(&self) -> EffectObservation {
        self.effects.observation()
    }

    fn prepare_effect_only(
        &mut self,
        effect: EffectDelta,
        sequence: ApplySequence,
        handoff: CommittedHandoff,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Effect(EffectOnlyDelta {
                effect,
                clocks,
                sequence,
            }),
            handoff,
        })
    }

    pub(super) fn plan_dependency_availability_for_foundation(
        &mut self,
        keys: Vec<DependencyKey>,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        self.effects.ensure_open()?;
        let sequence = self.clocks.next_sequence;
        let Some(control) = self
            .dependencies
            .plan_event(keys, DependencyEvent::Availability(DependencyCut(sequence)))?
        else {
            return Ok(None);
        };
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        Ok(Some(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Dependency(DependencyOnlyDelta {
                control,
                clocks,
                sequence,
            }),
            handoff: CommittedHandoff::None,
        }))
    }

    pub(super) fn plan_dependency_maintenance_for_foundation(
        &mut self,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        self.effects.ensure_open()?;
        let Some(ticket) = self.dependencies.next_maintenance()? else {
            return Ok(None);
        };
        let hash = ticket.hash().cloned();
        let requeue =
            ticket.requires_requeue(hash.as_ref().and_then(|hash| self.entries.get(hash)))?;
        let control = self.dependencies.plan_maintenance(ticket)?;
        let sequence = self.clocks.next_sequence;
        if !requeue {
            let clocks = AuthorityClocks {
                next_sequence: next_sequence(sequence)?,
                ..self.clocks
            };
            return Ok(Some(PreparedApply {
                authority: self,
                delta: AuthorityDelta::Dependency(DependencyOnlyDelta {
                    control,
                    clocks,
                    sequence,
                }),
                handoff: CommittedHandoff::None,
            }));
        }

        let hash = hash.ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let existing = self
            .entries
            .get(&hash)
            .cloned()
            .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
        };
        let charge = preaccepted.original_charge();
        let version = self.clocks.next_version;
        let after = existing
            .with_foundation_phase(
                PreAcceptedPhase::Queued(QueuedWork::Resolve),
                version,
                charge,
            )
            .map_err(PlanError::Stale)?;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        self.prepare_entry_delta_with_dependency(
            EntryTransition {
                key: hash,
                before: Some(existing),
                after: Some(after),
            },
            clocks,
            sequence,
            None,
            control,
        )
        .map(Some)
    }

    #[cfg(test)]
    pub(super) fn dependency_maintenance_observation_for_foundation(
        &self,
    ) -> Result<Option<(DependencyKey, Option<RawTxHash>)>, PlanError> {
        Ok(self.dependencies.next_maintenance_observation()?)
    }

    #[cfg(test)]
    pub(super) fn plan_checkout_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::state::WorkPermit,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_selected_checkout(key, expected, permit, CheckoutOrigin::Foundation)
    }

    /// Select and reserve one exact scheduler head. Saturated peer/source
    /// owners are skipped without publishing a second blocked-owner state.
    pub(super) fn plan_checkout_next(
        &mut self,
        permit: super::state::WorkPermit,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let search = self.search_checkout(permit)?;
        self.prepare_checkout_search(search, permit)
    }

    #[cfg(test)]
    pub(super) fn plan_checkout_next_with_probe_count_for_foundation(
        &mut self,
        permit: super::state::WorkPermit,
    ) -> Result<(Option<PreparedApply<'_>>, usize), PlanError> {
        let search = self.search_checkout(permit)?;
        let probes = search.probes;
        let plan = self.prepare_checkout_search(search, permit)?;
        Ok((plan, probes))
    }

    fn search_checkout(
        &mut self,
        permit: super::state::WorkPermit,
    ) -> Result<CheckoutSearch, PlanError> {
        match self
            .resources
            .active_work_availability(ComputeAttribution::Trusted)?
        {
            ActiveWorkAvailability::Available => {}
            ActiveWorkAvailability::PreAcceptedExhausted => {
                return Ok(CheckoutSearch {
                    selected: None,
                    #[cfg(test)]
                    probes: 0,
                });
            }
            ActiveWorkAvailability::RemoteExhausted | ActiveWorkAvailability::PeerExhausted(_) => {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
        }
        let owner_count = self.scheduler.owner_count(permit);
        let mut cursor = None;
        let mut selected = None;
        #[cfg(test)]
        let mut probes = 0usize;
        for _ in 0..owner_count {
            let ticket = match cursor {
                Some(owner) => self.scheduler.next_queued_after(permit, Some(owner)),
                None => self.scheduler.next_queued(permit),
            };
            let Some(ticket) = ticket else {
                break;
            };
            #[cfg(test)]
            {
                probes = probes
                    .checked_add(1)
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            }
            cursor = Some(ticket.owner());
            match self.plan_checkout_resources(ticket.hash(), ticket.version(), permit)? {
                CheckoutResource::Reserved(reservation) => {
                    selected = Some((ticket, reservation));
                    break;
                }
                CheckoutResource::SkipOwner => {}
                CheckoutResource::Stop => break,
            }
        }
        Ok(CheckoutSearch {
            selected,
            #[cfg(test)]
            probes,
        })
    }

    fn prepare_checkout_search(
        &mut self,
        search: CheckoutSearch,
        permit: super::state::WorkPermit,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let Some((ticket, reservation)) = search.selected else {
            return Ok(None);
        };
        let key = ticket.hash().clone();
        let version = ticket.version();
        self.plan_selected_checkout(
            &key,
            version,
            permit,
            CheckoutOrigin::Scheduled {
                ticket,
                reservation,
            },
        )
        .map(Some)
    }

    fn plan_checkout_resources(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::state::WorkPermit,
    ) -> Result<CheckoutResource, PlanError> {
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
        let attribution = preaccepted.record.class.compute_attribution();
        match self.resources.active_work_availability(attribution)? {
            ActiveWorkAvailability::Available => {}
            ActiveWorkAvailability::PeerExhausted(_) => {
                return Ok(CheckoutResource::SkipOwner);
            }
            ActiveWorkAvailability::PreAcceptedExhausted
            | ActiveWorkAvailability::RemoteExhausted => {
                return Ok(CheckoutResource::Stop);
            }
        }
        let (grant, after_charge) = self.checkout_projection(preaccepted, permit)?;
        let expected_charge = existing.charge_record();
        let after_record = ChargeRecord::PreAccepted {
            resources: after_charge,
            residency_peer: preaccepted.record.ingress.peer(),
            compute_peer: attribution.peer(),
        };
        let resources = self
            .resources
            .plan_replace(key.clone(), Some(expected_charge), Some(after_record))
            .map_err(|error| match error {
                ResourceError::PreAcceptedLimit
                | ResourceError::PeerLimit(_)
                | ResourceError::RemoteLimit => {
                    PlanError::Fault(AuthorityFault::ResourceProjection)
                }
                error => error.into(),
            })?;
        Ok(CheckoutResource::Reserved(CheckoutReservation {
            resources,
            grant,
            after_charge,
        }))
    }

    fn checkout_projection(
        &self,
        preaccepted: &PreAcceptedEntry,
        permit: super::state::WorkPermit,
    ) -> Result<(ComputeGrant, ResourceVector), PlanError> {
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
            return Err(PlanError::Stale(StalePlan::Dependency));
        }

        let (max_resident_bytes, max_edges) =
            self.resources.compute_limits().reservation_for(permit);
        let grant = ComputeGrant {
            max_resident_bytes,
            max_edges,
        };
        if let QueuedWork::Verify(resolved) = queued
            && (resolved.payload().resolved_resident_bytes() > grant.max_resident_bytes
                || resolved.payload().footprint.edge_count() > grant.max_edges)
        {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
        }
        let mut charge = preaccepted.retained_charge(
            preaccepted.original_charge().bytes,
            preaccepted.dependencies(),
        );
        charge.active_work = charge
            .active_work
            .checked_add(1)
            .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?;
        Ok((grant, charge))
    }

    fn plan_selected_checkout(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::state::WorkPermit,
        origin: CheckoutOrigin,
    ) -> Result<PreparedApply<'_>, PlanError> {
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

        let (grant, charge) = match &origin {
            CheckoutOrigin::Scheduled { reservation, .. } => {
                (reservation.grant, reservation.after_charge)
            }
            #[cfg(test)]
            CheckoutOrigin::Foundation => self.checkout_projection(preaccepted, permit)?,
        };
        let version = self.clocks.next_version;
        let lease = self.clocks.next_lease;
        let sequence = self.clocks.next_sequence;
        let dependency_cut = match queued {
            QueuedWork::Resolve => DependencyCut(sequence),
            QueuedWork::Verify(resolved) => resolved.dependency_cut(),
        };
        let token = LeaseToken {
            settlement: SettlementToken {
                hash: key.clone(),
                version,
                lease,
                chain_epoch: self.chain_epoch,
            },
            dependency_cut,
            permit,
            grant,
        };
        let work = CheckedOutWork::new(
            token,
            Arc::clone(&preaccepted.record.tx),
            preaccepted.basis.dependencies().clone(),
            queued.clone(),
        )
        .map_err(|_| PlanError::Stale(StalePlan::Phase))?;
        let active_dependencies = preaccepted.dependencies().clone();
        let attribution = preaccepted.record.class.compute_attribution();
        let after = existing
            .with_foundation_phase(
                PreAcceptedPhase::Computing(super::state::ActiveWork {
                    lease,
                    permit,
                    grant,
                    attribution,
                    dependency_cut,
                    dependencies: active_dependencies,
                }),
                version,
                charge,
            )
            .map_err(PlanError::Stale)?;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_lease: next_lease(lease)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        self.prepare_entry_delta(
            EntryTransition {
                key: key.clone(),
                before: Some(existing.clone()),
                after: Some(after),
            },
            clocks,
            sequence,
            Some(WorkHandoff { work, origin }),
        )
    }

    /// Consume a move-only compute completion in one atomic command. A
    /// successful Plan is intentionally not exposed as a droppable value:
    /// doing so could destroy the only lease completion while the authority
    /// still retained `Computing`.
    #[expect(
        clippy::result_large_err,
        reason = "the error must return the move-only lease token; boxing could allocate while reporting allocation backpressure, and the committed handoff already fixes the Result footprint"
    )]
    pub(super) fn apply_settlement(
        &mut self,
        settlement: ComputeSettlement,
    ) -> Result<CommittedDelta, ComputeSettlementFailure> {
        let ComputeSettlement { token, next } = settlement;
        match self.prepare_settlement(&token, next) {
            Ok(plan) => Ok(plan.apply()),
            Err(error) => Err(ComputeSettlementFailure { error, token }),
        }
    }

    fn prepare_settlement<'a>(
        &'a mut self,
        token: &SettlementToken,
        next: SettlementNext,
    ) -> Result<PreparedApply<'a>, PlanError> {
        let existing = self
            .entries
            .get(&token.hash)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != token.version {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        if self.chain_epoch != token.chain_epoch {
            return Err(PlanError::Stale(StalePlan::ChainEpoch));
        }
        let OwnedTx::PreAccepted(preaccepted) = existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if active.lease != token.lease {
            return Err(PlanError::Stale(StalePlan::Lease));
        }
        // The compact settlement capability carries only the identity, ABA,
        // and chain fences needed after compute. Permit, grant, and dependency
        // cut remain authoritative in the matching `ActiveWork`; sealed
        // receipts cannot be manufactured outside the checked-out capability.
        let dependency_cut = active.dependency_cut;
        let raw_charge = preaccepted.original_charge();
        let base_proof_is_current = self
            .dependencies
            .proof_is_current(preaccepted.dependencies(), dependency_cut);
        let (phase, retained_charge) = if !base_proof_is_current {
            (PreAcceptedPhase::Queued(QueuedWork::Resolve), raw_charge)
        } else {
            match next {
                SettlementNext::QueuedVerify(resolved) => {
                    if resolved.payload().identity() != &preaccepted.record.identity {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                    if resolved.chain_epoch() != token.chain_epoch {
                        return Err(PlanError::Stale(StalePlan::ChainEpoch));
                    }
                    if resolved.dependency_cut() != dependency_cut {
                        return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                    }
                    let dependencies = resolved.payload().dependencies().clone();
                    let retained_charge = preaccepted.retained_charge(
                        resolved.payload().resolved_resident_bytes(),
                        &dependencies,
                    );
                    if self.dependencies.resolution_is_current(
                        preaccepted.dependencies(),
                        &dependencies,
                        dependency_cut,
                    ) {
                        (
                            PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
                            retained_charge,
                        )
                    } else {
                        (PreAcceptedPhase::Queued(QueuedWork::Resolve), raw_charge)
                    }
                }
                SettlementNext::Waiting(missing) => {
                    let dependencies = missing.dependencies().clone();
                    if self.dependencies.missing_result_is_current(
                        preaccepted.dependencies(),
                        &dependencies,
                        missing.missing(),
                        dependency_cut,
                    ) {
                        if self.missing_resolution_disposition(
                            preaccepted.record.class,
                            missing.missing(),
                        ) == MissingResolutionDisposition::RejectUnavailable
                        {
                            (
                                PreAcceptedPhase::Computed(ComputedOutcome::Rejected(
                                    RejectionKind::UnavailableDependency,
                                )),
                                raw_charge,
                            )
                        } else {
                            let retained_charge = preaccepted.retained_charge(
                                preaccepted.original_charge().bytes,
                                &dependencies,
                            );
                            let observed = self.dependencies.observe_missing(
                                missing.missing(),
                                dependencies,
                                dependency_cut,
                            );
                            (
                                PreAcceptedPhase::Waiting(WaitCondition::Missing(observed)),
                                retained_charge,
                            )
                        }
                    } else {
                        (PreAcceptedPhase::Queued(QueuedWork::Resolve), raw_charge)
                    }
                }
                SettlementNext::Computed(super::state::ComputedOutcome::Verified(verified)) => {
                    if verified.witness() != &preaccepted.record.identity.witness
                        || verified.payload().identity() != &preaccepted.record.identity
                    {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                    if verified.chain_epoch() != token.chain_epoch {
                        return Err(PlanError::Stale(StalePlan::ChainEpoch));
                    }
                    if verified.dependency_cut() != dependency_cut {
                        return Err(PlanError::Fault(AuthorityFault::DependencyProjection));
                    }
                    let dependencies = verified.payload().dependencies().clone();
                    let retained_charge = preaccepted
                        .retained_charge(verified.metrics().cost.resident_bytes, &dependencies);
                    if self.dependencies.resolution_is_current(
                        preaccepted.dependencies(),
                        &dependencies,
                        dependency_cut,
                    ) {
                        (
                            PreAcceptedPhase::Computed(super::state::ComputedOutcome::Verified(
                                verified,
                            )),
                            retained_charge,
                        )
                    } else {
                        (PreAcceptedPhase::Queued(QueuedWork::Resolve), raw_charge)
                    }
                }
                SettlementNext::Computed(outcome) => {
                    (PreAcceptedPhase::Computed(outcome), raw_charge)
                }
            }
        };
        let grant_ceiling = ResourceVector::new(
            1,
            active.grant.max_resident_bytes,
            active.grant.max_edges,
            0,
        );
        if preaccepted.charge.active_work != 1 || !retained_charge.fits(grant_ceiling) {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
        }
        let expected_charge = existing.charge_record();
        let desired_charge = ChargeRecord::PreAccepted {
            resources: retained_charge,
            residency_peer: preaccepted.record.ingress.peer(),
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
                let fallback_charge = ChargeRecord::PreAccepted {
                    resources: raw_charge,
                    residency_peer: preaccepted.record.ingress.peer(),
                    compute_peer: None,
                };
                let resource = self
                    .resources
                    .plan_replace(
                        token.hash.clone(),
                        Some(expected_charge),
                        Some(fallback_charge),
                    )
                    .map_err(|_| PlanError::Fault(AuthorityFault::ResourceProjection))?;
                (
                    PreAcceptedPhase::Computed(ComputedOutcome::BudgetDenied),
                    raw_charge,
                    resource,
                )
            }
            Err(error) => return Err(error.into()),
        };
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let after = existing
            .with_foundation_phase(phase, version, retained_charge)
            .map_err(PlanError::Stale)?;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        self.prepare_entry_delta_with_preplanned_resource(
            EntryTransition {
                key: token.hash.clone(),
                before: Some(existing.clone()),
                after: Some(after),
            },
            clocks,
            sequence,
            None,
            resource,
        )
    }

    fn prepare_entry_delta(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        handoff: Option<WorkHandoff>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            handoff,
            TransitionControls::default(),
            None,
        )
    }

    fn prepare_entry_delta_with_preplanned_resource(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        handoff: Option<WorkHandoff>,
        resource: ResourcePlan,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            handoff,
            TransitionControls::default(),
            Some(resource),
        )
    }

    fn prepare_entry_delta_with_dependency(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        handoff: Option<WorkHandoff>,
        dependency_control: DependencyControlDelta,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            handoff,
            TransitionControls {
                dependency: dependency_control,
                effect: EffectDelta::default(),
            },
            None,
        )
    }

    fn prepare_entry_delta_with_controls(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        handoff: Option<WorkHandoff>,
        controls: TransitionControls,
        explicit_resources: Option<ResourcePlan>,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.ensure_open()?;
        let EntryTransition {
            key,
            before: expected,
            after,
        } = transition;
        let (work, checkout, checkout_resources) = match handoff {
            Some(WorkHandoff {
                work,
                origin:
                    CheckoutOrigin::Scheduled {
                        ticket,
                        reservation,
                    },
            }) => (Some(work), Some(ticket), Some(reservation.resources)),
            #[cfg(test)]
            Some(WorkHandoff {
                work,
                origin: CheckoutOrigin::Foundation,
            }) => (Some(work), None, None),
            None => (None, None, None),
        };
        let preplanned_resources = match (checkout_resources, explicit_resources) {
            (Some(_), Some(_)) => {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
            (Some(resources), None) | (None, Some(resources)) => Some(resources),
            (None, None) => None,
        };
        let expected_charge = expected.as_ref().map(OwnedTx::charge_record);
        let after_charge = after.as_ref().map(OwnedTx::charge_record);
        let old_proposal = expected
            .as_ref()
            .map(|entry| entry.record().identity.proposal.clone());
        let retirement = if expected.is_some() && after.is_none() {
            EntryRetirement::Outside(retired_buffer(1)?)
        } else {
            EntryRetirement::InlineDrop
        };
        if after.is_some() && expected.is_none() {
            self.entries
                .try_reserve(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            self.by_proposal
                .try_reserve(1)
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        }
        let resource = match preplanned_resources {
            Some(resources) => resources,
            None => self
                .resources
                .plan_replace(key.clone(), expected_charge, after_charge)?,
        };
        let scheduler = self
            .scheduler
            .plan_replace(expected.as_ref(), after.as_ref(), checkout)?;
        let dependency = self
            .dependencies
            .plan_replace(expected.as_ref(), after.as_ref())?
            .with_control(controls.dependency);
        let effect = controls.effect;
        let handoff = work.map_or(CommittedHandoff::None, CommittedHandoff::Compute);
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Entry(EntryDelta {
                key,
                old_proposal,
                after,
                retirement,
                resource,
                scheduler,
                dependency,
                effect,
                clocks,
                sequence,
            }),
            handoff,
        })
    }
}

impl OwnedTx {
    pub(super) fn with_foundation_phase(
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
            basis: entry.basis.clone(),
            phase,
            charge,
        }))
    }
}
