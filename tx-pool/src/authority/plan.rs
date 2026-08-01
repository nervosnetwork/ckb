mod chain_transition;
mod membership;
mod settlement;

#[cfg(test)]
use super::chain::AcceptedProof;
#[cfg(test)]
use super::chain::{AdmissionEvidenceError, DirectAdmissionWork};
use super::chain::{
    DirectAdmissionReceipt, FinalAdmissionReceipt, FinalAdmissionRejection, FinalAdmissionRetry,
    FinalAdmissionSubject, FinalAdmissionWork, ReadyPayloadRelation,
};
#[cfg(test)]
use super::dependency::DependencySnapshot;
use super::dependency::{
    DependencyBatchDelta, DependencyControlDelta, DependencyDelta, DependencyError,
    DependencyEvent, DependencyFrontier, DependencyMaintenanceAction,
};
use super::effect::{
    CommittedAcceptance, CommittedEffect, CommittedRejection, EffectBatch, EffectBuildError,
    EffectConfigError, EffectDelta, EffectError, EffectLease, EffectLimits, EffectLog,
    EffectPolicy, EffectPublication, EffectSettlement, ParentTransactionRequest, RejectionAudience,
};
#[cfg(test)]
use super::effect::{EffectObservation, EffectSnapshot};
#[cfg(test)]
use super::indexes::IndexSnapshot;
use super::indexes::{AuthorityIndexes, IndexDelta, IndexError};
use super::read::AuthorityReadView;
use super::rejection::CommittedPublicReject;
#[cfg(test)]
pub(in crate::authority) use super::rejection::ComponentLimitKind;
pub(in crate::authority) use super::rejection::MembershipReject;
#[cfg(test)]
use super::resources::ResourceSnapshot;
use super::resources::{
    ActiveWorkAvailability, ChargeRecord, ChargedAdmission, ResourceBatchPlan, ResourceError,
    ResourceLedger, ResourceLimits, ResourcePlan, ResourceVector,
};
#[cfg(test)]
use super::scheduler::SchedulerSnapshot;
use super::scheduler::{
    CheckoutTicket, FairFrontier, QueueLane, SchedulerBatchDelta, SchedulerDelta, SchedulerError,
    VerifyOrder,
};
#[cfg(test)]
use super::source::PoolTemplateVersions;
use super::source::{AuthoritySourceVersions, SourceVersionDelta};
#[cfg(test)]
use super::state::{AcceptedAtMillis, AcceptedStatus, TxIdentity};
use super::state::{
    AcceptedEntry, AcceptedProvenance, AdmissionBasis, ApplySequence, Arrival, AuthorityClocks,
    ChainRevision, ChainViewId, ComputeAttribution, ComputeGrant, DependencyCut, DependencyKey,
    DependencyOrigin, EntryVersion, KnownDependencies, MissingDependencies, OwnedTx, PayloadPolicy,
    PoolGeneration, PreAcceptedEntry, PreAcceptedPhase, PreAcceptedSource, ProposalBase,
    QueuedWork, RawTxHash, RemoteDeadline, ReplacementHistoryEntry, ReplacementHistoryError,
    TxRecord, ValidatedAdmission,
};
use super::validation::FinalAdmissionValidationOutcome;
use super::work::{
    CheckedOutWork, ComputeSettlement, LeaseToken, SettlementNext, SettlementRejection,
    SettlementToken,
};
use crate::error::Reject;
#[cfg(test)]
use ckb_types::core::TransactionView;
use ckb_types::{
    core::{error::OutPointError, tx_pool::get_transaction_weight},
    prelude::Unpack,
};
#[cfg(test)]
use ckb_verification::cache::ScriptVerificationRules;
pub(in crate::authority) use membership::MembershipConfig;
pub(in crate::authority) use membership::{
    AcceptedOrderKey, EvictionOrderKey, MembershipProjection,
};
#[cfg(test)]
pub(in crate::authority) use membership::{
    AncestorAggregate, DescendantAggregate, IndependentCoupling, MembershipSnapshot,
};
use membership::{MembershipRemoval, PreparedMembership, ProjectionDelta};
pub(in crate::authority) use membership::{RemovalCause, StatusCounts};
pub(in crate::authority) use settlement::{
    CandidateBatchError, IndependentCandidate, SettlementBatch, SettlementPlan,
};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::Arc;

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum OwnerPhaseSnapshot {
    PreAccepted {
        phase: PreAcceptedPhase,
        dependencies: KnownDependencies,
        original_charge: ResourceVector,
    },
    Accepted {
        status: AcceptedStatus,
        proof: AcceptedProof,
        dependencies: KnownDependencies,
        accepted_at: AcceptedAtMillis,
    },
    ReplacementHistory {
        dependencies: KnownDependencies,
        observation: DependencyCut,
    },
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OwnerSnapshot {
    identity: TxIdentity,
    source: OwnerSourceSnapshot,
    version: EntryVersion,
    arrival: Arrival,
    charge: ChargeRecord,
    phase: OwnerPhaseSnapshot,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnerSourceSnapshot {
    PreAccepted(PreAcceptedSource),
    Accepted(AcceptedProvenance),
    ReplacementHistory,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AuthoritySnapshot {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    entries: HashMap<RawTxHash, OwnerSnapshot>,
    indexes: IndexSnapshot,
    source_versions: AuthoritySourceVersions,
    resources: ResourceSnapshot,
    membership: MembershipSnapshot,
    scheduler: SchedulerSnapshot,
    dependencies: DependencySnapshot,
    effects: EffectSnapshot,
    clocks: AuthorityClocks,
}

#[cfg(test)]
impl AuthoritySnapshot {
    /// Reference equivalence for a commuting multi-owner Apply versus the
    /// same transitions applied one at a time.  Authority state and clocks
    /// must be identical; only effect-journal batch packaging may differ.
    pub(super) fn equivalent_modulo_effect_batching(&self, other: &Self) -> bool {
        self.generation == other.generation
            && self.chain_view == other.chain_view
            && self.entries == other.entries
            && self.indexes == other.indexes
            && self.source_versions == other.source_versions
            && self.resources == other.resources
            && self.membership == other.membership
            && self.scheduler == other.scheduler
            && self.dependencies == other.dependencies
            && self.effects.equivalent_stream(&other.effects)
            && self.clocks == other.clocks
    }
}

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

    #[cfg(test)]
    pub(super) fn new(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effect_limits: EffectLimits,
    ) -> Result<Self, AuthorityConfigError> {
        Self::from_runtime(
            limits,
            verify_order,
            effect_limits,
            MembershipConfig::testing_default(),
            ChainViewId::initial(),
        )
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
            membership_config,
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
        Self::assemble(
            limits,
            VerifyOrder::Arrival,
            EffectLog::for_foundation(),
            MembershipConfig::testing_default(),
            ChainViewId::initial(),
        )
    }

    #[cfg(test)]
    pub(super) fn for_foundation_with_order(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
    ) -> Self {
        Self::assemble(
            limits,
            verify_order,
            EffectLog::for_foundation(),
            MembershipConfig::testing_default(),
            ChainViewId::initial(),
        )
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

    pub(super) fn chain_revision(&self) -> ChainRevision {
        self.chain_view.revision()
    }

    pub(super) fn chain_view(&self) -> &ChainViewId {
        &self.chain_view
    }

    pub(super) fn generation(&self) -> PoolGeneration {
        self.generation
    }

    pub(super) fn clocks(&self) -> AuthorityClocks {
        self.clocks
    }

    /// Borrow one immutable authority cut for every query projection. The view
    /// exposes neither the primary owner enum nor independently captured
    /// accepted/preaccepted collections.
    pub(super) fn read_view(&self) -> AuthorityReadView<'_> {
        AuthorityReadView::new(
            self.generation,
            self.chain_view.clone(),
            self.clocks.next_sequence,
            &self.entries,
            &self.indexes,
            &self.membership,
            self.source_versions.template(),
        )
    }

    #[cfg(test)]
    pub(super) fn entries_for_reference(&self) -> &HashMap<RawTxHash, OwnedTx> {
        &self.entries
    }

    #[cfg(test)]
    pub(super) fn membership_snapshot_for_reference(&self) -> MembershipSnapshot {
        self.membership.snapshot()
    }

    /// Bounded strongest-first Ready identities for the runtime's sealed
    /// validation capture. Raw identities never cross the authority module.
    pub(in crate::authority) fn ready_candidates(&self) -> Vec<(RawTxHash, EntryVersion)> {
        self.scheduler.ready()
    }

    #[cfg(test)]
    pub(super) fn ready_for_reference(&self) -> Vec<(RawTxHash, EntryVersion)> {
        self.ready_candidates()
    }

    #[cfg(test)]
    pub(super) fn preaccepted_for_peer_for_reference(
        &self,
        peer: ckb_network::PeerIndex,
    ) -> Vec<RawTxHash> {
        let mut owners = self
            .indexes
            .preaccepted_for_peer(peer)
            .map_or_else(Vec::new, |owners| owners.iter().cloned().collect());
        owners.sort_unstable();
        owners
    }

    #[cfg(test)]
    pub(super) fn source_versions_for_reference(&self) -> (ApplySequence, ApplySequence) {
        (
            self.source_versions.accepted(),
            self.source_versions.status(),
        )
    }

    #[cfg(test)]
    pub(super) fn template_source_versions_for_reference(&self) -> PoolTemplateVersions {
        self.source_versions.template()
    }

    #[cfg(test)]
    pub(super) fn force_chain_view(&mut self, view: ChainViewId) {
        self.chain_view = view;
    }

    #[cfg(test)]
    pub(super) fn force_next_sequence(&mut self, sequence: ApplySequence) {
        self.clocks.next_sequence = sequence;
    }

    #[cfg(test)]
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
                        status: entry.status(),
                        proof: entry.proof.clone(),
                        dependencies: entry.proof.payload().dependencies().clone(),
                        accepted_at: entry.accepted_at,
                    },
                    OwnedTx::ReplacementHistory(entry) => OwnerPhaseSnapshot::ReplacementHistory {
                        dependencies: entry.dependencies().clone(),
                        observation: entry.observation().dependency_cut(),
                    },
                };
                let source = match owner {
                    OwnedTx::PreAccepted(entry) => OwnerSourceSnapshot::PreAccepted(entry.source),
                    OwnedTx::Accepted(entry) => OwnerSourceSnapshot::Accepted(entry.provenance),
                    OwnedTx::ReplacementHistory(_) => OwnerSourceSnapshot::ReplacementHistory,
                };
                (
                    hash.clone(),
                    OwnerSnapshot {
                        identity: record.identity.clone(),
                        source,
                        version: record.version,
                        arrival: record.arrival,
                        charge: owner.charge_record(),
                        phase,
                    },
                )
            })
            .collect();
        AuthoritySnapshot {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            entries,
            indexes: self.indexes.snapshot(),
            source_versions: self.source_versions,
            resources: self.resources.snapshot(),
            membership: self.membership.snapshot(),
            scheduler: self.scheduler.snapshot(),
            dependencies: self.dependencies.snapshot(),
            effects: self.effects.snapshot(),
            clocks: self.clocks,
        }
    }

    #[cfg(test)]
    pub(super) fn primary_projection_consistent(&self) -> bool {
        self.entries.len() == self.resources.charge_count()
            && self.entries.iter().all(|(hash, owner)| {
                self.resources.charge(hash) == Some(owner.charge_record())
                    && &owner.record().identity.raw == hash
            })
            && self.indexes.semantically_matches(&self.entries)
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
    Lease,
    Dependency,
    EffectLease,
    Generation,
    SourceVersion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthorityFault {
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
/// lease capability. Callers may turn it into a deterministic cancellation,
/// or discard it only after proving the reported error makes the lease stale.
#[derive(Debug)]
#[must_use = "a failed compute settlement still owns the active lease capability"]
pub(super) struct ComputeSettlementFailure {
    error: PlanError,
    token: SettlementToken,
    next: SettlementNext,
}

impl ComputeSettlementFailure {
    pub(super) fn error(&self) -> &PlanError {
        &self.error
    }

    pub(super) fn into_settlement(self) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: self.next,
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

/// Proof that the current generation has no outstanding compute capability.
/// It is move-only and rebound against generation, chain and owner-source
/// identity before a replacement can be planned.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct DrainedGenerationReceipt {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    owner_source: ApplySequence,
}

enum GenerationChainTarget {
    Preserve,
    Install(ChainViewId),
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
    GenerationControl(ApplySequence),
    AdminControl {
        sequence: ApplySequence,
        cause: AdminCause,
        changed_owners: usize,
    },
    ChainControl {
        sequence: ApplySequence,
        view: ChainViewId,
        changed_owners: usize,
    },
}

#[derive(Debug, Default)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing the move-only compute handoff would add one allocation to every successful hot-path checkout"
)]
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
    retired_generation: Option<RetiredGeneration>,
}

