use super::super::{
    chain::{
        AcceptedProof, DirectAdmissionError, DirectAdmissionWork,
        test_support::{AdmissionEvidenceError, ChainTransitionFacts},
    },
    dependency::test_support::{
        DependencyMaintenanceRank, DependencyMaintenanceRankError, DependencySnapshot,
    },
    effect::EffectReceipt,
    effect::test_support::{
        EffectObservation, EffectPublicationObservationSnapshot, EffectSnapshot, EffectTraceBatch,
    },
    exchange::{
        AuthorityComputeExecutionPermit, ComputeVerifierSlot, ComputeWorkerGrant, ComputeWorkerSlot,
    },
    indexes::test_support::IndexSnapshot,
    resources::{ResourceRead, test_support::ResourceSnapshot},
    scheduler::test_support::SchedulerSnapshot,
    state::{AcceptedStatus, ProposalId, RawTxHash, TxIdentity, WorkPermit},
    work::CheckedOutWork,
};
use super::*;
use crate::authority::ingress::{
    RetainedAdmissionBatch, RetainedIngressAttempt, RetainedIngressBoundaryError,
    RetainedIngressRejection,
};
use ckb_types::core::TransactionView;
use ckb_verification::cache::ScriptVerificationRules;

pub(in crate::authority) struct AuthorityTestToken(());

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

