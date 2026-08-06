use super::super::{
    chain::{
        AcceptedProof, DirectAdmissionError, DirectAdmissionWork,
        test_support::{AdmissionEvidenceError, ChainTransitionFacts},
    },
    dependency::test_support::DependencySnapshot,
    effect::test_support::{EffectObservation, EffectSnapshot},
    indexes::test_support::IndexSnapshot,
    resources::test_support::ResourceSnapshot,
    scheduler::test_support::SchedulerSnapshot,
    state::{AcceptedStatus, TxIdentity},
};
use super::*;
use ckb_types::core::TransactionView;
use ckb_verification::cache::ScriptVerificationRules;

pub(in crate::authority) use super::super::rejection::ComponentLimitKind;
pub(in crate::authority) use super::membership::StatusCounts;
pub(in crate::authority) use super::membership::test_support::MembershipSnapshot;
pub(in crate::authority) use super::settlement::test_support::CandidateBatchError;

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
    pub(in crate::authority) fn equivalent_modulo_effect_batching(&self, other: &Self) -> bool {
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
            && self.peer_bans == other.peer_bans
            && self.clocks == other.clocks
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
        self.plan_validated_admission(admission)
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

        let version = self.clocks.next_version;
        let sequence = self.clocks.next_sequence;
        let clocks = AuthorityClocks {
            next_version: next_version(version)?,
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
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
                clocks,
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
        let sequence = self.clocks.next_sequence;
        let effect = self.effects.plan_publication(publication, sequence)?;
        self.prepare_effect_only(effect, sequence)
    }

    pub(in crate::authority) fn plan_generation_reset_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let sequence = self.clocks.next_sequence;
        let effect = self.effects.plan_generation_reset(sequence)?;
        self.prepare_effect_only(effect, sequence)
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
        let clocks = AuthorityClocks {
            next_sequence: next_sequence(sequence)?,
            ..self.clocks
        };
        Ok(Some(PreparedApply {
            authority: self,
            delta: AuthorityDelta::Dependency(DependencyOnlyDelta { control, clocks }),
        }))
    }

    pub(in crate::authority) fn dependency_maintenance_observation_for_foundation(
        &self,
    ) -> Result<Option<(DependencyKey, Option<RawTxHash>)>, PlanError> {
        Ok(self.dependencies.next_maintenance_observation()?)
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
            .active_work_availability(preaccepted.source.compute_attribution())?
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
            CheckoutResource::Reserved(reservation) => reservation,
            CheckoutResource::SkipOwner => {
                return Err(PlanError::Stale(StalePlan::Dependency));
            }
            CheckoutResource::Stop => {
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