#[derive(Debug)]
struct RetiredGeneration {
    entries: HashMap<RawTxHash, OwnedTx>,
    _indexes: AuthorityIndexes,
    _resources: ResourceLedger,
    _membership: MembershipProjection,
    _scheduler: FairFrontier,
    _dependencies: DependencyFrontier,
}

impl RetiredGeneration {
    fn owner_count(&self) -> usize {
        self.entries.len()
    }
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

impl CommittedDelta {
    pub(in crate::authority) fn retired_len(&self) -> usize {
        self.retired.len().saturating_add(
            self.retired_generation
                .as_ref()
                .map_or(0, RetiredGeneration::owner_count),
        )
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
    after: Option<OwnedTx>,
    owners: DerivedOwnerDelta,
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

struct DerivedOwnerDelta {
    indexes: IndexDelta,
    sources: SourceVersionDelta,
}

struct TransitionControls {
    dependency: DependencyControlDelta,
    effect: EffectDelta,
    replacement_retirement: EntryReplacementRetirement,
}

impl Default for TransitionControls {
    fn default() -> Self {
        Self {
            dependency: DependencyControlDelta::default(),
            effect: EffectDelta::default(),
            replacement_retirement: EntryReplacementRetirement::SharedShellInline,
        }
    }
}

#[derive(Clone, Copy)]
enum EntryReplacementRetirement {
    SharedShellInline,
    OutsideGuard,
}

struct WorkHandoff {
    work: CheckedOutWork,
    origin: CheckoutOrigin,
}

#[cfg_attr(
    test,
    expect(
        clippy::large_enum_variant,
        reason = "the unit variant is test-only; boxing the production reservation would add a hot-path allocation"
    )
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

enum CheckoutEligibility {
    Ready {
        grant: ComputeGrant,
        after_charge: ResourceVector,
    },
    StaleDependency,
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
    changed_retirement: ChangedOwnerRetirement,
    removals: Vec<MembershipRemoval>,
    owners: DerivedOwnerDelta,
    resource: ResourceBatchPlan,
    projection: ProjectionDelta,
    scheduler: SchedulerDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    retired: Vec<OwnedTx>,
    clocks: AuthorityClocks,
    committed: CommittedChanges,
}

/// Source-specific validation produces this immutable input; the shared
/// membership compiler then handles RBF, capacity, projections and effects
/// identically for asynchronous and direct admission.
struct MembershipCompilation {
    key: RawTxHash,
    existing: Option<OwnedTx>,
    accepted: AcceptedEntry,
    prepared: PreparedMembership,
    base_clocks: AuthorityClocks,
    sequence: ApplySequence,
    effect_policy: EffectPolicy,
    changed_retirement: ChangedOwnerRetirement,
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

/// Public terminalization evidence for a pre-accepted owner.
///
/// Production has no silent variant: removing the owner necessarily consumes
/// a pre-built committed publication in the same Apply.  Foundation tests may
/// exercise dependency-loss mechanics without inventing a public outcome, but
/// that capability does not exist in a production build.
enum PreAcceptedTerminalOutcome<'publication> {
    Published(&'publication EffectPublication),
    #[cfg(test)]
    SilentFixture,
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

struct GenerationDelta {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    fresh: FreshGeneration,
    sources: SourceVersionDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    sequence: ApplySequence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdminCause {
    PeerRevocation(ckb_network::PeerIndex),
    RemoteExpiry { cutoff: RemoteDeadline },
}

struct AdminDelta {
    cause: AdminCause,
    hashes: Vec<RawTxHash>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    effect: EffectDelta,
    retired: Vec<OwnedTx>,
    clocks: AuthorityClocks,
    sequence: ApplySequence,
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
    sequence: ApplySequence,
}

enum AuthorityDelta {
    Entry(EntryDelta),
    Membership(MembershipDelta),
    Independent(IndependentDelta),
    Dependency(DependencyOnlyDelta),
    Effect(EffectOnlyDelta),
    Generation(GenerationDelta),
    Admin(AdminDelta),
    Chain(ChainDelta),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MissingResolutionDisposition {
    Wait,
    Reject(SettlementRejection),
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
    handoff: CommittedHandoff,
}

/// A prepared compute checkout whose post-Apply capability is present by
/// construction. Converting the private generic transition before mutation
/// keeps a missing compute handoff out of the runtime state space.
#[must_use = "a prepared checkout must be applied exactly once"]
pub(super) struct PreparedCheckout<'authority> {
    authority: &'authority mut TxPoolAuthority,
    delta: AuthorityDelta,
    work: CheckedOutWork,
}

#[must_use = "checked-out work and its retirement carrier must leave the authority guard together"]
pub(super) struct CommittedCheckout {
    work: CheckedOutWork,
    retirement: CommittedDelta,
}

impl PreparedCheckout<'_> {
    fn from_prepared(plan: PreparedApply<'_>) -> Result<PreparedCheckout<'_>, PlanError> {
        let PreparedApply {
            authority,
            delta,
            handoff,
        } = plan;
        let CommittedHandoff::Compute(work) = handoff else {
            return Err(PlanError::Fault(AuthorityFault::SchedulerProjection));
        };
        Ok(PreparedCheckout {
            authority,
            delta,
            work,
        })
    }

    pub(super) fn apply(self) -> CommittedCheckout {
        let Self {
            authority,
            delta,
            work,
        } = self;
        let retirement = PreparedApply {
            authority,
            delta,
            handoff: CommittedHandoff::None,
        }
        .apply();
        CommittedCheckout { work, retirement }
    }
}

impl CommittedCheckout {
    /// The runtime must keep `retirement` alive until after the authority guard
    /// opens; the compute job itself may then cross the worker boundary.
    pub(in crate::authority) fn into_parts(self) -> (CheckedOutWork, CommittedDelta) {
        (self.work, self.retirement)
    }

    /// Foundation fixtures own no runtime authority guard.
    #[cfg(test)]
    pub(in crate::authority) fn into_work(self) -> Option<CheckedOutWork> {
        Some(self.work)
    }

    #[cfg(test)]
    pub(in crate::authority) fn committed_delta_for_foundation(&self) -> &CommittedDelta {
        &self.retirement
    }
}

#[cfg(test)]
impl From<CommittedCheckout> for CommittedDelta {
    fn from(checkout: CommittedCheckout) -> Self {
        let CommittedCheckout {
            work,
            mut retirement,
        } = checkout;
        retirement.handoff = CommittedHandoff::Compute(work);
        retirement
    }
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

#[must_use = "direct duplicate outcome must be applied exactly once"]
pub(super) struct PreparedDirectDuplicate<'authority> {
    key: RawTxHash,
    plan: PreparedApply<'authority>,
}

impl PreparedDirectDuplicate<'_> {
    pub(super) fn key(&self) -> &RawTxHash {
        &self.key
    }

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
    pub(super) fn reason(&self) -> &MembershipReject {
        &self.reason
    }

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
    pub(super) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }

    pub(super) fn apply(self) -> (CommittedPublicReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
}

impl PreparedCandidateRejection<'_> {
    pub(super) fn reason(&self) -> &MembershipReject {
        &self.reason
    }

    pub(super) fn apply(self) -> (MembershipReject, CommittedDelta) {
        (self.reason, self.plan.apply())
    }
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
            AuthorityDelta::Generation(delta) => Self::apply_generation(authority, delta),
            AuthorityDelta::Admin(delta) => Self::apply_admin(authority, delta),
            AuthorityDelta::Chain(delta) => Self::apply_chain(authority, delta),
        }
    }

    fn apply_entry(
        authority: &mut TxPoolAuthority,
        delta: EntryDelta,
        handoff: CommittedHandoff,
    ) -> CommittedDelta {
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
        CommittedDelta {
            changes: CommittedChanges::One(CommittedChange {
                sequence: delta.sequence,
                changed: delta.key,
            }),
            handoff,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_membership(
        authority: &mut TxPoolAuthority,
        mut delta: MembershipDelta,
    ) -> CommittedDelta {
        let mut retired = delta.retired;
        for removal in &mut delta.removals {
            let previous = match removal.take_after() {
                Some(after) => authority.entries.insert(removal.hash.clone(), after),
                None => authority.entries.remove(&removal.hash),
            };
            if let Some(owner) = previous {
                retired.push(owner);
            }
        }
        let previous = authority
            .entries
            .insert(delta.changed_key, delta.changed_after);
        match delta.changed_retirement {
            ChangedOwnerRetirement::VacantOrSharedShellInline => drop(previous),
            ChangedOwnerRetirement::OutsideGuard => {
                if let Some(owner) = previous {
                    // Capacity was reserved while planning; Apply performs no
                    // allocation and the last payload reference dies only
                    // after the authority guard is released.
                    retired.push(owner);
                }
            }
        }
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
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
            retired_generation: None,
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
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
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
            retired_generation: None,
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
            retired_generation: None,
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
            retired_generation: None,
        }
    }

    fn apply_generation(authority: &mut TxPoolAuthority, delta: GenerationDelta) -> CommittedDelta {
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
        CommittedDelta {
            changes: CommittedChanges::GenerationControl(delta.sequence),
            handoff: CommittedHandoff::None,
            removals: Vec::new(),
            retired: Vec::new(),
            retired_effect,
            retired_generation: Some(retired_generation),
        }
    }

    fn apply_admin(authority: &mut TxPoolAuthority, delta: AdminDelta) -> CommittedDelta {
        let changed_owners = delta.hashes.len();
        let mut retired = delta.retired;
        for hash in &delta.hashes {
            if let Some(owner) = authority.entries.remove(hash) {
                retired.push(owner);
            }
        }
        authority.indexes.apply(delta.owners.indexes);
        authority.source_versions.apply(delta.owners.sources);
        authority.resources.apply_batch(delta.resources);
        authority.scheduler.apply_batch(delta.scheduler);
        authority.dependencies.apply_batch(delta.dependency);
        let retired_effect = authority.effects.apply(delta.effect);
        authority.clocks = delta.clocks;
        CommittedDelta {
            changes: CommittedChanges::AdminControl {
                sequence: delta.sequence,
                cause: delta.cause,
                changed_owners,
            },
            handoff: CommittedHandoff::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
        }
    }

    fn apply_chain(authority: &mut TxPoolAuthority, delta: ChainDelta) -> CommittedDelta {
        let changed_owners = delta.updates.len();
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
        CommittedDelta {
            changes: CommittedChanges::ChainControl {
                sequence: delta.sequence,
                view: delta.view,
                changed_owners,
            },
            handoff: CommittedHandoff::None,
            removals: Vec::new(),
            retired,
            retired_effect,
            retired_generation: None,
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
        if receipt.key() != &preaccepted.record.identity.raw
            || proof.payload().identity() != &preaccepted.record.identity
            || !proof.is_for(&self.chain_view)
        {
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

    #[cfg(test)]
    pub(super) fn independent_candidate_for_foundation(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<IndependentCandidate, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        Ok(IndependentCandidate::new(receipt))
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
                if candidate_inputs.contains(input) {
                    continue;
                }
                let Some(spender) = self.membership.spender(input) else {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                };
                if !removed.contains(spender) {
                    continue;
                }
                let chain_backed = victim.proof.is_chain_input(input);
                let parent = RawTxHash(input.tx_hash());
                let surviving_pool_parent = if removed.contains(&parent) {
                    false
                } else {
                    match self.entries.get(&parent) {
                        Some(OwnedTx::Accepted(entry)) => {
                            let index: u32 = input.index().unpack();
                            usize::try_from(index)
                                .ok()
                                .is_some_and(|index| index < entry.record.tx.outputs().len())
                        }
                        Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                            false
                        }
                    }
                };
                if chain_backed || surviving_pool_parent {
                    available.push(DependencyKey::Cell(input.clone()));
                }
            }
        }
        Ok(available)
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
    fn retain_replacement_history(
        &self,
        candidate: &AcceptedEntry,
        removals: &mut [MembershipRemoval],
        sequence: ApplySequence,
        mut version: EntryVersion,
        mut arrival: Arrival,
    ) -> Result<Option<(EntryVersion, Arrival)>, PlanError> {
        if !removals
            .iter()
            .any(|removal| removal.cause == RemovalCause::Replacement)
        {
            return Ok(Some((version, arrival)));
        }
        let mut removed = HashSet::new();
        if removed.try_reserve(removals.len()).is_err() {
            return Ok(None);
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
                return Ok(None);
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
                Err(super::state::DependencySetError::Allocation) => return Ok(None),
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
                version,
                arrival,
                DependencyCut(sequence),
            ) {
                Ok(history) => history,
                Err(ReplacementHistoryError::ResourceAllocation) => {
                    return Ok(None);
                }
                Err(ReplacementHistoryError::InvalidRecoveryTrigger) => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                Err(ReplacementHistoryError::ResourceArithmetic) => {
                    return Err(PlanError::Fault(AuthorityFault::CounterExhausted));
                }
            };
            removal.retain_replacement_history(history)?;
            version = next_version(version)?;
            arrival = next_arrival(arrival)?;
        }
        Ok(Some((version, arrival)))
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
        if matches!(
            admission.source,
            PreAcceptedSource::Recovery(lease) if lease.generation != self.generation
        ) {
            // Recovery is a generation-scoped chain capability, not a trusted
            // ingress flag. The chain receipt normally proves this cut; the
            // authority repeats the OCC fence so no alternate caller can
            // publish an old-generation owner.
            return Err(PlanError::Stale(StalePlan::Generation));
        }
        let admission = self.resources.charge_admission(admission)?;
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

        let version = self.clocks.next_version;
        let arrival = self.clocks.next_arrival;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_arrival: next_arrival(arrival)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
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
            lease: proposal,
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
            PreAcceptedSource::Remote(remote) => ProposalBase::Remote(remote),
            PreAcceptedSource::Proposal {
                lease: current,
                base,
            } => {
                if same_witness && current == proposal {
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

        let sequence = self.clocks.next_sequence;
        let trusted_proposal_base = match proposal_base {
            ProposalBase::Trusted => ProposalBase::Trusted,
            ProposalBase::Remote(remote) => ProposalBase::Remote(remote.with_trusted_payload()),
        };
        let (promoted, clocks) = if same_witness {
            // A policy-only promotion/refresh preserves EntryVersion and the
            // exact active lease. ActiveWork has already sealed its compute
            // attribution, so changing future scheduling trust cannot move
            // that capability between resource partitions.
            let mut promoted = entry.clone();
            promoted.source = PreAcceptedSource::Proposal {
                lease: proposal,
                base: trusted_proposal_base,
            };
            (
                promoted,
                AuthorityClocks {
                    next_sequence: next_sequence(sequence)?,
                    ..self.clocks
                },
            )
        } else {
            if matches!(entry.phase, PreAcceptedPhase::Computing(_)) {
                return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
            }
            let version = self.clocks.next_version;
            let promoted = PreAcceptedEntry {
                record: TxRecord {
                    tx: admission.tx,
                    identity: admission.identity,
                    version,
                    arrival: entry.record.arrival,
                },
                source: PreAcceptedSource::Proposal {
                    lease: proposal,
                    base: trusted_proposal_base,
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
            (
                promoted,
                AuthorityClocks {
                    next_version: next_version(version)?,
                    next_sequence: next_sequence(sequence)?,
                    ..self.clocks
                },
            )
        };
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
            lease: proposal,
            base: ProposalBase::Trusted,
        } = admission.source
        else {
            return Err(if same_witness {
                PlanError::Duplicate
            } else {
                PlanError::PayloadVariant
            });
        };
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let arrival = history.record().arrival;
        let promoted = if same_witness {
            let mut promoted = history.into_recovery(self.generation, version);
            promoted.source = PreAcceptedSource::Proposal {
                lease: proposal,
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
                    lease: proposal,
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
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
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

    #[cfg(test)]
    pub(super) fn plan_accept_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        self.plan_accept(receipt)
    }

    #[cfg(test)]
    pub(super) fn plan_candidate_disposition_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<CandidateDispositionPlan<'_>, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        self.plan_candidate_disposition(receipt)
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
        let policy = if preaccepted.source.ingress_peer().is_some() {
            EffectPolicy::Remote
        } else {
            EffectPolicy::Trusted
        };
        let publication = self
            .effects
            .build_publication(
                policy,
                vec![CommittedEffect::Rejected {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience: RejectionAudience::from_source(preaccepted.source),
                    reason: CommittedRejection::Validation(reason.clone()),
                }],
            )
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        let plan = self.plan_preaccepted_terminalization(
            subject.key(),
            subject.expected(),
            PreAcceptedTerminalOutcome::Published(&publication),
        )?;
        Ok(PreparedValidationRejection { reason, plan })
    }

    fn plan_final_reresolution(
        &mut self,
        retry: FinalAdmissionRetry,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let subject = retry.into_subject();
        let preaccepted = self.final_admission_subject_owner(&subject)?;
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut requeued = preaccepted.clone();
        requeued.record.version = version;
        requeued.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
        requeued.charge = preaccepted.original_charge();
        self.prepare_entry_delta_with_replacement_retirement(
            EntryTransition {
                key: subject.key().clone(),
                before: Some(OwnedTx::PreAccepted(preaccepted)),
                after: Some(OwnedTx::PreAccepted(requeued)),
            },
            clocks,
            sequence,
            EntryReplacementRetirement::OutsideGuard,
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
                handoff: CommittedHandoff::None,
            })),
            Err(PlanError::Membership(reason)) => {
                let existing = self
                    .entries
                    .get(&key)
                    .ok_or(PlanError::Stale(StalePlan::Missing))?;
                let OwnedTx::PreAccepted(preaccepted) = existing else {
                    return Err(PlanError::Stale(StalePlan::Phase));
                };
                let policy = if preaccepted.source.ingress_peer().is_some() {
                    EffectPolicy::Remote
                } else {
                    EffectPolicy::Trusted
                };
                let publication = self
                    .effects
                    .build_publication(
                        policy,
                        vec![CommittedEffect::Rejected {
                            tx: Arc::clone(&preaccepted.record.tx),
                            audience: RejectionAudience::from_source(preaccepted.source),
                            reason: CommittedRejection::Membership(reason.clone()),
                        }],
                    )
                    .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
                let plan = self.plan_preaccepted_terminalization(
                    &key,
                    expected,
                    PreAcceptedTerminalOutcome::Published(&publication),
                )?;
                Ok(CandidateDispositionPlan::Rejected(
                    PreparedCandidateRejection { reason, plan },
                ))
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    pub(super) fn plan_accept_context_sensitive_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_context_sensitive_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        self.plan_accept(receipt)
    }

    #[cfg(test)]
    pub(super) fn plan_direct_admission_for_foundation(
        &mut self,
        tx: Arc<TransactionView>,
        verified: super::state::VerifiedFacts,
        status: AcceptedStatus,
    ) -> Result<DirectAdmissionDisposition<'_>, PlanError> {
        let work = DirectAdmissionWork::new(tx, self.chain_view.clone(), verified)
            .map_err(Self::direct_admission_evidence_error)?;
        let receipt = work
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(Self::direct_admission_evidence_error)?;
        self.plan_direct_admission(receipt)
    }

    #[cfg(test)]
    fn direct_admission_evidence_error(error: AdmissionEvidenceError) -> PlanError {
        match error {
            AdmissionEvidenceError::TransactionIdentityMismatch => {
                PlanError::Fault(AuthorityFault::MembershipProjection)
            }
            AdmissionEvidenceError::ScriptRulesChanged => {
                PlanError::Stale(StalePlan::ChainRevision)
            }
        }
    }

    fn plan_direct_admission(
        &mut self,
        receipt: DirectAdmissionReceipt,
    ) -> Result<DirectAdmissionDisposition<'_>, PlanError> {
        self.effects.ensure_open()?;
        let key = receipt.key().clone();
        let existing = self.entries.get(&key).cloned();
        if let Some(OwnedTx::Accepted(accepted)) = &existing {
            let publication = self
                .effects
                .build_publication(
                    EffectPolicy::Trusted,
                    vec![CommittedEffect::Accepted {
                        tx: Arc::clone(receipt.transaction()),
                        status: accepted.status(),
                        cause: CommittedAcceptance::Duplicate {
                            requesting_peer: None,
                        },
                    }],
                )
                .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
            let sequence = self.clocks.next_sequence;
            let effect = self
                .effects
                .plan_publication(&publication, sequence)
                .map_err(PlanError::from)?;
            let plan = self.prepare_effect_only(effect, sequence, CommittedHandoff::None)?;
            return Ok(DirectAdmissionDisposition::Duplicate(
                PreparedDirectDuplicate { key, plan },
            ));
        }
        if matches!(
            &existing,
            Some(OwnedTx::PreAccepted(PreAcceptedEntry {
                phase: PreAcceptedPhase::Computing(_),
                ..
            }))
        ) {
            return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
        }
        self.validate_direct_acceptance_evidence(&receipt)?;

        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let (arrival, next_arrival) = match &existing {
            Some(owner) => (owner.record().arrival, self.clocks.next_arrival),
            None => (
                self.clocks.next_arrival,
                next_arrival(self.clocks.next_arrival)?,
            ),
        };
        let base_clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            next_arrival,
            ..self.clocks
        };
        let provenance = existing.as_ref().and_then(OwnedTx::ingress_peer).map_or(
            AcceptedProvenance::Trusted,
            |ingress| AcceptedProvenance::Peer {
                ingress,
                payload_policy: PayloadPolicy::Trusted,
            },
        );
        let (tx, proof, proposal, accepted_at) = receipt.into_membership_parts();
        let accepted = AcceptedEntry {
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
        };
        let prepared = match self.prepare_membership_candidate(&key, &accepted) {
            Ok(prepared) => prepared,
            Err(PlanError::Membership(reason)) => {
                let publication = self
                    .effects
                    .build_publication(
                        EffectPolicy::Trusted,
                        vec![CommittedEffect::Rejected {
                            tx: Arc::clone(&accepted.record.tx),
                            audience: RejectionAudience::default(),
                            reason: CommittedRejection::Membership(reason.clone()),
                        }],
                    )
                    .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
                let effect = self
                    .effects
                    .plan_publication(&publication, sequence)
                    .map_err(PlanError::from)?;
                let plan = self.prepare_effect_only(effect, sequence, CommittedHandoff::None)?;
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
            base_clocks,
            sequence,
            effect_policy: EffectPolicy::Trusted,
            changed_retirement: retirement,
        })?;
        Ok(DirectAdmissionDisposition::Accepted(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
            handoff: CommittedHandoff::None,
        }))
    }

    fn validate_direct_acceptance_evidence(
        &self,
        receipt: &DirectAdmissionReceipt,
    ) -> Result<(), PlanError> {
        if receipt.view() != &self.chain_view {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }
        let proof = receipt.proof();
        if receipt.key() != &proof.payload().identity().raw || !proof.is_for(&self.chain_view) {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        if !self
            .dependencies
            .proof_is_current(proof.payload().dependencies(), proof.dependency_cut())
        {
            return Err(PlanError::Stale(StalePlan::Dependency));
        }
        Ok(())
    }

    #[cfg(test)]
    fn plan_accept(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.prepare_accept_delta(receipt)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
            handoff: CommittedHandoff::None,
        })
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
        let (proof, proposal, accepted_at) = receipt.into_membership_parts();
        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let base_clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
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
        let effect_policy = if preaccepted.source.ingress_peer().is_some() {
            EffectPolicy::Remote
        } else {
            EffectPolicy::Trusted
        };
        self.compile_membership_delta(MembershipCompilation {
            key,
            existing: Some(existing),
            accepted,
            prepared,
            base_clocks,
            sequence,
            effect_policy,
            changed_retirement,
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
            base_clocks,
            sequence,
            effect_policy,
            changed_retirement,
        } = compilation;
        if existing.is_none() {
            self.reserve_primary_owner_insertions(1)?;
        }
        let PreparedMembership {
            mut removals,
            projection,
        } = prepared;
        let retained_clocks = self.retain_replacement_history(
            &accepted,
            &mut removals,
            sequence,
            base_clocks.next_version,
            base_clocks.next_arrival,
        )?;
        let mut clocks = base_clocks;
        if let Some((next_version, next_arrival)) = retained_clocks {
            if removals.iter().any(|removal| removal.after().is_some()) {
                clocks.next_version = next_version;
                clocks.next_arrival = next_arrival;
            }
        } else {
            removals.iter_mut().for_each(MembershipRemoval::terminalize);
        }

        let effect = self.plan_admission_effects(&accepted, &removals, sequence, effect_policy)?;
        let after = OwnedTx::Accepted(accepted);
        let has_history = removals.iter().any(|removal| removal.after().is_some());
        let resource =
            match self.plan_membership_resources(&key, existing.as_ref(), &after, &removals) {
                Ok(resource) => resource,
                Err(ResourceError::PreAcceptedLimit | ResourceError::ReplacementHistoryLimit)
                    if has_history =>
                {
                    removals.iter_mut().for_each(MembershipRemoval::terminalize);
                    clocks = base_clocks;
                    self.plan_membership_resources(&key, existing.as_ref(), &after, &removals)
                        .map_err(Self::membership_resource_error)?
                }
                Err(error) => return Err(Self::membership_resource_error(error)),
            };
        let changed_retirements = usize::from(
            existing.is_some()
                && matches!(changed_retirement, ChangedOwnerRetirement::OutsideGuard),
        );
        let retired_capacity = removals
            .len()
            .checked_add(changed_retirements)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let retired = retired_buffer(retired_capacity)?;
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
            changed_retirement,
            removals,
            owners,
            resource,
            projection,
            scheduler,
            dependency,
            effect,
            retired,
            clocks,
            committed: CommittedChanges::One(CommittedChange {
                sequence,
                changed: key,
            }),
        })
    }