fn retained_ingress_error(error: ConcurrentRetainedIngressError) -> PlanError {
    match error {
        ConcurrentRetainedIngressError::Stale => PlanError::Stale(StalePlan::Version),
        ConcurrentRetainedIngressError::Fault(fault) => PlanError::Fault(fault),
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

pub(in crate::authority) use super::membership::StatusCounts;
pub(in crate::authority) use super::membership::test_support::MembershipSnapshot;

#[must_use = "test checkout work and retirement must leave the oracle guard together"]
pub(in crate::authority) struct CommittedCheckout {
    work: CheckedOutWork,
    retirement: CommittedDelta,
}

impl CommittedCheckout {
    pub(in crate::authority) fn into_parts(self) -> (CheckedOutWork, CommittedDelta) {
        (self.work, self.retirement)
    }
}

impl CommittedRetainedAdmissionBatch {
    pub(in crate::authority) const fn consumed(&self) -> usize {
        match self {
            Self::Unchanged { consumed, .. } | Self::Applied { consumed, .. } => *consumed,
        }
    }
}

impl CompiledSharedRemoteExpiry {
    pub(in crate::authority) fn apply_for_foundation(
        self,
        authority: &TxPoolAuthority,
    ) -> CommittedDelta {
        let shared = self
            .bind(authority)
            .expect("the foundation Remote-expiry generation remains current")
            .apply()
            .unwrap_or_else(|failure| {
                let (error, _effect_wake) = failure.into_parts();
                panic!("the foundation Remote-expiry cut commits: {error:?}")
            });
        let (committed, post_commit_fault) = shared.into_parts();
        assert_eq!(post_commit_fault, None);
        committed
    }
}

impl CompiledSharedAcceptedExpiry {
    pub(in crate::authority) fn lifecycle_is_current_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> (bool, bool) {
        (
            self.generation == authority.generation,
            self.chain_view == authority.chain_view,
        )
    }
}

#[derive(Clone, Copy)]
pub(in crate::authority) struct SharedLocalRemovalSupport {
    owner_writes: super::super::shard::ShardWriteSupport,
    owner_apply: super::super::shard::ShardApplySupport,
    relation_apply: super::super::shard::ShardApplySupport,
    dependency_gates: super::super::shard::DependencyGateSupport,
}

impl SharedLocalRemovalSupport {
    pub(in crate::authority) fn owner_writes(self) -> super::super::shard::ShardWriteSupport {
        self.owner_writes
    }

    pub(in crate::authority) fn owner_apply(self) -> super::super::shard::ShardApplySupport {
        self.owner_apply
    }

    pub(in crate::authority) fn relation_apply(self) -> super::super::shard::ShardApplySupport {
        self.relation_apply
    }

    pub(in crate::authority) fn dependency_gates(
        self,
    ) -> super::super::shard::DependencyGateSupport {
        self.dependency_gates
    }
}

impl PreparedSharedOwnerRemoval<'_, AdministrativeRemovalControl> {
    fn physical_apply_support_for_foundation(
        &self,
    ) -> (
        super::super::shard::ShardApplySupport,
        super::super::shard::ShardApplySupport,
    ) {
        let owner_writes = self.physical_write_support_for_foundation();
        let mut owner_reads = super::super::shard::ShardReadSupport::default();
        let mut projected_owner_writes = super::super::shard::ShardWriteSupport::default();
        self.projections
            .extend_final_support(&mut owner_reads, &mut projected_owner_writes);
        super::apply_seal::SharedOwnerRemovalControl::extend_final_support(
            &self.control,
            &self.authority.entries,
            &mut owner_reads,
            &mut projected_owner_writes,
        );
        debug_assert_eq!(
            projected_owner_writes.mask_for_foundation() & !owner_writes.mask_for_foundation(),
            0,
            "the complete owner write support must contain every projection/control write"
        );
        let mut relation_reads = super::super::shard::ShardReadSupport::default();
        let mut relation_writes = super::super::shard::ShardWriteSupport::default();
        self.projections
            .extend_final_relation_support(&mut relation_reads, &mut relation_writes);
        (
            super::super::shard::ShardApplySupport::new(owner_reads, owner_writes),
            super::super::shard::ShardApplySupport::new(relation_reads, relation_writes),
        )
    }
}

impl TxPoolAuthority {
    pub(in crate::authority) fn shared_local_removal_support_for_foundation(
        &self,
        root: &RawTxHash,
    ) -> Result<Option<SharedLocalRemovalSupport>, PlanError> {
        let Some(compiled) = self.compile_shared_local_removal(root)? else {
            return Ok(None);
        };
        let owner_writes = self
            .entries
            .owner_write_support(compiled.removal.hashes.iter());
        let mut dependency_gates = compiled
            .removal
            .dependency
            .dependency_gate_support(&self.entries);
        dependency_gates.include(
            compiled
                .removal
                .membership
                .dependency_gate_support(&self.entries),
        );
        let prepared = compiled.bind(self)?;
        let (owner_apply, relation_apply) = prepared.physical_apply_support_for_foundation();
        Ok(Some(SharedLocalRemovalSupport {
            owner_writes,
            owner_apply,
            relation_apply,
            dependency_gates,
        }))
    }

    pub(in crate::authority) fn replace_proposal_owner_for_foundation(
        &self,
        proposal: &ProposalId,
        owner: Option<RawTxHash>,
    ) -> Option<RawTxHash> {
        self.indexes
            .replace_proposal_owner_for_test(proposal, owner)
    }

    pub(in crate::authority) fn owner_shard_write_available_for_foundation(
        &self,
        hash: &RawTxHash,
    ) -> bool {
        let support = self.entries.owner_write_support(std::iter::once(hash));
        self.entries.try_write_cut(support).is_some()
    }

    pub(in crate::authority) fn commit_retained_attempt_for_foundation(
        &self,
        attempt: RetainedIngressAttempt,
    ) -> Result<CommittedRetainedAdmissionBatch, PlanError> {
        let malformed = attempt.is_malformed_remote();
        let batch = RetainedAdmissionBatch::new(attempt, std::collections::VecDeque::new())
            .map_err(|error| match error {
                RetainedIngressBoundaryError::ResourceUnavailable => {
                    PlanError::Fault(AuthorityFault::ResourceProjection)
                }
                RetainedIngressBoundaryError::InvalidEvidence => {
                    PlanError::Fault(AuthorityFault::MembershipProjection)
                }
                RetainedIngressBoundaryError::Backpressure(_) => {
                    PlanError::Fault(AuthorityFault::ResourceProjection)
                }
                RetainedIngressBoundaryError::LifecycleClosed => PlanError::EffectClosed,
                RetainedIngressBoundaryError::Fault(fault) => PlanError::Fault(fault),
            })?;
        if malformed {
            return self
                .plan_shared_peer_revocation(&batch)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?
                .apply()
                .map_err(|failure| {
                    let (error, _effect_wake) = failure.into_parts();
                    retained_ingress_error(error)
                });
        }
        match self.classify_shared_retained_ingress_head(&batch)? {
            SharedRetainedIngressHead::Owner => self
                .compile_shared_retained_ingress_batch(&batch)?
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?
                .bind(self)
                .map_err(retained_ingress_error)?
                .apply()
                .map_err(retained_ingress_error),
            SharedRetainedIngressHead::EffectOrNoop => Ok(self
                .plan_shared_retained_effect_prefix(&batch)?
                .ok_or(PlanError::Stale(StalePlan::Version))?
                .apply()),
        }
    }

    pub(in crate::authority) fn commit_retained_rejection_for_foundation(
        &self,
        rejection: RetainedIngressRejection,
    ) -> Result<CommittedRetainedAdmissionBatch, PlanError> {
        self.commit_retained_attempt_for_foundation(RetainedIngressAttempt::Rejected(rejection))
    }

    pub(in crate::authority) fn dependency_consumers_for_foundation(
        &self,
        key: &crate::authority::state::DependencyKey,
    ) -> Option<std::collections::BTreeSet<RawTxHash>> {
        self.dependencies
            .consumers_for(key)
            .expect("the test dependency row remains within its declared bound")
    }

    pub(in crate::authority) fn dependency_unindexed_loss_for_foundation(
        &self,
        key: &crate::authority::state::DependencyKey,
    ) -> Option<DependencyCut> {
        self.dependencies
            .unindexed_definitive_loss_for_reference(key)
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
        observed: Vec<DependencyKey>,
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
    relay_parent_sources: [u64; super::super::shard::AUTHORITY_SHARD_COUNT],
    source_versions: AuthoritySourceVersionSnapshot,
    resources: ResourceSnapshot,
    membership: MembershipSnapshot,
    scheduler: SchedulerSnapshot,
    dependencies: DependencySnapshot,
    effects: EffectSnapshot,
    peer_bans: HashMap<ckb_network::PeerIndex, super::super::ban::PeerBanDeadline>,
    clocks: AuthorityClocks,
}

impl AuthoritySnapshot {
    /// Compare committed authority semantics while requiring the exact named
    /// gaps in the three private identity/order allocators.
    ///
    /// Planning reserves globally unique owner identities and Apply stamps
    /// before a prepared transition can race with another planner. Dropping or
    /// rejecting that prepared transition must not reuse those identities, so
    /// the clock high-water marks may advance even though no owner, projection,
    /// effect, ban, generation, or chain fact commits. This relation keeps that
    /// distinction explicit instead of weakening `PartialEq`: tests which bind
    /// exact allocator consumption must continue to use exact equality.
    pub(in crate::authority) fn equivalent_committed_state_with_exact_reservations(
        &self,
        before: &Self,
        versions: u128,
        arrivals: u128,
        sequences: u128,
    ) -> bool {
        let expected_version = before
            .clocks
            .next_version
            .0
            .checked_add(versions)
            .map(EntryVersion);
        let expected_arrival = before
            .clocks
            .next_arrival
            .0
            .checked_add(arrivals)
            .map(Arrival);
        let expected_sequence = before
            .clocks
            .next_sequence
            .0
            .checked_add(sequences)
            .map(ApplySequence);
        self.generation == before.generation
            && self.chain_view == before.chain_view
            && self.entries == before.entries
            && self.indexes == before.indexes
            && self.relay_parent_sources == before.relay_parent_sources
            && self.source_versions == before.source_versions
            && self.resources == before.resources
            && self.membership == before.membership
            && self.scheduler == before.scheduler
            && self.dependencies == before.dependencies
            && self.effects == before.effects
            && self.peer_bans == before.peer_bans
            && Some(self.clocks.next_version) == expected_version
            && Some(self.clocks.next_arrival) == expected_arrival
            && Some(self.clocks.next_sequence) == expected_sequence
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
}

impl RetiredGeneration {
    fn owner_count(&self) -> usize {
        self.entries.len()
    }
}

impl CompiledSharedIndependent {
    pub(in crate::authority) fn physical_apply_support_for_foundation(
        &self,
    ) -> super::super::shard::ShardApplySupport {
        self.support
    }

    pub(in crate::authority) fn dependency_write_support_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> super::super::shard::ShardWriteSupport {
        self.delta
            .dependency
            .relation_write_support_for_foundation(&authority.entries)
    }

    pub(in crate::authority) fn dependency_primary_insertion_shape_for_foundation(&self) -> bool {
        self.delta
            .dependency
            .primary_accepted_insertion_shape_for_foundation()
    }
}

impl PreparedIndependentApply<'_> {
    pub(in crate::authority) fn dependency_gate_cut_available_for_foundation(&self) -> bool {
        let Self::Shared {
            authority, delta, ..
        } = self;
        authority
            .entries
            .try_dependency_gate_cut(delta.dependency_gate_support_for_foundation(authority))
            .is_some()
    }
}

impl PreparedSharedDirectAdmissionDisposition<'_> {
    pub(in crate::authority) fn dependency_primary_insertion_shape_for_foundation(&self) -> bool {
        match self {
            Self::Accepted { compiled, .. } => {
                compiled.dependency_primary_insertion_shape_for_foundation()
            }
            Self::EffectOnly(_) => false,
        }
    }

    pub(in crate::authority) fn dependency_write_support_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> Option<super::super::shard::ShardWriteSupport> {
        match self {
            Self::Accepted { compiled, .. } => {
                Some(compiled.dependency_write_support_for_foundation(authority))
            }
            Self::EffectOnly(_) => None,
        }
    }

    pub(in crate::authority) fn physical_apply_support_for_foundation(
        &self,
    ) -> Option<super::super::shard::ShardApplySupport> {
        match self {
            Self::Accepted { compiled, .. } => Some(compiled.support),
            Self::EffectOnly(_) => None,
        }
    }

    pub(in crate::authority) fn is_compatible_with_for_foundation(
        &self,
        authority: &TxPoolAuthority,
        other: &Self,
    ) -> bool {
        match (self, other) {
            (
                Self::Accepted { compiled: left, .. },
                Self::Accepted {
                    compiled: right, ..
                },
            ) => {
                left.is_compatible_with(right)
                    && left
                        .delta
                        .dependency_gate_support_for_foundation(authority)
                        .is_compatible(
                            right
                                .delta
                                .dependency_gate_support_for_foundation(authority),
                        )
            }
            (Self::Accepted { .. } | Self::EffectOnly(_), _) => false,
        }
    }
}

impl TxPoolAuthority {
    pub(in crate::authority) fn reserve_ready_exact_for_foundation(
        &self,
        hashes: &[RawTxHash],
    ) -> super::super::scheduler::ReadyReservation {
        super::super::scheduler::ReadyReservation::capture_exact_for_foundation(
            &self.scheduler,
            hashes,
        )
        .expect("foundation Ready identities are current and unreserved")
    }

    pub(in crate::authority) fn ready_reserved_len_for_foundation(&self) -> usize {
        self.scheduler.lock().ready_reserved_len_for_foundation()
    }

    pub(in crate::authority) fn set_staged_rollback_terminal_probe_for_foundation(
        &self,
        probe: Option<std::sync::Arc<super::super::shard::ConcurrentRemovalProbe>>,
    ) {
        self.effects
            .lock()
            .set_staged_rollback_terminal_probe_for_foundation(probe);
    }

    pub(in crate::authority) fn ready_physical_counts_for_foundation(
        &self,
    ) -> (usize, usize, usize) {
        self.scheduler.lock().ready_physical_counts_for_foundation()
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
        let effects = EffectLog::new(effect_limits).map_err(AuthorityConfigError::Effect)?;
        Ok(Self::from_test(
            &AuthorityTestToken(()),
            limits,
            verify_order,
            effects,
            MembershipConfig::testing_default(),
            ChainViewId::initial(),
        ))
    }

    pub(in crate::authority) fn with_replacement(
        limits: ResourceLimits,
        minimum_rate: ckb_types::core::FeeRate,
    ) -> Self {
        let mut authority = Self::for_foundation(limits);
        authority.replace_membership_config_for_test(
            &AuthorityTestToken(()),
            MembershipConfig::testing_with_replacement(minimum_rate),
        );
        authority
    }

    pub(in crate::authority) fn for_foundation(limits: ResourceLimits) -> Self {
        Self::from_test(
            &AuthorityTestToken(()),
            limits,
            VerifyOrder::Arrival,
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

    pub(in crate::authority) fn entries_for_reference(
        &self,
    ) -> &crate::authority::shard::ShardedOwnerMap {
        &self.entries
    }

    pub(in crate::authority) fn owner_count(&self) -> usize {
        self.entries.len()
    }

    pub(in crate::authority) fn charged_count(&self) -> usize {
        self.entries.len()
    }

    pub(in crate::authority) fn resources(&self) -> ResourceRead<'_> {
        self.resources.read(&self.entries)
    }

    pub(in crate::authority) fn membership_counts(&self) -> StatusCounts {
        self.entries
            .status_counts()
            .expect("foundation owner count is bounded by configured resources")
    }

    pub(in crate::authority) fn accepted_parents(
        &self,
        hash: &RawTxHash,
    ) -> Option<std::collections::HashSet<RawTxHash>> {
        self.membership.parents(hash)
    }

    pub(in crate::authority) fn accepted_children(
        &self,
        hash: &RawTxHash,
    ) -> Option<std::collections::HashSet<RawTxHash>> {
        self.membership.children(hash)
    }

    pub(in crate::authority) fn generation(&self) -> PoolGeneration {
        self.generation
    }

    pub(in crate::authority) fn clocks(&self) -> AuthorityClocks {
        self.clocks.snapshot()
    }

    pub(in crate::authority) fn ready_for_reference(&self) -> Vec<(RawTxHash, EntryVersion)> {
        self.ready_candidates()
            .expect("foundation Ready scratch is available")
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
        self.entries
            .peer_is_banned_at(peer, Instant::now())
            .unwrap_or(false)
    }

    pub(in crate::authority) fn peer_fence_hidden_for_reference(
        &self,
        peer: ckb_network::PeerIndex,
    ) -> bool {
        self.entries
            .peer_ingress_row(peer)
            .is_some_and(|row| row.has_hidden_fence())
    }

    pub(in crate::authority) fn peer_ingress_row_count_for_reference(&self) -> usize {
        self.entries.peer_ingress_row_count_for_test()
    }

    pub(in crate::authority) fn force_chain_view(&mut self, view: ChainViewId) {
        self.replace_chain_view_for_test(&AuthorityTestToken(()), view);
    }

    pub(in crate::authority) fn force_next_sequence(&mut self, sequence: ApplySequence) {
        self.replace_next_sequence_for_test(&AuthorityTestToken(()), sequence);
    }

    pub(in crate::authority) fn normalized_snapshot(&self) -> AuthoritySnapshot {
        let owner_snapshot = self.entries.snapshot_for_test();
        let entries = owner_snapshot
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
                        observed: entry.observation().keys().cloned().collect(),
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
            relay_parent_sources: self.entries.relay_parent_sources(),
            source_versions: self
                .source_versions
                .snapshot_with_template(self.template_source_versions()),
            resources: self.resources().snapshot(),
            membership: self.membership.snapshot(self.membership_counts()),
            scheduler: self.scheduler.lock().snapshot(),
            dependencies: self.dependencies.snapshot(),
            effects: self.effects.lock().snapshot(),
            peer_bans: self.indexes.peer_ban_snapshot(),
            clocks: self.clocks.snapshot(),
        }
    }

    pub(in crate::authority) fn primary_projection_consistent(&self) -> bool {
        self.primary_projection_inconsistencies().is_empty()
    }

    pub(in crate::authority) fn primary_projection_inconsistencies(&self) -> Vec<&'static str> {
        let mut failures = Vec::new();
        let owner_snapshot = self.entries.snapshot_for_test();
        if !owner_snapshot
            .iter()
            .all(|(hash, owner)| &owner.record().identity.raw == hash)
        {
            failures.push("owner_identity");
        }
        if !self.indexes.semantically_matches(&self.entries) {
            failures.push("indexes");
        }
        if !self.resources().semantically_matches() {
            failures.push("resources");
        }
        if !self.membership_projection_consistent() {
            failures.push("membership");
        }
        if !self
            .scheduler
            .lock()
            .semantically_matches_snapshot(&owner_snapshot)
        {
            failures.push("scheduler");
        }
        if !self.dependencies.semantically_matches(&self.entries) {
            failures.push("dependencies");
        }
        if !self.peer_bans.semantically_consistent()
            || self.peer_bans.snapshot() != self.indexes.peer_ban_snapshot()
        {
            failures.push("peer_bans");
        }
        if !self
            .effects
            .lock()
            .semantically_consistent(self.clocks.snapshot().next_sequence)
        {
            failures.push("effects");
        }
        failures
    }

    pub(in crate::authority) fn membership_projection_consistent(&self) -> bool {
        self.membership.semantically_matches(&self.entries)
    }

    pub(in crate::authority) fn independent_candidate_for_foundation(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<FinalAdmissionReceipt, PlanError> {
        let receipt = self
            .final_admission_work(key, expected)?
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))?;
        Ok(receipt)
    }

    pub(in crate::authority) fn plan_admission(
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

    pub(in crate::authority) fn prepare_production_shared_direct_admission_for_foundation(
        &self,
        tx: Arc<TransactionView>,
        verified: super::super::state::VerifiedFacts,
        status: AcceptedStatus,
    ) -> Result<PreparedSharedDirectAdmissionDisposition<'_>, PlanError> {
        let work = DirectAdmissionWork::new(tx, verified).map_err(|error| match error {
            DirectAdmissionError::TransactionIdentityMismatch => {
                PlanError::Fault(AuthorityFault::MembershipProjection)
            }
        })?;
        let receipt = work
            .validate_for_foundation(status, ScriptVerificationRules::V0)
            .map_err(Self::direct_admission_evidence_error_for_foundation)?;
        self.prepare_shared_direct_admission(receipt)
    }

    fn direct_admission_evidence_error_for_foundation(error: AdmissionEvidenceError) -> PlanError {
        match error {
            AdmissionEvidenceError::ScriptRulesChanged => {
                PlanError::Stale(StalePlan::ChainRevision)
            }
        }
    }

    pub(in crate::authority) fn plan_accept_for_foundation_receipt(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let delta = self.prepare_accept_delta(receipt)?;
        PreparedApply::prepare(self, DependencyAuthorityDelta::Membership(delta))
    }

    pub(in crate::authority) fn plan_status_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.lock().ensure_open()?;
        let existing = self
            .entries
            .get(key)
            .as_deref()
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

        let mut clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let version = clocks.replacement()?;
        let mut after = before.clone();
        after.record.version = version;
        after.proposal = super::super::chain::ProposalContextReceipt::from_internal_status(status);
        let projection = self.prepare_status_change(key, before, &after)?;
        let after = OwnedTx::Accepted(after);
        let resource_changes = vec![(
            key.clone(),
            Some(existing.charge_record()),
            Some(after.charge_record()),
        )];
        let resource = self
            .resources_for_test_plan()
            .plan_batch(resource_changes)?;
        let scheduler = self
            .scheduler
            .lock()
            .plan_replace(Some(&existing), Some(&after), None)?;
        let owners =
            self.plan_membership_owner_derivations((key, Some(&existing), &after), &[], sequence)?;
        let dependency =
            self.plan_membership_dependency_delta(Some(&existing), &after, &[], sequence)?;
        PreparedApply::prepare(
            self,
            DependencyAuthorityDelta::Membership(MembershipDelta {
                changed_key: key.clone(),
                changed_expected: OwnerPrestate::from_owner(&existing),
                changed_after: after,
                retired: RetiredOwners::default(),
                removals: Vec::new(),
                owners,
                resource,
                projection,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                async_process_start: None,
            }),
        )
    }

    pub(in crate::authority) fn plan_terminalize_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
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
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let policy = EffectPolicy::for_preaccepted_source(preaccepted.source);
        let publication = self
            .effects
            .lock()
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
        self.effects.lock().build_publication(policy, effects)
    }

    pub(in crate::authority) fn plan_effect_publication_for_foundation(
        &mut self,
        publication: &EffectPublication,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.effects.lock().preflight_publication(publication)?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self
            .effects_for_test_plan()
            .plan_publication(publication, sequence)?;
        Ok(self.prepared_effect_only(effect))
    }

    pub(in crate::authority) fn classify_overtaken_effect_settlement_for_foundation(
        &self,
        settlement: ComputeSettlement,
    ) -> ComputeSettlementFailure {
        self.compute_settlement_failure(PlanError::Stale(StalePlan::EffectSequence), settlement)
    }

    pub(in crate::authority) fn plan_generation_reset_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self.effects.lock().plan_generation_reset(sequence)?;
        Ok(self.prepared_effect_only(effect))
    }

    pub(in crate::authority) fn plan_peer_revocation_for_foundation(
        &mut self,
        peer: ckb_network::PeerIndex,
    ) -> Result<PreparedApply<'_>, PlanError> {
        self.plan_peer_revocation_at_for_foundation(peer, Instant::now())
    }

    pub(in crate::authority) fn prepare_shared_local_removal_for_foundation(
        &self,
        root: &RawTxHash,
    ) -> Result<Option<PreparedSharedOwnerRemoval<'_, AdministrativeRemovalControl>>, PlanError>
    {
        let Some(compiled) = self.compile_shared_local_removal(root)? else {
            return Ok(None);
        };
        compiled.bind(self).map(Some)
    }

    pub(in crate::authority) fn plan_peer_revocation_at_for_foundation(
        &mut self,
        peer: ckb_network::PeerIndex,
        observed_at: Instant,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let mut hashes = Vec::new();
        if let Some(indexed) = self.indexes.preaccepted_for_peer(peer) {
            hashes.reserve(indexed.len());
            hashes.extend(indexed.iter().cloned());
        }
        hashes.sort_unstable();
        let marker = self
            .peer_bans_for_test_plan()
            .plan_record(peer, observed_at)?;
        let revocation =
            CommittedPeerCohortRevocation::administrative_for_foundation(marker.lease());
        let delta = self.compile_administrative_removal(hashes, marker, revocation)?;
        PreparedApply::prepare(self, DependencyAuthorityDelta::Admin(delta))
    }

    pub(in crate::authority) fn set_peer_ban_limit_for_foundation(&mut self, capacity: usize) {
        self.replace_peer_bans_for_test(&AuthorityTestToken(()), capacity);
    }

    pub(in crate::authority) fn effect_publication_receipt_for_foundation(
        &self,
    ) -> Option<EffectReceipt> {
        match self.effect_publication_observation() {
            EffectPublicationObservation::Receipt(receipt) => Some(receipt),
            EffectPublicationObservation::Idle | EffectPublicationObservation::ClosedAndDrained => {
                None
            }
        }
    }

    pub(in crate::authority) fn set_next_effect_activation_probe_for_foundation(
        &self,
        probe: Option<std::sync::Arc<super::super::shard::ConcurrentRemovalProbe>>,
    ) {
        EffectLog::set_next_staged_activation_probe(&self.effects, probe);
    }

    pub(in crate::authority) fn effect_publication_observation_for_foundation(
        &self,
    ) -> EffectPublicationObservationSnapshot {
        self.effect_publication_observation().snapshot()
    }

    pub(in crate::authority) fn apply_effect_settlement_for_foundation(
        &mut self,
        receipt: EffectReceipt,
    ) -> Result<CommittedDelta, EffectSettlementFailure> {
        self.apply_effect_settlement(receipt)
            .map(|(applied, _next)| applied.into_committed_for_foundation())
    }

    pub(in crate::authority) fn effect_settlement_for_foundation(
        &mut self,
        receipt: EffectReceipt,
    ) -> Result<EffectSettlementApplied, EffectSettlementFailure> {
        self.apply_effect_settlement(receipt)
            .map(|(applied, _next)| applied)
    }

    pub(in crate::authority) fn close_effects_for_foundation(
        &mut self,
    ) -> Result<(), EffectCloseError> {
        let _wake = self.close_effects()?;
        Ok(())
    }

    pub(in crate::authority) fn effect_observation_for_foundation(&self) -> EffectObservation {
        self.effects.lock().observation()
    }

    pub(in crate::authority) fn effect_trace_for_reference(&self) -> Vec<EffectTraceBatch> {
        self.effects.lock().trace_batches()
    }

    pub(in crate::authority) fn plan_dependency_availability_for_foundation(
        &self,
        keys: Vec<DependencyKey>,
    ) -> Result<Option<PreparedIndependentApply<'_>>, PlanError> {
        self.effects.lock().ensure_open()?;
        let sequence = self.clocks.snapshot().next_sequence;
        let Some(control) =
            self.dependencies
                .plan_events(keys, Vec::new(), DependencyCut(sequence))?
        else {
            return Ok(None);
        };
        let _clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        self.prepare_shared_dependency_control_for_foundation(control)
            .map(Some)
    }

    pub(in crate::authority) fn plan_dependency_loss_for_foundation(
        &self,
        keys: Vec<DependencyKey>,
    ) -> Result<Option<PreparedIndependentApply<'_>>, PlanError> {
        self.effects.lock().ensure_open()?;
        let sequence = self.clocks.snapshot().next_sequence;
        let Some(control) =
            self.dependencies
                .plan_events(Vec::new(), keys, DependencyCut(sequence))?
        else {
            return Ok(None);
        };
        let _clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        self.prepare_shared_dependency_control_for_foundation(control)
            .map(Some)
    }

    fn prepare_shared_dependency_control_for_foundation(
        &self,
        control: DependencyEntryControlDelta,
    ) -> Result<PreparedIndependentApply<'_>, PlanError> {
        let dependency = self
            .dependencies
            .seal_shared_control_for_foundation(control)?;
        let delta = IndependentDelta {
            owner_cuts: Vec::new(),
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
        Ok(PreparedIndependentApply::Shared {
            authority: self,
            delta,
            support,
            staged_effect: None,
        })
    }

    pub(in crate::authority) fn apply_dependency_loss_during_shared_plan_for_foundation(
        &self,
        keys: Vec<DependencyKey>,
    ) -> Result<(), PlanError> {
        let sequence = self.clocks.snapshot().next_sequence;
        if let Some(control) =
            self.dependencies
                .plan_events(Vec::new(), keys, DependencyCut(sequence))?
        {
            self.dependencies
                .apply_control_in_exact_cut_for_reference(control);
        }
        Ok(())
    }

    pub(in crate::authority) fn rebind_owner_version_during_shared_plan_for_foundation(
        &self,
        hash: &RawTxHash,
    ) -> EntryVersion {
        let mut owner = self
            .entries
            .get(hash)
            .as_deref()
            .cloned()
            .expect("the rebind fixture owner exists");
        let next = EntryVersion(
            owner
                .record()
                .version
                .0
                .checked_add(1)
                .expect("the fixture owner version is not exhausted"),
        );
        let OwnedTx::PreAccepted(entry) = &mut owner else {
            panic!("the compute rebind fixture remains preaccepted");
        };
        entry.record.version = next;
        let mut entries = self.entries.clone();
        drop(entries.insert(hash.clone(), owner));
        next
    }

    pub(in crate::authority) fn cycle_owner_row_during_shared_plan_for_foundation(
        &self,
        hash: &RawTxHash,
    ) {
        let owner = self
            .entries
            .get(hash)
            .as_deref()
            .cloned()
            .expect("the owner ABA fixture exists");
        let shard = self.entries.owner_shard(hash);
        let mut support = super::super::shard::ShardWriteSupport::default();
        support.insert(shard);
        let mut cut = self.entries.write_cut(support);
        let removed = cut
            .replace(shard, hash.clone(), None)
            .expect("the owner ABA fixture removes its exact row");
        assert_eq!(removed.record().version, owner.record().version);
        assert!(cut.replace(shard, hash.clone(), Some(owner)).is_none());
    }

    pub(in crate::authority) fn hold_positive_compute_reservation_for_foundation(
        &self,
    ) -> Result<super::super::resources::HeldResourceCapacityReservation, ResourceError> {
        self.resources
            .hold_positive_compute_reservation_for_foundation()
    }

    pub(in crate::authority) fn hold_positive_accepted_reservation_for_foundation(
        &self,
    ) -> Result<super::super::resources::HeldResourceCapacityReservation, ResourceError> {
        self.resources
            .hold_positive_accepted_reservation_for_foundation()
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
            let committed = plan
                .apply()
                .expect("fixture dependency maintenance successor remains fresh through Apply");
            drop(committed);
            let after_rank = self.dependencies.maintenance_rank()?;
            if !before_rank.strictly_decreases_to(after_rank) {
                return Err(DependencyMaintenanceDrainError::Nondecreasing {
                    before: before_rank,
                    after: after_rank,
                });
            }
            let owner_requeued = observed_owner.is_some_and(|(hash, version)| {
                matches!(
                    self.entries.get(&hash).as_deref(),
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

    fn foundation_compute_grant(permit: WorkPermit) -> ComputeWorkerGrant {
        let slot = match permit {
            WorkPermit::ResolveOnly => ComputeWorkerSlot::ordered_resolve(),
            WorkPermit::VerifyOnly(capability) | WorkPermit::ResolveThenVerify(capability) => {
                ComputeVerifierSlot::new(0, capability).into()
            }
        };
        let permit = std::sync::Arc::new(tokio::sync::Semaphore::new(1))
            .try_acquire_owned()
            .expect("the fixture owns its execution permit");
        ComputeWorkerGrant::new(
            slot,
            AuthorityComputeExecutionPermit::new(
                permit,
                std::sync::Arc::new(tokio::sync::Notify::new()),
            ),
        )
    }

    fn checkout_one_for_foundation(
        &self,
        permit: WorkPermit,
    ) -> Result<Option<CommittedCheckout>, PlanError> {
        let committed = self
            .apply_compute_exchange(vec![Self::foundation_compute_grant(permit)], &[])
            .map_err(Self::foundation_exchange_error)?;
        Ok(Self::committed_checkout_for_foundation(committed))
    }

    fn foundation_exchange_error(failure: ComputeExchangePlanFailure) -> PlanError {
        let (error, grants) = failure.into_parts();
        drop(grants);
        error
    }

    fn committed_checkout_for_foundation(
        committed: CommittedComputeExchange,
    ) -> Option<CommittedCheckout> {
        let CommittedComputeExchange {
            retirement,
            mut assignments,
            unused_grants,
        } = committed;
        let Some(assignment) = assignments.pop() else {
            assert!(retirement.is_none());
            assert_eq!(unused_grants.len(), 1);
            drop(unused_grants);
            return None;
        };
        assert!(assignments.is_empty());
        assert!(unused_grants.is_empty());
        let (_, execution, work) = assignment.into_parts();
        drop(execution);
        Some(CommittedCheckout {
            work,
            retirement: retirement.expect("an assignment commits its owner transition"),
        })
    }

    pub(in crate::authority) fn checkout_next(
        &self,
        permit: WorkPermit,
    ) -> Result<Option<CommittedCheckout>, PlanError> {
        self.checkout_one_for_foundation(permit)
    }

    pub(in crate::authority) fn checkout_for_foundation(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: WorkPermit,
    ) -> Result<CommittedCheckout, PlanError> {
        let committed = self
            .apply_compute_exchange_for_owner(
                Self::foundation_compute_grant(permit),
                key,
                expected,
                permit,
            )
            .map_err(Self::foundation_exchange_error)?;
        Self::committed_checkout_for_foundation(committed)
            .ok_or(PlanError::Backpressure(Backpressure::ComputeResources))
    }
}
