use super::super::{
    chain::{
        AcceptedProof, DirectAdmissionError, DirectAdmissionWork,
        test_support::{AdmissionEvidenceError, ChainTransitionFacts},
    },
    dependency::test_support::{
        DependencyMaintenanceRank, DependencyMaintenanceRankError, DependencySnapshot,
    },
    effect::test_support::{EffectObservation, EffectSnapshot, EffectTraceBatch},
    indexes::test_support::IndexSnapshot,
    resources::{ActiveWorkAvailability, test_support::ResourceSnapshot},
    scheduler::{CheckoutTicket, test_support::SchedulerSnapshot},
    state::{AcceptedStatus, ActiveWork, ComputeAttribution, TxIdentity},
    work::{CheckedOutWork, LeaseToken},
};
use super::*;
use crate::authority::ingress::RetainedIngress;
use ckb_types::core::TransactionView;
use ckb_verification::cache::ScriptVerificationRules;

#[derive(Debug)]
pub(in crate::authority) enum DependencyMaintenanceDrainError {
    Rank(DependencyMaintenanceRankError),
    Plan(PlanError),
    Allocation,
    MissingObservation,
    MissingSuccessor(DependencyMaintenanceRank),
    Nondecreasing {
        before: DependencyMaintenanceRank,
        after: DependencyMaintenanceRank,
    },
    ResidualSuccessor,
}

impl std::fmt::Display for DependencyMaintenanceDrainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rank(error) => write!(formatter, "dependency rank failure: {error:?}"),
            Self::Plan(error) => write!(formatter, "dependency Plan failure: {error:?}"),
            Self::Allocation => formatter.write_str("dependency drain observation allocation"),
            Self::MissingObservation => {
                formatter.write_str("positive dependency rank has no next observation")
            }
            Self::MissingSuccessor(rank) => {
                write!(
                    formatter,
                    "dependency rank {rank:?} has no sealed successor"
                )
            }
            Self::Nondecreasing { before, after } => write!(
                formatter,
                "dependency maintenance rank did not decrease: {before:?} -> {after:?}"
            ),
            Self::ResidualSuccessor => {
                formatter.write_str("zero dependency rank still has a successor")
            }
        }
    }
}

impl From<DependencyMaintenanceRankError> for DependencyMaintenanceDrainError {
    fn from(error: DependencyMaintenanceRankError) -> Self {
        Self::Rank(error)
    }
}

impl From<PlanError> for DependencyMaintenanceDrainError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct DependencyMaintenanceObservation {
    key: DependencyKey,
    hash: Option<RawTxHash>,
    owner_requeued: bool,
    before_rank: DependencyMaintenanceRank,
    after_rank: DependencyMaintenanceRank,
}

impl DependencyMaintenanceObservation {
    pub(in crate::authority) fn key(&self) -> &DependencyKey {
        &self.key
    }

    pub(in crate::authority) fn hash(&self) -> Option<&RawTxHash> {
        self.hash.as_ref()
    }

    pub(in crate::authority) const fn owner_requeued(&self) -> bool {
        self.owner_requeued
    }

    pub(in crate::authority) const fn before_rank(&self) -> DependencyMaintenanceRank {
        self.before_rank
    }

    pub(in crate::authority) const fn after_rank(&self) -> DependencyMaintenanceRank {
        self.after_rank
    }
}

pub(in crate::authority) use super::super::rejection::ComponentLimitKind;
pub(in crate::authority) use super::compute_exchange::test_support::ComputeExchangeRecovery;
pub(in crate::authority) use super::membership::StatusCounts;
pub(in crate::authority) use super::membership::test_support::MembershipSnapshot;
pub(in crate::authority) use super::settlement::test_support::CandidateBatchError;

/// Test-only sequential checkout oracle. Production checkout is owned solely
/// by the bounded compute exchange; keeping this oracle in test support makes
/// the differential fold explicit without compiling a second mutation path
/// into the service.
struct CheckoutReservation {
    resources: ResourcePlan,
    grant: ComputeGrant,
    after_charge: ResourceVector,
}

enum CheckoutUnavailable {
    SkipOwner,
    Stop,
}

type CheckoutResource = Result<CheckoutReservation, CheckoutUnavailable>;

struct CheckoutSearch {
    selected: Option<(CheckoutTicket, CheckoutReservation)>,
    probes: usize,
}

#[must_use = "the sequential checkout oracle has no effect until applied"]
pub(in crate::authority) struct PreparedCheckout<'authority> {
    plan: PreparedApply<'authority>,
    work: CheckedOutWork,
}

#[must_use = "test checkout work and retirement must leave the oracle guard together"]
pub(in crate::authority) struct CommittedCheckout {
    work: CheckedOutWork,
    retirement: CommittedDelta,
}

impl PreparedCheckout<'_> {
    pub(in crate::authority) fn apply(self) -> CommittedCheckout {
        let Self { plan, work } = self;
        CommittedCheckout {
            work,
            retirement: plan.apply(),
        }
    }
}