    fn plan_admission_effects(
        &self,
        accepted: &AcceptedEntry,
        removals: &[MembershipRemoval],
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
        effects.push(CommittedEffect::Accepted {
            tx: Arc::clone(&accepted.record.tx),
            status: accepted.status(),
            cause: CommittedAcceptance::Admission {
                ingress_peer: accepted.provenance.ingress_peer(),
            },
        });
        for removal in removals {
            let owner = self
                .entries
                .get(&removal.hash)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let OwnedTx::Accepted(removed) = owner else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let reason = match removal.cause {
                RemovalCause::Replacement => CommittedRejection::ReplacedBy {
                    winner: accepted.record.identity.raw.clone(),
                },
                RemovalCause::Capacity => {
                    let cost = removed.proof.metrics().cost;
                    CommittedRejection::CapacityEvicted {
                        fee_rate: ckb_types::core::FeeRate::calculate(
                            removed.proof.metrics().fee,
                            get_transaction_weight(cost.serialized_bytes, cost.cycles),
                        ),
                    }
                }
            };
            effects.push(CommittedEffect::Rejected {
                tx: Arc::clone(&removed.record.tx),
                audience: RejectionAudience::from_owner(
                    owner.ingress_peer(),
                    owner.payload_blame_peer(),
                ),
                reason,
            });
        }
        let publication = self
            .effects
            .build_publication(policy, effects)
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.effects
            .plan_publication(&publication, sequence)
            .map_err(PlanError::from)
    }

