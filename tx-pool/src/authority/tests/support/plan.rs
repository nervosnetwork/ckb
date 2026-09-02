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
        EffectWakeProjectionInput,
    },
    indexes::test_support::IndexSnapshot,
    resources::{ActiveWorkAvailability, ResourceRead, test_support::ResourceSnapshot},
    scheduler::{CheckoutTicket, test_support::SchedulerSnapshot},
    state::{AcceptedStatus, ComputeAttribution, TxIdentity},
    work::CheckedOutWork,
};
use super::*;
use crate::authority::ingress::{
    RetainedAdmissionBatch, RetainedIngress, RetainedIngressAttempt, RetainedIngressBoundaryError,
    RetainedIngressKind, RetainedIngressRejection,
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

pub(in crate::authority) fn retired_buffer_capacity_for_foundation(
    requested: usize,
) -> Result<usize, PlanError> {
    retired_buffer(requested).map(|buffer| buffer.capacity())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum ReleasedInputContextForFoundation {
    Replacement { candidate_uses_input: bool },
    Administrative,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) enum MissingResolutionObservationForFoundation {
    Wait,
    RejectUnknownCell(OutPoint),
    RejectInvalidHeader(ckb_types::packed::Byte32),
    UnexpectedReject(CommittedPublicReject),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum SettlementClassificationObservationForFoundation {
    QueuedResolve,
    QueuedVerify,
    Waiting,
    Ready,
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct WakeProjectionInput {
    pub(in crate::authority) scheduler: [Option<u128>; 4],
    pub(in crate::authority) active_work: usize,
    pub(in crate::authority) dependency_maintenance: bool,
    pub(in crate::authority) effects: EffectWakeProjectionInput,
    pub(in crate::authority) template_sources: [u128; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct WakeObservation {
    pub(in crate::authority) compute: bool,
    pub(in crate::authority) ready: bool,
    pub(in crate::authority) dependency_maintenance: bool,
    pub(in crate::authority) effect_publisher: bool,
    pub(in crate::authority) effect_capacity: bool,
    pub(in crate::authority) template: bool,
}

impl WakeProjectionInput {
    fn into_production(self) -> AuthorityWakeProjection {
        let [resolve, verify_small, verify_any, ready] = self.scheduler;
        let [proposals, transactions, chain] = self.template_sources;
        AuthorityWakeProjection {
            scheduler: SchedulerWakeProjection {
                resolve: resolve.map(EntryVersion),
                verify_small: verify_small.map(EntryVersion),
                verify_any: verify_any.map(EntryVersion),
                ready: ready.map(EntryVersion),
            },
            active_work: self.active_work,
            effects: EffectWakeProjection::from_input(self.effects),
            template: [
                ApplySequence(proposals),
                ApplySequence(transactions),
                ApplySequence(chain),
            ],
        }
    }
}

impl AuthorityWakeTransition {
    pub(in crate::authority) fn observe_for_foundation(
        before: WakeProjectionInput,
        after: WakeProjectionInput,
    ) -> WakeObservation {
        let dependency_maintenance_activated =
            !before.dependency_maintenance && after.dependency_maintenance;
        let mut transition = Self::between(before.into_production(), after.into_production());
        transition.dependency_maintenance_activated = dependency_maintenance_activated;
        WakeObservation {
            compute: transition.compute_advanced(),
            ready: transition.ready_advanced(),
            dependency_maintenance: transition.dependency_maintenance_activated(),
            effect_publisher: transition.effect_publisher_advanced(),
            effect_capacity: transition.effect_capacity_released(),
            template: transition.owner_source_advanced(),
        }
    }
}

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

impl CompiledSharedRemoteExpiry {
    pub(in crate::authority) fn administrative_removal_keys_for_claim(
        &self,
    ) -> Option<Vec<RawTxHash>> {
        Some(self.removal.removal.hashes.clone())
    }

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

    pub(in crate::authority) fn corrupt_resource_witness_for_foundation(&mut self) -> bool {
        self.removal
            .removal
            .resources
            .swap_first_owner_witnesses_for_foundation()
    }

    pub(in crate::authority) fn corrupt_proposed_witness_for_foundation(&mut self) -> bool {
        self.removal
            .removal
            .membership
            .erase_first_proposed_removal_for_foundation()
    }

    pub(in crate::authority) fn administrative_removal_keys_for_claim(
        &self,
    ) -> Option<Vec<RawTxHash>> {
        Some(self.removal.removal.hashes.clone())
    }

    pub(in crate::authority) fn apply_for_foundation(
        self,
        authority: &TxPoolAuthority,
    ) -> CommittedDelta {
        let shared = self
            .bind(authority)
            .expect("the foundation Accepted-expiry generation remains current")
            .apply()
            .unwrap_or_else(|failure| {
                let (error, _effect_wake) = failure.into_parts();
                panic!("the foundation Accepted-expiry cut commits: {error:?}")
            });
        let (committed, post_commit_fault) = shared.into_parts();
        assert_eq!(post_commit_fault, None);
        committed
    }
}

impl TxPoolAuthority {
    pub(in crate::authority) fn set_membership_secondary_read_probe_for_foundation(
        &self,
        hash: RawTxHash,
        probe: Arc<crate::authority::shard::ConcurrentRemovalProbe>,
    ) {
        self.entries
            .set_membership_secondary_read_probe(hash, probe);
    }

    pub(in crate::authority) fn owner_shard_write_available_for_foundation(
        &self,
        hash: &RawTxHash,
    ) -> bool {
        let support = self.entries.owner_write_support(std::iter::once(hash));
        self.entries.try_write_cut(support).is_some()
    }

    pub(in crate::authority) fn commit_retained_rejection_for_foundation(
        &self,
        rejection: RetainedIngressRejection,
    ) -> Result<CommittedRetainedAdmissionBatch, PlanError> {
        let malformed =
            rejection.is_malformed() && matches!(rejection.kind(), RetainedIngressKind::Remote(_));
        let batch = RetainedAdmissionBatch::new(
            RetainedIngressAttempt::Rejected(rejection),
            std::collections::VecDeque::new(),
        )
        .map_err(|error| match error {
            RetainedIngressBoundaryError::ResourceUnavailable => {
                PlanError::Backpressure(Backpressure::Allocation)
            }
            RetainedIngressBoundaryError::InvalidEvidence => {
                PlanError::Fault(AuthorityFault::MembershipProjection)
            }
            RetainedIngressBoundaryError::Backpressure(_) => {
                PlanError::Backpressure(Backpressure::Allocation)
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
                    match error {
                        ConcurrentRetainedIngressError::Stale => {
                            PlanError::Stale(StalePlan::Version)
                        }
                        ConcurrentRetainedIngressError::Fault(fault) => PlanError::Fault(fault),
                        ConcurrentRetainedIngressError::Backpressure(pressure) => {
                            PlanError::Backpressure(pressure)
                        }
                    }
                });
        }
        Ok(self
            .plan_shared_retained_effect_prefix(&batch)?
            .ok_or(PlanError::Stale(StalePlan::Version))?
            .apply())
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

    pub(in crate::authority) fn scheduler_resolve_head_for_foundation(
        &self,
    ) -> Option<EntryVersion> {
        self.scheduler.lock().wake_projection().resolve
    }

    pub(in crate::authority) fn plan_retained_admission(
        &mut self,
        ingress: RetainedIngress,
    ) -> Result<RetainedAdmissionDisposition<'_>, PlanError> {
        let (kind, admission) = ingress.into_parts();
        let key = admission.identity.raw.clone();
        let banned = if let RetainedIngressKind::Remote(peer) = kind {
            self.entries
                .peer_is_banned_at(peer, Instant::now())
                .map_err(|error| match error {
                    super::super::shard::PeerFenceStageError::Stale => {
                        PlanError::Stale(StalePlan::Version)
                    }
                    super::super::shard::PeerFenceStageError::Allocation => {
                        PlanError::Backpressure(Backpressure::Allocation)
                    }
                })?
        } else {
            false
        };
        if let RetainedIngressKind::Remote(peer) = kind
            && banned
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

        let existing = self.entries.get(&key).as_deref().cloned();
        match kind {
            RetainedIngressKind::Remote(peer) => match existing.as_ref() {
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
            RetainedIngressKind::Proposal => match existing.as_ref() {
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
    pub(in crate::authority) fn first_entry_difference(&self, other: &Self) -> Option<String> {
        let mut hashes = self
            .entries
            .keys()
            .chain(other.entries.keys())
            .cloned()
            .collect::<Vec<_>>();
        hashes.sort_unstable();
        hashes.dedup();
        hashes.into_iter().find_map(|hash| {
            let left = self.entries.get(&hash);
            let right = other.entries.get(&hash);
            match (left, right) {
                (Some(left), Some(right)) if left != right => {
                    let mut fields = Vec::new();
                    if left.identity != right.identity {
                        fields.push("identity");
                    }
                    if left.source != right.source {
                        fields.push("source");
                    }
                    if left.version != right.version {
                        fields.push("version");
                    }
                    if left.arrival != right.arrival {
                        fields.push("arrival");
                    }
                    if left.charge != right.charge {
                        fields.push("charge");
                    }
                    if left.phase != right.phase {
                        fields.push("phase");
                    }
                    Some(format!("{hash:?}:{}", fields.join(",")))
                }
                (Some(_), None) => Some(format!("{hash:?}:left_only")),
                (None, Some(_)) => Some(format!("{hash:?}:right_only")),
                (Some(_), Some(_)) | (None, None) => None,
            }
        })
    }

    pub(in crate::authority) fn first_difference(&self, other: &Self) -> Option<&'static str> {
        if self.generation != other.generation {
            Some("generation")
        } else if self.chain_view != other.chain_view {
            Some("chain_view")
        } else if self.entries != other.entries {
            Some("entries")
        } else if self.indexes != other.indexes {
            Some("indexes")
        } else if self.relay_parent_sources != other.relay_parent_sources {
            Some("relay_parent_sources")
        } else if self.source_versions != other.source_versions {
            Some("source_versions")
        } else if self.resources != other.resources {
            Some("resources")
        } else if self.membership != other.membership {
            Some("membership")
        } else if self.scheduler != other.scheduler {
            Some("scheduler")
        } else if self.dependencies != other.dependencies {
            Some("dependencies")
        } else if self.effects != other.effects {
            Some("effects")
        } else if self.peer_bans != other.peer_bans {
            Some("peer_bans")
        } else if self.clocks != other.clocks {
            Some("clocks")
        } else {
            None
        }
    }

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
        let source_versions_equivalent = own_template.proposals
            == other_template
                .proposals
                .compact_barrier_for_foundation(batch_sequence, canonical_next_sequence)
            && own_template.transactions
                == other_template
                    .transactions
                    .compact_barrier_for_foundation(batch_sequence, canonical_next_sequence)
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
            && self.relay_parent_sources == other.relay_parent_sources
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
                        observed,
                        observation,
                    },
                    OwnerPhaseSnapshot::ReplacementHistory {
                        dependencies: other_dependencies,
                        observed: other_observed,
                        observation: other_observation,
                    },
                ) => {
                    dependencies == other_dependencies
                        && observed == other_observed
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
    pub(in crate::authority) fn compute_wake_for_foundation(&self) -> bool {
        self.wake.compute_advanced()
    }

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
    /// Observe only the size of the production-sealed proposed-count delta.
    /// This proves that a Pending/Gap-only status transition does not retain a
    /// redundant aggregate write without creating a second status model.
    pub(in crate::authority) fn proposed_count_delta_len_for_foundation(&self) -> Option<usize> {
        let PreparedApplyKind::Dependency(prepared) = &self.kind else {
            return None;
        };
        let DependencyAuthorityDelta::Membership(delta) = &prepared.delta else {
            return None;
        };
        Some(delta.projection.proposed_count_plan().len())
    }
}

impl PreparedIndependentApply<'_> {
    /// Inspect the already-sealed independent Apply order without retaining a
    /// second production receipt after the transition commits.
    pub(in crate::authority) fn independent_order_for_foundation(&self) -> Option<Vec<RawTxHash>> {
        let delta = match self {
            Self::Shared { delta, .. } | Self::Exclusive { delta, .. } => delta,
        };
        Some(
            delta
                .owner_cuts
                .iter()
                .filter_map(|owner| {
                    matches!(
                        owner.action,
                        IndependentOwnerAction::Replace(Some(OwnedTx::Accepted(_)))
                    )
                    .then_some(owner.key.clone())
                })
                .collect(),
        )
    }
}

impl CompiledSharedIndependent {
    pub(in crate::authority) fn physical_apply_support_for_foundation(
        &self,
    ) -> super::super::shard::ShardApplySupport {
        self.support
    }

    pub(in crate::authority) fn shared_direct_matches_canonical_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> Result<bool, PlanError> {
        let [owner] = self.delta.owner_cuts.as_slice() else {
            return Ok(false);
        };
        let IndependentOwnerAction::Replace(Some(OwnedTx::Accepted(candidate))) = &owner.action
        else {
            return Ok(false);
        };
        authority.direct_absent_matches_canonical_for_foundation(&owner.key, candidate)
    }

    pub(in crate::authority) fn physical_write_support_for_foundation(
        &self,
        _authority: &TxPoolAuthority,
    ) -> super::super::shard::ShardWriteSupport {
        self.support.writes()
    }

    pub(in crate::authority) fn physical_read_support_for_foundation(
        &self,
        _authority: &TxPoolAuthority,
    ) -> super::super::shard::ShardReadSupport {
        self.support.reads()
    }

    pub(in crate::authority) fn dependency_stage_write_support_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> super::super::shard::ShardWriteSupport {
        self.delta
            .dependency
            .relation_stage_write_support_for_foundation(&authority.entries)
    }

    pub(in crate::authority) fn dependency_ready_phase_shape_for_foundation(&self) -> bool {
        self.delta.dependency.ready_phase_shape_for_foundation()
    }

    pub(in crate::authority) fn membership_prestate_is_fresh_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> bool {
        let support = self.delta.physical_support(authority);
        let cut = authority
            .entries
            .mixed_cut(support.reads(), support.writes());
        self.delta
            .projection
            .prestate_is_fresh(&authority.entries, &cut)
    }

    pub(in crate::authority) fn index_prestate_is_fresh_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> bool {
        let support = self.delta.physical_write_support(authority);
        let cut = authority.entries.write_cut(support);
        self.delta
            .owners
            .indexes
            .prestate_is_fresh(&authority.entries, &cut)
    }

    pub(in crate::authority) fn resource_prestate_is_fresh_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> bool {
        let support = self.delta.physical_write_support(authority);
        let cut = authority.entries.write_cut(support);
        self.delta
            .resource
            .as_ref()
            .is_none_or(|resources| cut.resource_plan_is_fresh(resources.shard_plan()))
    }

    pub(in crate::authority) fn dependency_prestate_is_fresh_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> bool {
        let support = self.delta.physical_write_support(authority);
        let read_support = self
            .delta
            .dependency
            .sharded_read_support(&authority.entries);
        let cut = authority.entries.mixed_cut(read_support, support);
        self.delta
            .dependency
            .prestate_is_fresh(&authority.entries, &cut)
    }

    pub(in crate::authority) fn dependency_phase_transition_is_staged_for_foundation(
        &self,
        _authority: &TxPoolAuthority,
    ) -> bool {
        self.delta
            .dependency
            .has_consumer_phase_transition_for_foundation()
    }

    pub(in crate::authority) fn dependency_final_support_masks_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> (u64, u64) {
        (
            self.delta
                .dependency
                .sharded_read_support(&authority.entries)
                .mask_for_foundation(),
            self.delta
                .dependency
                .sharded_owner_commit_write_support(&authority.entries)
                .mask_for_foundation(),
        )
    }

    pub(in crate::authority) fn scheduler_prestate_is_fresh_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> bool {
        self.delta
            .scheduler
            .prestate_is_fresh(&authority.scheduler.lock())
    }
}

impl PreparedSharedDirectAdmissionDisposition<'_> {
    pub(in crate::authority) fn physical_apply_support_for_foundation(
        &self,
    ) -> Option<super::super::shard::ShardApplySupport> {
        match self {
            Self::Accepted { compiled, .. } => Some(compiled.support),
            Self::EffectOnly(_) => None,
        }
    }

    pub(in crate::authority) fn is_compatible_with_for_foundation(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Accepted { compiled: left, .. },
                Self::Accepted {
                    compiled: right, ..
                },
            ) => left.is_compatible_with(right),
            (Self::Accepted { .. } | Self::EffectOnly(_), _) => false,
        }
    }

    pub(in crate::authority) fn matches_vacant_canonical_for_foundation(
        &self,
        authority: &TxPoolAuthority,
    ) -> Result<bool, PlanError> {
        match self {
            Self::Accepted { compiled, .. } => {
                compiled.shared_direct_matches_canonical_for_foundation(authority)
            }
            Self::EffectOnly(_) => Ok(false),
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

    pub(in crate::authority) fn ready_physical_counts_for_foundation(
        &self,
    ) -> (usize, usize, usize) {
        self.scheduler.lock().ready_physical_counts_for_foundation()
    }

    pub(in crate::authority) fn scheduler_frontier_for_foundation(
        &self,
    ) -> std::sync::Arc<ckb_util::parking_lot::Mutex<super::super::scheduler::FairFrontier>> {
        std::sync::Arc::clone(&self.scheduler)
    }

    pub(in crate::authority) fn source_versions_lock_for_foundation(
        &self,
    ) -> ckb_util::parking_lot::MutexGuard<'_, super::super::source::AuthoritySourceVersionSnapshot>
    {
        self.source_versions.lock_for_foundation()
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

    pub(in crate::authority) fn with_replacement_and_effect_limits(
        limits: ResourceLimits,
        minimum_rate: ckb_types::core::FeeRate,
        effect_limits: EffectLimits,
    ) -> Result<Self, AuthorityConfigError> {
        let mut authority = Self::new(limits, VerifyOrder::Arrival, effect_limits)?;
        authority.replace_membership_config_for_test(
            &AuthorityTestToken(()),
            MembershipConfig::testing_with_replacement(minimum_rate),
        );
        Ok(authority)
    }

    pub(in crate::authority) fn with_max_ancestors_for_foundation(
        limits: ResourceLimits,
        max_ancestors: usize,
    ) -> Self {
        let mut authority = Self::for_foundation(limits);
        authority.replace_membership_config_for_test(
            &AuthorityTestToken(()),
            MembershipConfig::from_runtime(
                max_ancestors,
                crate::constants::MAX_POOL_MUTATION_CANDIDATES,
                None,
            ),
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

    pub(in crate::authority) fn for_foundation_with_order(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
    ) -> Self {
        Self::from_test(
            &AuthorityTestToken(()),
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

    pub(in crate::authority) fn entries_for_reference(
        &self,
    ) -> &crate::authority::shard::ShardedOwnerMap {
        &self.entries
    }

    pub(in crate::authority) fn scheduler_cursors_for_refinement(
        &self,
    ) -> (
        Option<super::super::scheduler::WorkOwner>,
        Option<super::super::scheduler::WorkOwner>,
    ) {
        self.scheduler.lock().cursors_for_refinement()
    }

    pub(in crate::authority) fn scheduler_worker_wave_for_refinement(
        &self,
        slots: &[super::super::exchange::ComputeWorkerSlot],
    ) -> Result<super::super::scheduler::test_support::SchedulerWaveObservation, PlanError> {
        self.scheduler
            .lock()
            .worker_wave_observation(slots)
            .map_err(PlanError::from)
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

    pub(in crate::authority) fn has_reserved_resource_capacity_for_foundation(&self) -> bool {
        self.resources
            .capacity_observation()
            .has_reserved_capacity_for_foundation()
    }

    pub(in crate::authority) fn reserve_primary_owner_capacity_for_foundation(
        &mut self,
        additional: usize,
    ) -> Result<usize, PlanError> {
        let key = RawTxHash(ckb_types::packed::Byte32::default());
        self.reserve_primary_owner_insertions(std::iter::repeat_n(&key, additional))?;
        Ok(self.entries.capacity())
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

    pub(in crate::authority) fn released_input_for_foundation(
        &self,
        removed_entry: &RawTxHash,
        input: &OutPoint,
        removed: &[RawTxHash],
        context: ReleasedInputContextForFoundation,
    ) -> Result<bool, PlanError> {
        let entry = match self.entries.get(removed_entry).as_deref() {
            Some(OwnedTx::Accepted(entry)) => entry.clone(),
            Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        };
        match context {
            ReleasedInputContextForFoundation::Replacement {
                candidate_uses_input,
            } => {
                let removed = removed.iter().cloned().collect::<HashSet<_>>();
                let candidate_inputs = if candidate_uses_input {
                    [input.clone()].into_iter().collect::<HashSet<_>>()
                } else {
                    HashSet::new()
                };
                self.released_input_backing_in_final_owner_set(
                    &entry,
                    input,
                    ProjectedFinalOwnerSet {
                        removed: ProjectedRemovalSet::Replacement(&removed),
                    },
                    ReleasedInputContext::Replacement {
                        candidate_inputs: &candidate_inputs,
                    },
                )
                .map(|backing| backing.is_available())
            }
            ReleasedInputContextForFoundation::Administrative => {
                let removed = AcceptedRemovalSet::try_from_vec(removed.to_vec())?;
                self.released_input_backing_in_final_owner_set(
                    &entry,
                    input,
                    ProjectedFinalOwnerSet {
                        removed: ProjectedRemovalSet::Administrative(&removed),
                    },
                    ReleasedInputContext::Administrative {
                        victim: removed_entry,
                    },
                )
                .map(|backing| backing.is_available())
            }
        }
    }

    pub(in crate::authority) fn generation(&self) -> PoolGeneration {
        self.generation
    }

    pub(in crate::authority) fn chain_view_for_reference(&self) -> &ChainViewId {
        &self.chain_view
    }

    pub(in crate::authority) fn clocks(&self) -> AuthorityClocks {
        self.clocks.snapshot()
    }

    pub(in crate::authority) fn membership_snapshot_for_reference(&self) -> MembershipSnapshot {
        self.membership.snapshot(self.membership_counts())
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

    pub(in crate::authority) fn template_source_versions_for_reference(
        &self,
    ) -> PoolTemplateVersions {
        self.template_source_versions()
    }

    pub(in crate::authority) fn force_chain_view(&mut self, view: ChainViewId) {
        self.replace_chain_view_for_test(&AuthorityTestToken(()), view);
    }

    pub(in crate::authority) fn force_next_sequence(&mut self, sequence: ApplySequence) {
        self.replace_next_sequence_for_test(&AuthorityTestToken(()), sequence);
    }

    pub(in crate::authority) fn force_next_version(&mut self, version: EntryVersion) {
        self.replace_next_version_for_test(&AuthorityTestToken(()), version);
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
                    .as_deref()
                    .cloned()
                    .ok_or(PlanError::Stale(StalePlan::Missing))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.collect_dependency_loss_keys(parents.iter())?.work)
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

    pub(in crate::authority) fn exclusive_membership_witness_activity_for_foundation(
        &mut self,
        receipt: FinalAdmissionReceipt,
    ) -> Result<(usize, usize), PlanError> {
        let delta = self.prepare_accept_delta(receipt)?;
        Ok(delta.projection.policy_witness_activity_for_foundation())
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

    pub(in crate::authority) fn final_admission_receipt_at_for_foundation(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
        status: AcceptedStatus,
        accepted_at: AcceptedAtMillis,
    ) -> Result<FinalAdmissionReceipt, PlanError> {
        self.final_admission_work(key, expected)?
            .validate_at_for_foundation(status, ScriptVerificationRules::V0, accepted_at)
            .map_err(|_| PlanError::Stale(StalePlan::ChainRevision))
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

    pub(in crate::authority) fn validate_final_acceptance_for_foundation(
        &self,
        key: &RawTxHash,
        receipt: &FinalAdmissionReceipt,
    ) -> Result<(), PlanError> {
        let existing = self
            .entries
            .get(key)
            .as_deref()
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        self.validate_acceptance_evidence(preaccepted, receipt)
    }

    pub(in crate::authority) fn validate_final_subject_for_foundation(
        &self,
        subject: &FinalAdmissionSubject,
    ) -> Result<(), PlanError> {
        self.final_admission_subject_owner(subject).map(|_| ())
    }

    pub(in crate::authority) fn validate_direct_acceptance_for_foundation(
        &self,
        receipt: &DirectAdmissionReceipt,
    ) -> Result<(), PlanError> {
        self.validate_direct_acceptance_evidence(receipt)
    }

    pub(in crate::authority) fn missing_resolution_observation_for_foundation(
        &self,
        source: PreAcceptedSource,
        missing: &MissingDependencies,
    ) -> MissingResolutionObservationForFoundation {
        match self.missing_resolution_disposition(source, missing) {
            MissingResolutionDisposition::Wait => MissingResolutionObservationForFoundation::Wait,
            MissingResolutionDisposition::Reject(rejection) => {
                let rejection = rejection.into_public();
                match rejection.reject() {
                    Reject::Resolve(OutPointError::Unknown(out_point)) => {
                        MissingResolutionObservationForFoundation::RejectUnknownCell(
                            out_point.clone(),
                        )
                    }
                    Reject::Resolve(OutPointError::InvalidHeader(header)) => {
                        MissingResolutionObservationForFoundation::RejectInvalidHeader(
                            header.clone(),
                        )
                    }
                    _ => MissingResolutionObservationForFoundation::UnexpectedReject(rejection),
                }
            }
        }
    }

    pub(in crate::authority) fn classify_settlement_for_foundation(
        &self,
        settlement: ComputeSettlement,
    ) -> Result<SettlementClassificationObservationForFoundation, PlanError> {
        let ComputeSettlement { token, next } = settlement;
        let existing = self
            .entries
            .get(&token.hash)
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != token.version {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &*existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        if preaccepted.charge.active_work != 1 {
            return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
        }
        let (candidate_dependencies, missing_dependencies) = settlement_dependency_inputs(&next);
        let dependency = self.dependencies.capture_settlement_evidence(
            &token.hash,
            preaccepted.dependencies(),
            candidate_dependencies,
            missing_dependencies,
        )?;
        Ok(
            match self.classify_settlement(preaccepted, active, &dependency, next)? {
                SettlementClassification::OwnerLocal(OwnerLocalSettlement { phase, .. }) => {
                    match phase {
                        OwnerLocalPhase::Resolve => {
                            SettlementClassificationObservationForFoundation::QueuedResolve
                        }
                        OwnerLocalPhase::Verify(_) => {
                            SettlementClassificationObservationForFoundation::QueuedVerify
                        }
                        OwnerLocalPhase::Ready(_) => {
                            SettlementClassificationObservationForFoundation::Ready
                        }
                    }
                }
                SettlementClassification::NonLocal(NonLocalSettlement::Waiting(_)) => {
                    SettlementClassificationObservationForFoundation::Waiting
                }
                SettlementClassification::NonLocal(
                    NonLocalSettlement::Rejected(_)
                    | NonLocalSettlement::VerificationRejected { .. },
                ) => SettlementClassificationObservationForFoundation::Rejected,
            },
        )
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
        PreparedApply::stage(self, DependencyAuthorityDelta::Membership(delta))
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

        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let mut after = before.clone();
        after.record.version = version;
        after.proposal = super::super::chain::ProposalContextReceipt::from_internal_status(status);
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
        let resource = self
            .resources_for_test_plan()
            .plan_batch(resource_changes)?;
        let scheduler = self
            .scheduler
            .lock()
            .plan_replace(Some(&existing), Some(&after), None)?;
        let dependency =
            self.plan_membership_dependency_delta(Some(&existing), &after, &[], sequence)?;
        let owners =
            self.plan_membership_owner_derivations(key, Some(&existing), &after, &[], sequence)?;
        PreparedApply::stage(
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
                clocks: clocks.finish(),
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
        Ok(self.prepared_effect_only(effect, clocks))
    }

    pub(in crate::authority) fn plan_generation_reset_for_foundation(
        &mut self,
    ) -> Result<PreparedApply<'_>, PlanError> {
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let effect = self.effects.lock().plan_generation_reset(sequence)?;
        Ok(self.prepared_effect_only(effect, clocks))
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
    ) -> Result<Option<PreparedSharedLocalRemoval<'_>>, PlanError> {
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
            hashes
                .try_reserve(indexed.len())
                .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))?;
            hashes.extend(indexed.iter().cloned());
        }
        hashes.sort_unstable();
        let marker = self
            .peer_bans_for_test_plan()
            .plan_record(peer, observed_at)?;
        let revocation =
            CommittedPeerCohortRevocation::administrative_for_foundation(marker.lease());
        let delta = self.compile_administrative_removal(hashes, marker, revocation)?;
        PreparedApply::stage(self, DependencyAuthorityDelta::Admin(delta))
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

    pub(in crate::authority) fn effect_publication_observation_for_foundation(
        &self,
    ) -> EffectPublicationObservationSnapshot {
        self.effect_publication_observation().snapshot()
    }

    pub(in crate::authority) fn apply_effect_settlement_for_foundation(
        &mut self,
        settlement: EffectSettlement,
    ) -> Result<CommittedDelta, EffectSettlementFailure> {
        self.apply_effect_settlement(settlement)
            .map(|(commit, _next)| match commit {
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
            .map(|(commit, _next)| commit)
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
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        self.prepare_shared_dependency_control_for_foundation(control, clocks.finish())
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
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        self.prepare_shared_dependency_control_for_foundation(control, clocks.finish())
            .map(Some)
    }

    fn prepare_shared_dependency_control_for_foundation(
        &self,
        control: DependencyEntryControlDelta,
        clocks: AuthorityClocks,
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
            clocks,
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

    /// Coherently move one test Remote deadline without changing ownership,
    /// resources, scheduler state, or version. Production residency tickets
    /// are immutable; this isolated interposition exists only to prove that
    /// the Remote-expiry prefix/head witness, rather than an adjacent OCC row,
    /// rejects a newly earlier index entry.
    pub(in crate::authority) fn reticket_remote_deadline_for_foundation(
        &self,
        hash: &RawTxHash,
        expires_at: RemoteDeadline,
    ) {
        let shard = self.entries.owner_shard(hash);
        let mut support = super::super::shard::ShardWriteSupport::default();
        support.insert(shard);
        let mut cut = self.entries.write_cut(support);
        let mut owner = cut
            .replace(shard, hash.clone(), None)
            .expect("the Remote reticket fixture owner exists");
        let OwnedTx::PreAccepted(entry) = &mut owner else {
            panic!("the Remote reticket fixture stays preaccepted");
        };
        let PreAcceptedSource::Remote(remote) = &mut entry.source else {
            panic!("the Remote reticket fixture stays under direct Remote policy");
        };
        let previous = remote.residency.expires_at;
        assert_ne!(previous, expires_at);
        let previous_key = super::super::indexes::DeadlineKey {
            expires_at: previous,
            hash: hash.clone(),
        };
        let next_key = super::super::indexes::DeadlineKey {
            expires_at,
            hash: hash.clone(),
        };
        assert!(
            cut.projection_shard_mut(shard)
                .deadlines
                .remove(&previous_key)
        );
        assert!(cut.projection_shard_mut(shard).deadlines.insert(next_key));
        remote.residency.expires_at = expires_at;
        assert!(cut.replace(shard, hash.clone(), Some(owner)).is_none());
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

    pub(in crate::authority) fn invalidate_compute_exchange_cursor_for_foundation(
        &self,
        version: EntryVersion,
    ) {
        self.scheduler
            .lock()
            .invalidate_compute_exchange_cursor_for_foundation(version);
    }

    pub(in crate::authority) fn reserve_unrelated_compute_clock_for_foundation(
        &self,
    ) -> (EntryVersion, ApplySequence) {
        let plan = ClockPlanReservation::begin(Arc::clone(&self.clocks));
        let (version, plan) = plan
            .replacement()
            .expect("the fixture replacement identity remains available");
        let clocks = plan
            .commit()
            .expect("the fixture sequence identity remains available");
        let sequence = clocks.sequence();
        let _ = clocks.finish();
        (version, sequence)
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
            .resources()
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
        let scheduler = std::sync::Arc::clone(&self.scheduler);
        let owner_count = scheduler.lock().owner_count_for_reference(permit);
        let mut wave = scheduler.lock().checkout_wave(1)?;
        let mut cursor = None;
        let mut selected = None;
        let mut probes = 0usize;
        for _ in 0..owner_count {
            let ticket = match cursor {
                Some(owner) => scheduler
                    .lock()
                    .next_queued_after_in_wave_for_reference(&wave, permit, owner),
                None => scheduler
                    .lock()
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
            .as_deref()
            .cloned()
            .ok_or(PlanError::Stale(StalePlan::Missing))?;
        if existing.record().version != expected {
            return Err(PlanError::Stale(StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &existing else {
            return Err(PlanError::Stale(StalePlan::Phase));
        };
        let attribution = preaccepted.source.compute_attribution();
        match self
            .resources()
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
            .resources_for_test_plan()
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
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (version, clocks) = clocks.replacement()?;
        let (work, active) = CheckedOutWork::from_owner(
            version,
            self.chain_view.clone(),
            DependencyCut(sequence),
            permit,
            grant,
            preaccepted,
        )
        .map_err(|_| PlanError::Stale(StalePlan::Phase))?;
        let after = existing
            .with_preaccepted_phase(PreAcceptedPhase::Computing(active), version, after_charge)
            .map_err(PlanError::Stale)?;
        let scheduler =
            self.scheduler
                .lock()
                .plan_replace(Some(&existing), Some(&after), Some(ticket))?;
        let dependency = self
            .dependencies
            .plan_replace(Some(&existing), Some(&after))?
            .into_shared_batch(&self.dependencies, None)?;
        let sources = self
            .source_versions
            .plan_replacements(std::iter::once((Some(&existing), Some(&after))), sequence);
        let template_sources =
            self.plan_owner_sources(std::iter::once((key, Some(&existing), Some(&after))))?;
        let indexes =
            self.indexes_for_test_plan()
                .plan_replace(key, Some(&existing), Some(&after))?;
        let plan = PreparedApply::stage(
            self,
            DependencyAuthorityDelta::Entry(EntryDelta {
                key: key.clone(),
                expected: OwnerPrestate::PreAccepted(existing.record().version),
                after: Some(after),
                owners: DerivedOwnerDelta {
                    indexes,
                    sources,
                    template_sources,
                },
                retired: RetiredOwners::default(),
                resource: resources,
                scheduler,
                dependency,
                effect: EffectDelta::default(),
                clocks: clocks.finish(),
            }),
        )?;
        Ok(PreparedCheckout { plan, work })
    }

    pub(in crate::authority) fn plan_checkout_for_foundation(
        &mut self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: super::super::state::WorkPermit,
    ) -> Result<PreparedCheckout<'_>, PlanError> {
        let attribution = {
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
            preaccepted.source.compute_attribution()
        };
        match self
            .resources()
            .active_work_availability_for_reference(attribution)?
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
            .lock()
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