impl CommittedCheckout {
    pub(in crate::authority) fn into_parts(self) -> (CheckedOutWork, CommittedDelta) {
        (self.work, self.retirement)
    }
}

/// Sequential retained-ingress oracle used only to refine the production
/// ordered batch against the canonical no-interleave fold.
pub(in crate::authority) enum RetainedAdmissionDisposition<'authority> {
    Retained(PreparedApply<'authority>),
    AcceptedDuplicate(PreparedApply<'authority>),
    RemoteReleased(PreparedApply<'authority>),
    ProposalUnchanged,
    ProposalPayloadVariant,
}

impl CommittedRetainedAdmissionBatch {
    pub(in crate::authority) const fn consumed(&self) -> usize {
        match self {
            Self::Unchanged { consumed, .. } | Self::Applied { consumed, .. } => *consumed,
        }
    }
}

impl TxPoolAuthority {
    pub(in crate::authority) fn plan_retained_admission(
        &mut self,
        ingress: RetainedIngress,
    ) -> Result<RetainedAdmissionDisposition<'_>, PlanError> {
        let (kind, admission) = ingress.into_parts();
        let key = admission.identity.raw.clone();
        if let RetainedIngressKind::Remote(peer) = kind
            && self.peer_bans.contains_at(peer, Instant::now())
        {
            return self
                .plan_single_effect(
                    EffectPolicy::Remote,
                    CommittedEffect::RemoteIngressReleased(
                        CommittedRemoteIngressRelease::unretained_remote_submission(key, peer),
                    ),
                )
                .map(RetainedAdmissionDisposition::RemoteReleased);
        }

        match kind {
            RetainedIngressKind::Remote(peer) => match self.entries.get(&key) {
                Some(OwnedTx::Accepted(_)) => {
                    return self
                        .plan_single_effect(
                            EffectPolicy::Remote,
                            CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                                tx_hash: key,
                                requesting_peer: Some(peer),
                            }),
                        )
                        .map(RetainedAdmissionDisposition::AcceptedDuplicate);
                }
                Some(OwnedTx::PreAccepted(_)) | Some(OwnedTx::ReplacementHistory(_)) => {
                    return self
                        .plan_single_effect(
                            EffectPolicy::Remote,
                            CommittedEffect::RemoteIngressReleased(
                                CommittedRemoteIngressRelease::unretained_remote_submission(
                                    key, peer,
                                ),
                            ),
                        )
                        .map(RetainedAdmissionDisposition::RemoteReleased);
                }
                None => {}
            },
            RetainedIngressKind::Proposal => match self.entries.get(&key) {
                Some(OwnedTx::Accepted(_)) => {
                    return Ok(RetainedAdmissionDisposition::ProposalUnchanged);
                }
                Some(OwnedTx::PreAccepted(entry))
                    if entry.record.identity.witness == admission.identity.witness
                        && !matches!(entry.source, PreAcceptedSource::Remote(_)) =>
                {
                    return Ok(RetainedAdmissionDisposition::ProposalUnchanged);
                }
                Some(OwnedTx::PreAccepted(entry))
                    if matches!(entry.source, PreAcceptedSource::Recovery(_)) =>
                {
                    return Ok(RetainedAdmissionDisposition::ProposalPayloadVariant);
                }
                Some(OwnedTx::PreAccepted(_)) | Some(OwnedTx::ReplacementHistory(_)) | None => {}
            },
        }

        self.plan_validated_admission_for_foundation(admission)
            .map(RetainedAdmissionDisposition::Retained)
    }

    fn plan_validated_admission_for_foundation(
        &mut self,
        admission: ValidatedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        if matches!(
            admission.source,
            PreAcceptedSource::Recovery(lease) if lease.generation != self.generation
        ) {
            return Err(PlanError::Stale(StalePlan::Generation));
        }
        let admission = self.resources.charge_admission(admission)?;
        self.plan_charged_admission(admission)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum OwnerPhaseSnapshot {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct OwnerSnapshot {
    identity: TxIdentity,
    source: OwnerSourceSnapshot,
    version: EntryVersion,
    arrival: Arrival,
    charge: ChargeRecord,
    phase: OwnerPhaseSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnerSourceSnapshot {
    PreAccepted(PreAcceptedSource),
    Accepted(AcceptedProvenance),
    ReplacementHistory,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct AuthoritySnapshot {
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
    peer_bans: HashMap<ckb_network::PeerIndex, super::super::ban::PeerBanDeadline>,
    clocks: AuthorityClocks,
}

impl AuthoritySnapshot {
    /// Compare one atomic batch with its named no-interleave per-owner
    /// reference under the exact Apply-sequence quotient.
    ///
    /// Entry versions remain exact. Only reference sequence values in
    /// `[batch_sequence, canonical_next_sequence)` collapse onto the one batch
    /// stamp; callers separately prove both next-sequence cursors. Effects are
    /// compared as their public ordered stream, independent of journal batch
    /// envelopes.
    pub(in crate::authority) fn equivalent_modulo_atomic_batch_stamp(
        &self,
        other: &Self,
        batch_sequence: ApplySequence,
        canonical_next_sequence: ApplySequence,
    ) -> bool {
        fn compact(
            sequence: ApplySequence,
            batch: ApplySequence,
            canonical_next: ApplySequence,
        ) -> ApplySequence {
            if sequence >= batch && sequence < canonical_next {
                batch
            } else {
                sequence
            }
        }

        let own_template = self.source_versions.template();
        let other_template = other.source_versions.template();
        let source_versions_equivalent = self.source_versions.accepted()
            == compact(
                other.source_versions.accepted(),
                batch_sequence,
                canonical_next_sequence,
            )
            && self.source_versions.relay_parents()
                == compact(
                    other.source_versions.relay_parents(),
                    batch_sequence,
                    canonical_next_sequence,
                )
            && own_template.proposals
                == compact(
                    other_template.proposals,
                    batch_sequence,
                    canonical_next_sequence,
                )
            && own_template.transactions
                == compact(
                    other_template.transactions,
                    batch_sequence,
                    canonical_next_sequence,
                )
            && own_template.chain
                == compact(
                    other_template.chain,
                    batch_sequence,
                    canonical_next_sequence,
                );
        self.generation == other.generation
            && self.chain_view == other.chain_view
            && self.entries.len() == other.entries.len()
            && self.entries.iter().all(|(hash, owner)| {
                other.entries.get(hash).is_some_and(|other_owner| {
                    owner.equivalent_after_atomic_stamp_compaction(
                        other_owner,
                        batch_sequence,
                        canonical_next_sequence,
                    )
                })
            })
            && self.indexes == other.indexes
            && source_versions_equivalent
            && self.resources == other.resources
            && self.membership == other.membership
            && self.scheduler == other.scheduler
            && self.dependencies.equivalent_after_atomic_stamp_compaction(
                &other.dependencies,
                DependencyCut(batch_sequence),
                DependencyCut(canonical_next_sequence),
            )
            && self.effects.equivalent_stream(&other.effects)
            && self.peer_bans == other.peer_bans
            && self.clocks.next_version == other.clocks.next_version
            && self.clocks.next_arrival == other.clocks.next_arrival
    }
}

impl OwnerSnapshot {
    fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        self.identity == other.identity
            && self.source == other.source
            && self.version == other.version
            && self.arrival == other.arrival
            && self.charge == other.charge
            && match (&self.phase, &other.phase) {
                (
                    OwnerPhaseSnapshot::PreAccepted {
                        phase,
                        dependencies,
                        original_charge,
                    },
                    OwnerPhaseSnapshot::PreAccepted {
                        phase: other_phase,
                        dependencies: other_dependencies,
                        original_charge: other_original_charge,
                    },
                ) => {
                    phase.equivalent_after_atomic_stamp_compaction(
                        other_phase,
                        batch,
                        canonical_next,
                    ) && dependencies == other_dependencies
                        && original_charge == other_original_charge
                }
                (
                    OwnerPhaseSnapshot::Accepted {
                        status,
                        proof,
                        dependencies,
                        accepted_at,
                    },
                    OwnerPhaseSnapshot::Accepted {
                        status: other_status,
                        proof: other_proof,
                        dependencies: other_dependencies,
                        accepted_at: other_accepted_at,
                    },
                ) => {
                    status == other_status
                        && proof.equivalent_after_atomic_stamp_compaction(
                            other_proof,
                            batch,
                            canonical_next,
                        )
                        && dependencies == other_dependencies
                        && accepted_at == other_accepted_at
                }
                (
                    OwnerPhaseSnapshot::ReplacementHistory {
                        dependencies,
                        observation,
                    },
                    OwnerPhaseSnapshot::ReplacementHistory {
                        dependencies: other_dependencies,
                        observation: other_observation,
                    },
                ) => {
                    dependencies == other_dependencies
                        && *observation
                            == compact_dependency_cut(*other_observation, batch, canonical_next)
                }
                (
                    OwnerPhaseSnapshot::PreAccepted { .. }
                    | OwnerPhaseSnapshot::Accepted { .. }
                    | OwnerPhaseSnapshot::ReplacementHistory { .. },
                    OwnerPhaseSnapshot::PreAccepted { .. }
                    | OwnerPhaseSnapshot::Accepted { .. }
                    | OwnerPhaseSnapshot::ReplacementHistory { .. },
                ) => false,
            }
    }
}

fn compact_dependency_cut(
    cut: DependencyCut,
    batch: ApplySequence,
    canonical_next: ApplySequence,
) -> DependencyCut {
    if cut.0 >= batch && cut.0 < canonical_next {
        DependencyCut(batch)
    } else {
        cut
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct DependencyLossWork {
    pub(in crate::authority) output_keys: usize,
    pub(in crate::authority) indexed_origin_keys: usize,
}

impl DependencyLossWork {
    pub(in crate::authority) fn total(self) -> Option<usize> {
        self.output_keys.checked_add(self.indexed_origin_keys)
    }
}

impl ComputeSettlementFailure {
    pub(in crate::authority) fn allocation_for_foundation(settlement: ComputeSettlement) -> Self {
        let ComputeSettlement { token, next } = settlement;
        Self::new(
            PlanError::Backpressure(Backpressure::Allocation),
            token,
            next,
        )
    }
}

impl EffectSettlementFailure {
    pub(in crate::authority) fn into_settlement(self) -> EffectSettlement {
        self.settlement
    }
}

impl CommittedDelta {
    pub(in crate::authority) fn retired_len(&self) -> usize {
        self.retired.len().saturating_add(
            self.retired_generation
                .as_ref()
                .map_or(0, RetiredGeneration::owner_count),
        )
    }

    pub(in crate::authority) fn retired_effect_len(&self) -> usize {
        usize::from(self.retired_effect.is_some())
    }

    pub(in crate::authority) fn async_process_observation_count(&self) -> usize {
        match &self.async_process_observations {
            AsyncProcessObservations::None => 0,
            AsyncProcessObservations::One(_) => 1,
            AsyncProcessObservations::Batch(observations) => observations.len(),
        }
    }
}

impl RetiredGeneration {
    fn owner_count(&self) -> usize {
        self.entries.len()
    }
}

impl PreparedApply<'_> {
    /// Inspect the already-sealed independent Apply order without retaining a
    /// second production receipt after the transition commits.
    pub(in crate::authority) fn independent_order_for_foundation(&self) -> Option<Vec<RawTxHash>> {
        let AuthorityDelta::Independent(delta) = &self.delta else {
            return None;
        };
        Some(
            delta
                .updates
                .iter()
                .map(|update| update.key.clone())
                .collect(),
        )
    }
}

impl PreparedDirectDuplicate<'_> {
    pub(in crate::authority) fn key(&self) -> &RawTxHash {
        &self.key
    }
}

impl PreparedDirectRejection<'_> {
    pub(in crate::authority) fn reason(&self) -> &MembershipReject {
        &self.reason
    }
}

impl PreparedValidationRejection<'_> {
    pub(in crate::authority) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

impl PreparedCandidateRejection<'_> {
    pub(in crate::authority) fn reason(&self) -> &MembershipReject {
        &self.reason
    }
}

impl CommittedCheckout {
    pub(in crate::authority) fn into_work(self) -> CheckedOutWork {
        self.work
    }
}

impl TxPoolAuthority {
    pub(in crate::authority) fn chain_validation_work(
        &self,
        facts: ChainTransitionFacts,
    ) -> Result<super::super::chain::ChainValidationWork, PlanError> {
        self.chain_validation_work_from_view(facts.as_view())
    }

    pub(in crate::authority) fn new(
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

    pub(in crate::authority) fn with_replacement(
        limits: ResourceLimits,
        minimum_rate: ckb_types::core::FeeRate,
    ) -> Self {
        let mut authority = Self::for_foundation(limits);
        authority.membership_config = MembershipConfig::testing_with_replacement(minimum_rate);
        authority
    }

    pub(in crate::authority) fn for_foundation(limits: ResourceLimits) -> Self {
        Self::assemble(
            limits,
            VerifyOrder::Arrival,
            EffectLog::for_foundation(),
            MembershipConfig::testing_default(),
            ChainViewId::initial(),
        )
    }

    pub(in crate::authority) fn for_foundation_with_order(
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

    pub(in crate::authority) fn for_foundation_with_effect_limits(
        limits: ResourceLimits,
        effect_limits: EffectLimits,
    ) -> Result<Self, AuthorityConfigError> {
        Self::new(limits, VerifyOrder::Arrival, effect_limits)
    }

    pub(in crate::authority) fn entries_for_reference(&self) -> &HashMap<RawTxHash, OwnedTx> {
        &self.entries
    }

    pub(in crate::authority) fn scheduler_cursors_for_refinement(
        &self,
    ) -> (
        Option<super::super::scheduler::WorkOwner>,
        Option<super::super::scheduler::WorkOwner>,
    ) {
        self.scheduler.cursors_for_refinement()
    }

    pub(in crate::authority) fn scheduler_worker_wave_for_refinement(
        &self,
        slots: &[super::super::exchange::ComputeWorkerSlot],
    ) -> Result<super::super::scheduler::test_support::SchedulerWaveObservation, PlanError> {
        self.scheduler
            .worker_wave_observation(slots)
            .map_err(PlanError::from)
    }

    pub(in crate::authority) fn owner_count(&self) -> usize {
        self.entries.len()
    }

    pub(in crate::authority) fn charged_count(&self) -> usize {
        self.resources.charge_count()
    }

    pub(in crate::authority) fn resources(&self) -> &ResourceLedger {
        &self.resources
    }

    pub(in crate::authority) fn membership_counts(&self) -> StatusCounts {
        self.membership.counts()
    }

    pub(in crate::authority) fn accepted_parents(
        &self,
        hash: &RawTxHash,
    ) -> Option<&std::collections::HashSet<RawTxHash>> {
        self.membership.parents(hash)
    }

    pub(in crate::authority) fn accepted_children(
        &self,
        hash: &RawTxHash,
    ) -> Option<&std::collections::HashSet<RawTxHash>> {
        self.membership.children(hash)
    }

    pub(in crate::authority) fn generation(&self) -> PoolGeneration {
        self.generation
    }

    pub(in crate::authority) fn chain_view_for_reference(&self) -> &ChainViewId {
        &self.chain_view
    }

    pub(in crate::authority) fn clocks(&self) -> AuthorityClocks {
        self.clocks
    }

    pub(in crate::authority) fn membership_snapshot_for_reference(&self) -> MembershipSnapshot {
        self.membership.snapshot()
    }

    pub(in crate::authority) fn ready_for_reference(&self) -> Vec<(RawTxHash, EntryVersion)> {
        self.ready_candidates()
    }

    pub(in crate::authority) fn preaccepted_for_peer_for_reference(
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

    pub(in crate::authority) fn peer_is_banned_for_reference(
        &self,
        peer: ckb_network::PeerIndex,
    ) -> bool {
        self.peer_bans.contains_at(peer, Instant::now())
    }

    pub(in crate::authority) fn accepted_source_for_reference(&self) -> ApplySequence {
        self.source_versions.accepted()
    }

    pub(in crate::authority) fn template_source_versions_for_reference(
        &self,
    ) -> PoolTemplateVersions {
        self.template_source_versions()
    }

    pub(in crate::authority) fn force_chain_view(&mut self, view: ChainViewId) {
        self.chain_view = view;
    }

    pub(in crate::authority) fn force_next_sequence(&mut self, sequence: ApplySequence) {
        self.clocks.next_sequence = sequence;
    }

    pub(in crate::authority) fn force_next_version(&mut self, version: EntryVersion) {
        self.clocks.next_version = version;
    }

    pub(in crate::authority) fn normalized_snapshot(&self) -> AuthoritySnapshot {
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
            peer_bans: self.peer_bans.snapshot(),
            clocks: self.clocks,
        }
    }

    pub(in crate::authority) fn primary_projection_consistent(&self) -> bool {
        self.entries.len() == self.resources.charge_count()
            && self.entries.iter().all(|(hash, owner)| {
                self.resources.charge(hash) == Some(owner.charge_record())
                    && &owner.record().identity.raw == hash
            })
            && self.indexes.semantically_matches(&self.entries)
            && self.resources.semantically_matches(&self.entries)
            && self.scheduler.semantically_matches(&self.entries)
            && self.dependencies.semantically_matches(&self.entries)
            && self.peer_bans.semantically_consistent()
            && self
                .effects
                .semantically_consistent(self.clocks.next_sequence)
    }

    pub(in crate::authority) fn independent_candidate_for_foundation(
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

    pub(in crate::authority) fn dependency_loss_work_for_foundation(
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

    pub(in crate::authority) fn plan_admission(
        &mut self,
        admission: ValidatedAdmission,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_validated_admission_for_foundation(admission)
    }

    pub(in crate::authority) fn plan_accept_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        self.plan_accept_for_foundation_receipt(receipt)
    }

    pub(in crate::authority) fn plan_accept_at_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
        accepted_at: AcceptedAtMillis,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_at_for_foundation(status, ScriptVerificationRules::V0, accepted_at)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        self.plan_accept_for_foundation_receipt(receipt)
    }

    pub(in crate::authority) fn plan_candidate_disposition_for_foundation(
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

    pub(in crate::authority) fn plan_accept_context_sensitive_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_context_sensitive_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        self.plan_accept_for_foundation_receipt(receipt)
    }

    pub(in crate::authority) fn plan_direct_admission_for_foundation(
        &mut self,
        tx: Arc<TransactionView>,
        verified: super::super::state::VerifiedFacts,
        status: AcceptedStatus,
    ) -> Result<DirectAdmissionDisposition<'_>, PlanError> {
        let work = DirectAdmissionWork::new(tx, verified).map_err(|error| match error {
            DirectAdmissionError::TransactionIdentityMismatch => {
                PlanError::Fault(AuthorityFault::MembershipProjection)
            }
        })?;
        let receipt = work
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(Self::direct_admission_evidence_error_for_foundation)?;
        self.plan_direct_admission(receipt)
    }

    fn direct_admission_evidence_error_for_foundation(error: AdmissionEvidenceError) -> PlanError {
        match error {
            AdmissionEvidenceError::ScriptRulesChanged => {
                PlanError::Stale(StalePlan::ChainRevision)
            }
        }
    }

    fn plan_accept_for_foundation_receipt(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.prepare_accept_delta(receipt)?;
        Ok(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Membership(delta),
        })
    }

    pub(in crate::authority) fn plan_status_for_foundation(
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

        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let mut after = before.clone();
        after.record.version = version;
        after.proposal = super::super::chain::ProposalContextReceipt::from_validation(status);
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
                retired: Vec::new(),
                clocks: clocks.finish(),
                async_process_start: None,
            }),
        })
    }

    pub(in crate::authority) fn plan_terminalize_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
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
        let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        let publication = self
            .effects
            .build_publication(
                policy,
                vec![CommittedEffect::Rejected(CommittedRejection::Validation {
                    tx: Arc::clone(&preaccepted.record.tx),
                    audience: RejectionAudience::from_source(preaccepted.source),
                    reason: CommittedPublicReject::new(Reject::Invalidated(
                        "foundation terminalization".to_owned(),
                    )),
                })],
            )
            .map_err(|_| PlanError::Fault(AuthorityFault::EffectProjection))?;
        self.plan_preaccepted_terminalization(key, expected, &publication)
    }

    pub(in crate::authority) fn plan_terminalize_with_effect_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_preaccepted_terminalization(key, expected, publication)
    }

    pub(in crate::authority) fn effect_publication_for_foundation(
        &self,
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
    ) -> Result<EffectPublication, EffectBuildError> {
        self.effects.build_publication(policy, effects)
    }

    pub(in crate::authority) fn plan_effect_publication_for_foundation(
        &mut self,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let effect = self.effects.plan_publication(publication, sequence)?;
        Ok(self.prepared_effect_only(effect, clocks))
    }

    pub(in crate::authority) fn plan_generation_reset_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let effect = self.effects.plan_generation_reset(sequence)?;
        Ok(self.prepared_effect_only(effect, clocks))
    }

    pub(in crate::authority) fn plan_peer_revocation_for_foundation(
        &mut self,
        peer: ckb_network::PeerIndex,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let mut hashes = Vec::new();
        if let Some(indexed) = self.indexes.preaccepted_for_peer(peer) {
            hashes
                .try_reserve(indexed.len())
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            hashes.extend(indexed.iter().cloned());
        }
        hashes.sort_unstable();
        let marker = self.peer_bans.plan_record(peer, Instant::now())?;
        let revocation =
            CommittedPeerCohortRevocation::administrative_for_foundation(marker.lease());
        self.plan_administrative_removal(hashes, AdminPlan::PeerRevocation { marker, revocation })
    }

    pub(in crate::authority) fn set_peer_ban_limit_for_foundation(&mut self, capacity: usize) {
        self.peer_bans = PeerBanRegistry::with_limit_for_test(capacity);
    }

    pub(in crate::authority) fn effect_publication_receipt_for_foundation(
        &self,
    ) -> Option<EffectReceipt> {
        self.effect_publication_receipt()
    }

    pub(in crate::authority) fn apply_effect_settlement_for_foundation(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<CommittedDelta, EffectSettlementFailure> {
        self.apply_effect_settlement(settlement)
            .map(|commit| match commit {
                EffectSettlementCommit::Applied(delta) => delta,
                EffectSettlementCommit::Superseded(_) => {
                    panic!("the exact foundation settlement was unexpectedly superseded")
                }
            })
    }

    pub(in crate::authority) fn effect_settlement_for_foundation(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<EffectSettlementCommit, EffectSettlementFailure> {
        self.apply_effect_settlement(settlement)
    }

    pub(in crate::authority) fn plan_effect_close_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, EffectCloseError> {
        self.plan_effect_close()
    }

    pub(in crate::authority) fn effects_closed_and_drained_for_foundation(&self) -> bool {
        self.effects_closed_and_drained()
    }

    pub(in crate::authority) fn effect_observation_for_foundation(&self) -> EffectObservation {
        self.effects.observation()
    }

    pub(in crate::authority) fn effect_trace_for_reference(&self) -> Vec<EffectTraceBatch> {
        self.effects.trace_batches()
    }

    pub(in crate::authority) fn plan_dependency_availability_for_foundation(
        &mut self,
        keys: Vec<DependencyKey>,
    ) -> Result<Option<PreparedApply<'_>>, PlanError> {
        self.effects.ensure_open()?;
        let sequence = self.clocks.next_sequence;
        let Some(control) =
            self.dependencies
                .plan_events(keys, Vec::new(), DependencyCut(sequence))?
        else {
            return Ok(None);
        };
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        Ok(Some(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Dependency(DependencyOnlyDelta {
                control,
                clocks: clocks.finish(),
            }),
        }))
    }

    pub(in crate::authority) fn dependency_maintenance_observation_for_foundation(
        &self,
    ) -> Result<Option<(DependencyKey, Option<RawTxHash>)>, PlanError> {
        Ok(self.dependencies.next_maintenance_observation()?)
    }

    pub(in crate::authority) fn dependency_maintenance_rank_for_foundation(
        &self,
    ) -> Result<DependencyMaintenanceRank, DependencyMaintenanceRankError> {
        self.dependencies.maintenance_rank()
    }

    /// Drain one stable dependency epoch using its mechanically derived ghost
    /// rank. Every Apply must be a sealed successor and strictly decrease the
    /// rank; owner requeue may safely discharge more than one waiter
    /// obligation from current and pending epochs.
    pub(in crate::authority) fn drain_dependency_maintenance_for_foundation(
        &mut self,
    ) -> Result<Vec<DependencyMaintenanceObservation>, DependencyMaintenanceDrainError> {
        let mut before_rank = self.dependencies.maintenance_rank()?;
        let mut observations = Vec::new();
        observations
            .try_reserve(before_rank.value())
            .map_err(|_| DependencyMaintenanceDrainError::Allocation)?;
        while before_rank.value() != 0 {
            let (key, hash) = self
                .dependencies
                .next_maintenance_observation()
                .map_err(PlanError::from)?
                .ok_or(DependencyMaintenanceDrainError::MissingObservation)?;
            let observed_owner = hash.as_ref().and_then(|hash| {
                self.entries
                    .get(hash)
                    .map(|owner| (hash.clone(), owner.record().version))
            });
            let plan = self.plan_dependency_maintenance()?.ok_or(
                DependencyMaintenanceDrainError::MissingSuccessor(before_rank),
            )?;
            drop(plan.apply());
            let after_rank = self.dependencies.maintenance_rank()?;
            if !before_rank.strictly_decreases_to(after_rank) {
                return Err(DependencyMaintenanceDrainError::Nondecreasing {
                    before: before_rank,
                    after: after_rank,
                });
            }
            let owner_requeued = observed_owner.is_some_and(|(hash, version)| {
                matches!(
                    self.entries.get(&hash),
                    Some(OwnedTx::PreAccepted(entry))
                        if entry.record.version != version
                            && matches!(entry.phase, PreAcceptedPhase::Queued(QueuedWork::Resolve))
                )
            });
            observations.push(DependencyMaintenanceObservation {
                key,
                hash,
                owner_requeued,
                before_rank,
                after_rank,
            });
            before_rank = after_rank;
        }
        if self.plan_dependency_maintenance()?.is_some() {
            return Err(DependencyMaintenanceDrainError::ResidualSuccessor);
        }
        Ok(observations)
    }

    /// Canonical one-member reference transition used by scheduler and trace
    /// refinement. Production uses `apply_compute_exchange`; this test-only
    /// oracle deliberately keeps the old no-interleave serialization visible.
    pub(in crate::authority) fn plan_checkout_next(
        &mut self,
        permit: super::super::state::WorkPermit,
    ) -> Result<Option<PreparedCheckout<'_>>, PlanError> {
        let search = self.search_checkout(permit)?;
        self.prepare_checkout_search(search, permit)
    }

    fn search_checkout(
        &mut self,
        permit: super::super::state::WorkPermit,
    ) -> Result<CheckoutSearch, PlanError> {
        match self
            .resources
            .active_work_availability_for_reference(ComputeAttribution::Trusted)?
        {
            ActiveWorkAvailability::Available => {}
            ActiveWorkAvailability::PreAcceptedExhausted => {
                return Ok(CheckoutSearch {
                    selected: None,
                    probes: 0,
                });
            }
            ActiveWorkAvailability::RemoteExhausted | ActiveWorkAvailability::PeerExhausted(_) => {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
        }
        let owner_count = self.scheduler.owner_count_for_reference(permit);
        let mut wave = self.scheduler.checkout_wave(1)?;
        let mut cursor = None;
        let mut selected = None;
        let mut probes = 0usize;
        for _ in 0..owner_count {
            let ticket = match cursor {
                Some(owner) => self
                    .scheduler
                    .next_queued_after_in_wave_for_reference(&wave, permit, owner),
                None => self
                    .scheduler
                    .next_queued_in_wave_for_reference(&wave, permit),
            };
            let Some(ticket) = ticket else {
                break;
            };
            probes = probes
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            cursor = Some(ticket.owner());
            match self.plan_checkout_resources(ticket.hash(), ticket.version(), permit)? {
                Ok(reservation) => {
                    wave.select(&ticket)?;
                    selected = Some((ticket, reservation));
                    break;
                }
                Err(CheckoutUnavailable::SkipOwner) => {}
                Err(CheckoutUnavailable::Stop) => break,
            }
        }
        Ok(CheckoutSearch { selected, probes })
    }

    fn prepare_checkout_search(
        &mut self,
        search: CheckoutSearch,
        permit: super::super::state::WorkPermit,
    ) -> Result<Option<PreparedCheckout<'_>>, PlanError> {
        let Some((ticket, reservation)) = search.selected else {
            return Ok(None);
        };
        let key = ticket.hash().clone();
        let version = ticket.version();
        self.plan_selected_checkout(&key, version, permit, ticket, reservation)
            .map(Some)
    }

    fn plan_checkout_resources(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::super::state::WorkPermit,
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
        match self
            .resources
            .active_work_availability_for_reference(attribution)?
        {
            ActiveWorkAvailability::Available => {}
            ActiveWorkAvailability::PeerExhausted(_) => {
                return Ok(Err(CheckoutUnavailable::SkipOwner));
            }
            ActiveWorkAvailability::PreAcceptedExhausted
            | ActiveWorkAvailability::RemoteExhausted => {
                return Ok(Err(CheckoutUnavailable::Stop));
            }
        }
        let (grant, after_charge) = match self.checkout_eligibility(preaccepted, permit)? {
            CheckoutEligibility::Ready {
                grant,
                after_charge,
            } => (grant, after_charge),
            CheckoutEligibility::StaleDependency => {
                return Ok(Err(CheckoutUnavailable::SkipOwner));
            }
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
        Ok(Ok(CheckoutReservation {
            resources,
            grant,
            after_charge,
        }))
    }

    fn plan_selected_checkout(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::super::state::WorkPermit,
        ticket: CheckoutTicket,
        reservation: CheckoutReservation,
    ) -> Result<PreparedCheckout<'_>, PlanError> {
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

        let CheckoutReservation {
            resources,
            grant,
            after_charge,
        } = reservation;
        let clocks = ApplyClockReservation::begin(self.clocks)?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let dependency_cut = match queued {
            QueuedWork::Resolve => DependencyCut(sequence),
            QueuedWork::Verify(resolved) => resolved.dependency_cut(),
        };
        let token = LeaseToken {
            settlement: SettlementToken {
                hash: key.clone(),
                version,
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
        let after = existing
            .with_preaccepted_phase(
                PreAcceptedPhase::Computing(ActiveWork {
                    chain_view: self.chain_view.clone(),
                    permit,
                    grant,
                    attribution: preaccepted.source.compute_attribution(),
                    payload_policy: preaccepted.source.payload_policy(),
                    dependency_cut,
                    dependencies: preaccepted.dependencies().clone(),
                }),
                version,
                after_charge,
            )
            .map_err(PlanError::Stale)?;
        let scheduler = self
            .scheduler
            .plan_replace(Some(&existing), Some(&after), Some(ticket))?;
        let dependency = self
            .dependencies
            .plan_replace(Some(&existing), Some(&after))?;
        let sources = self
            .source_versions
            .plan_replacements(std::iter::once((Some(&existing), Some(&after))), sequence);
        let indexes = self
            .indexes
            .plan_replace(key, Some(&existing), Some(&after))?;
        let plan = PreparedApply {
            authority: self,
            delta: AuthorityDelta::Entry(EntryDelta {
                key: key.clone(),
                after: Some(after),
                owners: DerivedOwnerDelta { indexes, sources },
                retirement: EntryRetirement::InlineDrop,
                resource: resources,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                clocks: clocks.finish(),
            }),
        };
        Ok(PreparedCheckout { plan, work })
    }

    pub(in crate::authority) fn plan_checkout_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::super::state::WorkPermit,
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
        match self
            .resources
            .active_work_availability_for_reference(preaccepted.source.compute_attribution())?
        {
            ActiveWorkAvailability::Available => {}
            ActiveWorkAvailability::PreAcceptedExhausted => {
                return Err(PlanError::Backpressure(Backpressure::TotalResources));
            }
            ActiveWorkAvailability::RemoteExhausted => {
                return Err(PlanError::Backpressure(Backpressure::RemoteResources));
            }
            ActiveWorkAvailability::PeerExhausted(_) => {
                return Err(PlanError::Backpressure(Backpressure::PeerResources));
            }
        }
        let ticket = self
            .scheduler
            .ticket_for_foundation(key, expected, permit)
            .ok_or(PlanError::Stale(StalePlan::Phase))?;
        let reservation = match self.plan_checkout_resources(key, expected, permit)? {
            Ok(reservation) => reservation,
            Err(CheckoutUnavailable::SkipOwner) => {
                return Err(PlanError::Stale(StalePlan::Dependency));
            }
            Err(CheckoutUnavailable::Stop) => {
                return Err(PlanError::Backpressure(Backpressure::ComputeResources));
            }
        };
        self.plan_selected_checkout(key, expected, permit, ticket, reservation)
    }

    pub(in crate::authority) fn plan_checkout_next_with_probe_count_for_foundation(
        &mut self,
        permit: super::super::state::WorkPermit,
    ) -> Result<(Option<PreparedCheckout<'_>>, usize), PlanError> {
        let search = self.search_checkout(permit)?;
        let probes = search.probes;
        let plan = self.prepare_checkout_search(search, permit)?;
        Ok((plan, probes))
    }
}