    #[cfg(test)]
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
        if before.status() == status {
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
        after.proposal = super::chain::ProposalContextReceipt::from_validation(status);
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
        let dependency =
            self.plan_membership_dependency_delta(Some(&existing), &after, &[], sequence)?;
        let owners =
            self.plan_membership_owner_derivations(key, Some(&existing), &after, &[], sequence)?;
        let retired = Vec::new();
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(MembershipDelta {
                changed_key: key.clone(),
                changed_after: after,
                changed_retirement: ChangedOwnerRetirement::VacantOrSharedShellInline,
                removals: Vec::new(),
                owners,
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

    #[cfg(test)]
    pub(super) fn plan_terminalize_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_preaccepted_terminalization(
            key,
            expected,
            PreAcceptedTerminalOutcome::SilentFixture,
        )
    }

    #[cfg(test)]
    pub(super) fn plan_terminalize_with_effect_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_preaccepted_terminalization(
            key,
            expected,
            PreAcceptedTerminalOutcome::Published(publication),
        )
    }

    fn plan_preaccepted_terminalization(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        outcome: PreAcceptedTerminalOutcome<'_>,
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
        let effect = match outcome {
            PreAcceptedTerminalOutcome::Published(publication) => {
                self.effects.plan_publication(publication, sequence)?
            }
            #[cfg(test)]
            PreAcceptedTerminalOutcome::SilentFixture => EffectDelta::default(),
        };
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
                ..TransitionControls::default()
            },
            None,
        )
    }

    #[cfg(test)]
    pub(super) fn effect_publication_for_foundation(
        &self,
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
    ) -> Result<EffectPublication, EffectBuildError> {
        self.effects.build_publication(policy, effects)
    }

    #[cfg(test)]
    pub(super) fn plan_effect_publication_for_foundation(
        &mut self,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let sequence = self.clocks.next_sequence;
        let effect = self.effects.plan_publication(publication, sequence)?;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::None)
    }

    #[cfg(test)]
    pub(super) fn plan_generation_reset_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let sequence = self.clocks.next_sequence;
        let effect = self.effects.plan_generation_reset(sequence)?;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::None)
    }

    pub(super) fn drained_generation_receipt_for_foundation(
        &self,
    ) -> Result<DrainedGenerationReceipt, PlanError> {
        if self.resources.preaccepted().active_work != 0 {
            return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
        }
        Ok(DrainedGenerationReceipt {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            owner_source: self.source_versions.owners(),
        })
    }

    pub(super) fn plan_clear_generation_for_foundation(
        &mut self,
        receipt: DrainedGenerationReceipt,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_generation_replacement(receipt, GenerationChainTarget::Preserve)
    }

    pub(super) fn plan_replace_generation_chain_for_foundation(
        &mut self,
        receipt: DrainedGenerationReceipt,
        chain_view: ChainViewId,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_generation_replacement(receipt, GenerationChainTarget::Install(chain_view))
    }

    fn plan_generation_replacement(
        &mut self,
        receipt: DrainedGenerationReceipt,
        target: GenerationChainTarget,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.ensure_open()?;
        if receipt.generation != self.generation {
            return Err(PlanError::Stale(StalePlan::Generation));
        }
        if receipt.chain_view != self.chain_view {
            return Err(PlanError::Stale(StalePlan::ChainRevision));
        }
        if receipt.owner_source != self.source_versions.owners() {
            return Err(PlanError::Stale(StalePlan::SourceVersion));
        }
        if self.resources.preaccepted().active_work != 0 {
            return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
        }
        let chain_view = match target {
            GenerationChainTarget::Preserve => self.chain_view.clone(),
            GenerationChainTarget::Install(chain_view) => {
                if chain_view.revision() != next_chain_revision(self.chain_revision())? {
                    return Err(PlanError::Stale(StalePlan::ChainRevision));
                }
                chain_view
            }
        };
        let generation = next_generation(self.generation)?;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let effect = self.effects.plan_generation_reset(sequence)?;
        let sources = self.source_versions.plan_generation_replacement(sequence);
        let fresh = FreshGeneration::empty(&self.resources, &self.scheduler);
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Generation(GenerationDelta {
                generation,
                chain_view,
                fresh,
                sources,
                effect,
                clocks,
                sequence,
            }),
            handoff: CommittedHandoff::None,
        })
    }

    fn plan_administrative_removal(
        &mut self,
        mut hashes: Vec<RawTxHash>,
        cause: AdminCause,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.ensure_open()?;
        let mut owner_refs = Vec::new();
        owner_refs
            .try_reserve(hashes.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for hash in &hashes {
            let owner = self
                .entries
                .get(hash)
                .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?;
            let OwnedTx::PreAccepted(entry) = owner else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            match cause {
                AdminCause::PeerRevocation(peer) => {
                    if entry.source.ingress_peer() != Some(peer) {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    }
                }
                AdminCause::RemoteExpiry { cutoff } => match entry.source {
                    PreAcceptedSource::Remote(remote) if remote.residency.expires_at <= cutoff => {}
                    PreAcceptedSource::Remote(_)
                    | PreAcceptedSource::Proposal { .. }
                    | PreAcceptedSource::Recovery(_) => {
                        return Err(PlanError::Fault(AuthorityFault::IndexProjection));
                    }
                },
            }
            if matches!(&entry.phase, PreAcceptedPhase::Computing(_)) {
                return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
            }
            owner_refs.push(owner);
        }

        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let mut effects = Vec::new();
        effects
            .try_reserve(owner_refs.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        for owner in &owner_refs {
            let effect = match cause {
                AdminCause::PeerRevocation(peer) => CommittedEffect::PeerRevoked {
                    tx_hash: owner.record().identity.raw.clone(),
                    peer,
                },
                AdminCause::RemoteExpiry { .. } => {
                    let peer = owner
                        .ingress_peer()
                        .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?;
                    CommittedEffect::RemoteExpired {
                        tx_hash: owner.record().identity.raw.clone(),
                        peer,
                    }
                }
            };
            effects.push(effect);
        }
        let effect = match cause {
            AdminCause::PeerRevocation(_) => {
                self.effects.plan_critical_rebuildable(effects, sequence)?
            }
            AdminCause::RemoteExpiry { .. } => {
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
                owner_refs.truncate(selected.get());
                self.effects.plan_publication(&publication, sequence)?
            }
        };

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
            .plan_batch(owner_refs.iter().map(|owner| (Some(*owner), None)))?;
        let dependency_control = self.plan_dependency_loss(owner_refs.iter().copied(), sequence)?;
        let dependency = self
            .dependencies
            .plan_replacements(owner_refs.iter().map(|owner| (Some(*owner), None)))?
            .with_control(dependency_control);
        let sources = self.source_versions.plan_replacements(
            owner_refs.iter().map(|owner| (Some(*owner), None)),
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
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Admin(AdminDelta {
                cause,
                hashes,
                owners,
                resources,
                scheduler,
                dependency,
                effect,
                retired,
                clocks,
                sequence,
            }),
            handoff: CommittedHandoff::None,
        })
    }

    /// Remove the complete bounded pre-accepted cohort owned by one banned
    /// ingress peer. Accepted membership is deliberately absent from the peer
    /// index: a commit before the external ban fence wins, while a fence-first
    /// race reaches this transition. Active compute must first return its
    /// unique settlement capability; it is never made stale by deletion.
    pub(super) fn plan_peer_revocation_for_foundation(
        &mut self,
        peer: ckb_network::PeerIndex,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let Some(indexed) = self.indexes.preaccepted_for_peer(peer) else {
            return Ok(None);
        };
        let mut hashes = Vec::new();
        hashes
            .try_reserve(indexed.len())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        hashes.extend(indexed.iter().cloned());
        hashes.sort_unstable();
        self.plan_administrative_removal(hashes, AdminCause::PeerRevocation(peer))
            .map(Some)
    }

    /// Remove up to `limit` inactive Remote owners whose retained residency
    /// lease has elapsed. Computing owners keep their unique settlement
    /// capability and are skipped; the scan expansion is bounded by the
    /// globally charged active-work population, so one slow worker cannot
    /// head-of-line block unrelated expiry.
    pub(super) fn plan_remote_expiry_for_foundation(
        &mut self,
        cutoff: RemoteDeadline,
        limit: NonZeroUsize,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let scan_limit = limit
            .get()
            .checked_add(self.resources.preaccepted().active_work)
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let due = self.indexes.due_remote(cutoff, scan_limit)?;
        if due.is_empty() {
            return Ok(None);
        }
        let mut hashes = Vec::new();
        hashes
            .try_reserve(limit.get())
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
        let mut blocked = false;
        for candidate in due {
            let owner = self
                .entries
                .get(&candidate.hash)
                .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?;
            let OwnedTx::PreAccepted(entry) = owner else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            if entry.record.version != candidate.version {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
            let PreAcceptedSource::Remote(remote) = entry.source else {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            };
            if remote.residency.expires_at != candidate.expires_at {
                return Err(PlanError::Fault(AuthorityFault::IndexProjection));
            }
            if matches!(entry.phase, PreAcceptedPhase::Computing(_)) {
                blocked = true;
                continue;
            }
            hashes.push(candidate.hash);
            if hashes.len() == limit.get() {
                break;
            }
        }
        if hashes.is_empty() {
            return if blocked {
                Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain))
            } else {
                Err(PlanError::Fault(AuthorityFault::IndexProjection))
            };
        }
        self.plan_administrative_removal(hashes, AdminCause::RemoteExpiry { cutoff })
            .map(Some)
    }

    pub(super) fn plan_effect_checkout(&mut self) -> Result<Option<PreparedApply<'_>>, PlanError> {
        let Some((effect, lease)) = self.effects.plan_checkout()? else {
            return Ok(None);
        };
        let sequence = self.clocks.next_sequence;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::Effect(lease))
            .map(Some)
    }

    pub(super) fn apply_effect_settlement(
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

    pub(super) fn plan_effect_close(&mut self) -> Result<PreparedApply<'_>, PlanError> {
        if self.resources.preaccepted().active_work != 0 {
            return Err(PlanError::Backpressure(Backpressure::ActiveWorkDrain));
        }
        let effect = self.effects.plan_close()?;
        let sequence = self.clocks.next_sequence;
        self.prepare_effect_only(effect, sequence, CommittedHandoff::None)
    }

    pub(super) fn effects_closed_and_drained(&self) -> bool {
        self.effects.is_closed_and_drained()
    }

    #[cfg(test)]
    pub(super) fn plan_effect_checkout_for_foundation(
        &mut self,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        self.plan_effect_checkout()
    }

    #[cfg(test)]
    pub(super) fn apply_effect_settlement_for_foundation(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<CommittedDelta, EffectSettlementFailure> {
        self.apply_effect_settlement(settlement)
    }

    #[cfg(test)]
    pub(super) fn plan_effect_close_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_effect_close()
    }

    #[cfg(test)]
    pub(super) fn effects_closed_and_drained_for_foundation(&self) -> bool {
        self.effects_closed_and_drained()
    }

    #[cfg(test)]
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
        let action = ticket.action(
            &self.dependencies,
            hash.as_ref().and_then(|hash| self.entries.get(hash)),
        )?;
        let control = self.dependencies.plan_maintenance(ticket)?;
        let sequence = self.clocks.next_sequence;
        match action {
            DependencyMaintenanceAction::Advance => {
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
            DependencyMaintenanceAction::Requeue => {}
        }

        let hash = hash.ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let existing = self
            .entries
            .get(&hash)
            .cloned()
            .ok_or(PlanError::Fault(AuthorityFault::DependencyProjection))?;
        let version = self.clocks.next_version;
        let after = match existing.clone() {
            OwnedTx::PreAccepted(preaccepted) => existing
                .with_foundation_phase(
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
    ) -> Result<PreparedCheckout<'_>, PlanError> {
        self.plan_selected_checkout(key, expected, permit, CheckoutOrigin::Foundation)
    }

    /// Select and reserve one exact scheduler head. Saturated peer/source
    /// owners are skipped without publishing a second blocked-owner state.
    pub(super) fn plan_checkout_next(
        &mut self,
        permit: super::state::WorkPermit,
    ) -> Result<Option<PreparedCheckout<'_>>, PlanError> {
        let search = self.search_checkout(permit)?;
        self.prepare_checkout_search(search, permit)
    }

    #[cfg(test)]
    pub(super) fn plan_checkout_next_with_probe_count_for_foundation(
        &mut self,
        permit: super::state::WorkPermit,
    ) -> Result<(Option<PreparedCheckout<'_>>, usize), PlanError> {
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
    ) -> Result<Option<PreparedCheckout<'_>>, PlanError> {
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
        let attribution = preaccepted.source.compute_attribution();
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
        let (grant, after_charge) = match self.checkout_eligibility(preaccepted, permit)? {
            CheckoutEligibility::Ready {
                grant,
                after_charge,
            } => (grant, after_charge),
            CheckoutEligibility::StaleDependency => return Ok(CheckoutResource::SkipOwner),
        };
        let expected_charge = existing.charge_record();
        let after_record = ChargeRecord::PreAccepted {
            resources: after_charge,
            residency_peer: preaccepted.source.ingress_peer(),
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

    fn plan_selected_checkout(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::state::WorkPermit,
        origin: CheckoutOrigin,
    ) -> Result<PreparedCheckout<'_>, PlanError> {
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
            CheckoutOrigin::Foundation => match self.checkout_eligibility(preaccepted, permit)? {
                CheckoutEligibility::Ready {
                    grant,
                    after_charge,
                } => (grant, after_charge),
                CheckoutEligibility::StaleDependency => {
                    return Err(PlanError::Stale(StalePlan::Dependency));
                }
            },
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
            },
            chain_view: self.chain_view.clone(),
            dependency_cut,
            permit,
            grant,
            payload_policy: preaccepted.source.payload_policy(),
        };
        let work = CheckedOutWork::new(
            token,
            Arc::clone(&preaccepted.record.tx),
            preaccepted.basis.dependencies().clone(),
            queued.clone(),
        )
        .map_err(|_| PlanError::Stale(StalePlan::Phase))?;
        let active_dependencies = preaccepted.dependencies().clone();
        let attribution = preaccepted.source.compute_attribution();
        let after = existing
            .with_foundation_phase(
                PreAcceptedPhase::Computing(super::state::ActiveWork {
                    lease,
                    chain_view: self.chain_view.clone(),
                    permit,
                    grant,
                    attribution,
                    payload_policy: preaccepted.source.payload_policy(),
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
        let plan = self.prepare_entry_delta(
            EntryTransition {
                key: key.clone(),
                before: Some(existing.clone()),
                after: Some(after),
            },
            clocks,
            sequence,
            Some(WorkHandoff { work, origin }),
        )?;
        PreparedCheckout::from_prepared(plan)
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
            Err(PrepareSettlementError::Recompute(error)) => Err(ComputeSettlementFailure {
                error,
                token,
                next: SettlementNext::Retry,
            }),
            Err(PrepareSettlementError::Preserve { error, next }) => {
                Err(ComputeSettlementFailure { error, token, next })
            }
        }
    }

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
        if active.lease != token.lease {
            return Err(PlanError::Stale(StalePlan::Lease).into());
        }
        // Entry version and lease decide completion authority. Chain identity
        // decides only whether the resulting proof may be retained: a tip
        // change cannot invalidate the sole capability able to release this
        // Computing owner and its active charge.
        let chain_state_is_current = self.chain_view.has_same_chain_state(&active.chain_view);
        let dependency_cut = active.dependency_cut;
        let raw_charge = preaccepted.original_charge();
        if preaccepted.charge.active_work != 1 {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection).into());
        }
        let base_proof_is_current = self
            .dependencies
            .proof_is_current(preaccepted.dependencies(), dependency_cut);
        let disposition = if !base_proof_is_current {
            SettlementDisposition::Retain {
                phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                charge: raw_charge,
            }
        } else {
            match next {
                SettlementNext::QueuedVerify(resolved) => {
                    if resolved.payload().identity() != &preaccepted.record.identity {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection).into());
                    }
                    if resolved.chain_view() != &active.chain_view {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection).into());
                    }
                    if resolved.dependency_cut() != dependency_cut {
                        return Err(PlanError::Fault(AuthorityFault::DependencyProjection).into());
                    }
                    let dependencies = resolved.payload().dependencies().clone();
                    let retained_charge = self
                        .resources
                        .retained_entry_charge(
                            preaccepted,
                            resolved.payload().resolved_resident_bytes(),
                            dependencies.len(),
                        )
                        .map_err(|_| PlanError::Fault(AuthorityFault::ResourceProjection))?;
                    if self.dependencies.resolution_is_current(
                        preaccepted.dependencies(),
                        &dependencies,
                        dependency_cut,
                    ) {
                        SettlementDisposition::Retain {
                            phase: PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
                            charge: retained_charge,
                        }
                    } else {
                        SettlementDisposition::Retain {
                            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                            charge: raw_charge,
                        }
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
                        match self
                            .missing_resolution_disposition(preaccepted.source, missing.missing())
                        {
                            MissingResolutionDisposition::Reject(rejection) => {
                                SettlementDisposition::Terminal(rejection)
                            }
                            MissingResolutionDisposition::Wait => {
                                let retained_charge = self
                                    .resources
                                    .retained_entry_charge(
                                        preaccepted,
                                        preaccepted.basis.payload_bytes(),
                                        dependencies.len(),
                                    )
                                    .map_err(|_| {
                                        PlanError::Fault(AuthorityFault::ResourceProjection)
                                    })?;
                                let observed = self.dependencies.observe_missing(
                                    missing.missing(),
                                    dependencies,
                                    dependency_cut,
                                );
                                let publication = match preaccepted.source {
                                    PreAcceptedSource::Remote(remote) => {
                                        ParentTransactionRequest::new(
                                            remote.residency.peer,
                                            Arc::clone(missing.parent_transactions()),
                                        )
                                        .map(|request| {
                                            self.effects.build_publication(
                                                EffectPolicy::Remote,
                                                vec![CommittedEffect::ParentTransactionsRequested(
                                                    request,
                                                )],
                                            )
                                        })
                                        .transpose()
                                        .map_err(|_| {
                                            PlanError::Fault(AuthorityFault::EffectProjection)
                                        })?
                                    }
                                    PreAcceptedSource::Proposal { .. }
                                    | PreAcceptedSource::Recovery(_) => None,
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
                    } else {
                        SettlementDisposition::Retain {
                            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                            charge: raw_charge,
                        }
                    }
                }
                SettlementNext::Ready(verified) => {
                    if verified.witness() != &preaccepted.record.identity.witness
                        || verified.payload().identity() != &preaccepted.record.identity
                    {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection).into());
                    }
                    if verified.chain_view() != &active.chain_view {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection).into());
                    }
                    if verified.dependency_cut() != dependency_cut {
                        return Err(PlanError::Fault(AuthorityFault::DependencyProjection).into());
                    }
                    let dependencies = verified.payload().dependencies().clone();
                    let retained_charge = self
                        .resources
                        .retained_entry_charge(
                            preaccepted,
                            verified.metrics().cost.resident_bytes,
                            dependencies.len(),
                        )
                        .map_err(|_| PlanError::Fault(AuthorityFault::ResourceProjection))?;
                    if self.dependencies.resolution_is_current(
                        preaccepted.dependencies(),
                        &dependencies,
                        dependency_cut,
                    ) {
                        SettlementDisposition::Retain {
                            phase: PreAcceptedPhase::Ready(verified),
                            charge: retained_charge,
                        }
                    } else {
                        SettlementDisposition::Retain {
                            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                            charge: raw_charge,
                        }
                    }
                }
                SettlementNext::Rejected(rejection) => {
                    if chain_state_is_current || rejection.remains_valid_after_chain_change() {
                        SettlementDisposition::Terminal(rejection)
                    } else {
                        SettlementDisposition::Retain {
                            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                            charge: raw_charge,
                        }
                    }
                }
                SettlementNext::VerificationRejected {
                    rejection,
                    resolved,
                } => {
                    if resolved.payload().identity() != &preaccepted.record.identity
                        || resolved.chain_view() != &active.chain_view
                        || resolved.dependency_cut() != dependency_cut
                    {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection).into());
                    }
                    let current_policy = preaccepted.source.payload_policy();
                    if current_policy == active.payload_policy {
                        if chain_state_is_current {
                            SettlementDisposition::Terminal(SettlementRejection::ChainBound(
                                rejection,
                            ))
                        } else {
                            SettlementDisposition::Retain {
                                phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                                charge: raw_charge,
                            }
                        }
                    } else if matches!(
                        active.payload_policy,
                        PayloadPolicy::RemoteDeclaredCycles(_)
                    ) && current_policy == PayloadPolicy::Trusted
                    {
                        if !chain_state_is_current {
                            SettlementDisposition::Retain {
                                phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                                charge: raw_charge,
                            }
                        } else {
                            let dependencies = resolved.payload().dependencies().clone();
                            let retained_charge = self
                                .resources
                                .retained_entry_charge(
                                    preaccepted,
                                    resolved.payload().resolved_resident_bytes(),
                                    dependencies.len(),
                                )
                                .map_err(|_| {
                                    PlanError::Fault(AuthorityFault::ResourceProjection)
                                })?;
                            if self.dependencies.resolution_is_current(
                                preaccepted.dependencies(),
                                &dependencies,
                                dependency_cut,
                            ) {
                                SettlementDisposition::Retain {
                                    phase: PreAcceptedPhase::Queued(QueuedWork::Verify(resolved)),
                                    charge: retained_charge,
                                }
                            } else {
                                SettlementDisposition::Retain {
                                    phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                                    charge: raw_charge,
                                }
                            }
                        }
                    } else {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection).into());
                    }
                }
                SettlementNext::Retry => SettlementDisposition::Retain {
                    phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                    charge: raw_charge,
                },
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
        let grant_ceiling = ResourceVector::new(
            1,
            active.grant.max_resident_bytes,
            active.grant.max_edges,
            0,
        );
        if !retained_charge.fits(grant_ceiling) {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection).into());
        }
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
        let effect = publication
            .as_ref()
            .map_or_else(
                || Ok(EffectDelta::default()),
                |publication| self.effects.plan_publication(publication, sequence),
            )
            .map_err(PlanError::from)?;
        self.prepare_entry_delta_with_controls(
            EntryTransition {
                key: token.hash.clone(),
                before: Some(existing),
                after: Some(after),
            },
            clocks,
            sequence,
            None,
            TransitionControls {
                effect,
                ..TransitionControls::default()
            },
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
        let policy = if preaccepted.source.ingress_peer().is_some() {
            EffectPolicy::Remote
        } else {
            EffectPolicy::Trusted
        };
        let publication = self
            .effects
            .build_publication(
                policy,
                vec![CommittedEffect::Rejected {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience: RejectionAudience::from_source(preaccepted.source),
                    reason: CommittedRejection::Validation(rejection.into_public()),
                }],
            )
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        let dependency = self.plan_dependency_loss(std::iter::once(&existing), sequence)?;
        let effect = self.effects.plan_publication(&publication, sequence)?;
        let key = preaccepted.record.identity.raw.clone();
        self.prepare_entry_delta_with_controls(
            EntryTransition {
                key,
                before: Some(existing),
                after: None,
            },
            clocks,
            sequence,
            None,
            TransitionControls {
                dependency,
                effect,
                ..TransitionControls::default()
            },
            None,
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
                ..TransitionControls::default()
            },
            None,
        )
    }

    fn prepare_entry_delta_with_replacement_retirement(
        &mut self,
        transition: EntryTransition,
        clocks: AuthorityClocks,
        sequence: ApplySequence,
        replacement_retirement: EntryReplacementRetirement,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.prepare_entry_delta_with_controls(
            transition,
            clocks,
            sequence,
            None,
            TransitionControls {
                replacement_retirement,
                ..TransitionControls::default()
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
        let TransitionControls {
            dependency: dependency_control,
            effect,
            replacement_retirement,
        } = controls;
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
        let retirement = match (expected.is_some(), after.is_none(), replacement_retirement) {
            (true, true, _) | (true, false, EntryReplacementRetirement::OutsideGuard) => {
                EntryRetirement::Outside(retired_buffer(1)?)
            }
            (false, _, _) | (true, false, EntryReplacementRetirement::SharedShellInline) => {
                EntryRetirement::InlineDrop
            }
        };
        self.reserve_primary_owner_insertions(usize::from(after.is_some() && expected.is_none()))?;
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
            .with_control(dependency_control);
        let sources = self.source_versions.plan_replacements(
            std::iter::once((expected.as_ref(), after.as_ref())),
            sequence,
        );
        let indexes = self
            .indexes
            .plan_replace(&key, expected.as_ref(), after.as_ref())?;
        let owners = DerivedOwnerDelta { indexes, sources };
        let handoff = work.map_or(CommittedHandoff::None, CommittedHandoff::Compute);
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
            source: entry.source,
            basis: entry.basis.clone(),
            phase,
            charge,
        }))
    }
}
