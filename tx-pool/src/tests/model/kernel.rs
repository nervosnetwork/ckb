use super::state::{
    AcceptedProvenance, AcceptedStatus, ApplyStamp, Arrival, CapabilityId, CellId, ChainView,
    DirectCapability, DirectKind, DirectRequestId, EffectClaim, EffectClaimSource, EffectClass,
    EffectRecord, EntryVersion, FinishedWorkCapability, HeaderId, InputOrigin, LogicalEffect,
    MembershipRejection, MissingDependencies, ModelInvariantError, MonotonicTick, Omega, Owner,
    OwnerLocation, PeerBanDeadline, PeerBanRecord, PeerId, PoolGeneration, ProposalBase, ReadyKey,
    RemoteDeadline, ResolvedEvidence, RetainedOwner, RetainedPhase, RetainedSource, RulesId,
    Source, Transaction, TxId, VerifyCapability, ViewId, WorkCapability, WorkPermit, WorkStage,
};
use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU16;

pub(super) use super::state::WorkResult;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Admission {
    pub(super) transaction: Transaction,
    pub(super) source: RetainedSource,
    pub(super) observed_at: MonotonicTick,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum WorkCompletionPlan {
    ContinueVerify(ResolvedEvidence),
    Ready(ResolvedEvidence),
    RequeueResolve,
    Wait(MissingDependencies),
    Reject,
    Invalid,
    PrivateContinuationRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompletionLocation {
    Executing,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckoutRoute {
    Split,
    Continuous(VerifyCapability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvidenceValidity {
    Current,
    RelevantChange,
    Invalid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Completion {
    pub(super) capability: CapabilityId,
    pub(super) result: WorkResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolveContinuation {
    pub(super) capability: CapabilityId,
    pub(super) evidence: ResolvedEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DirectNegativeReason {
    MissingDependency,
    Policy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CellObservation {
    pub(super) producer: Option<(TxId, EntryVersion)>,
    pub(super) spender: Option<(TxId, EntryVersion)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectNegativeEvidence {
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) witness: super::state::WitnessId,
    pub(super) reason: DirectNegativeReason,
    pub(super) reads: BTreeMap<CellId, CellObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DirectWorkResult {
    Verified(ResolvedEvidence),
    Rejected(DirectNegativeEvidence),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectCompletion {
    pub(super) capability: CapabilityId,
    pub(super) wall_time: u64,
    pub(super) result: DirectWorkResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReadyCapture {
    pub(super) keys: Vec<ReadyKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MembershipEvaluation {
    Accepted(MembershipAcceptance),
    Rejected(MembershipRejection),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MembershipAcceptance {
    replacement_victims: Vec<TxId>,
    capacity_victims: Vec<TxId>,
    late_children: Vec<TxId>,
    owner_loss: OwnerLossPlan,
    replacement_history: BTreeMap<TxId, MissingDependencies>,
}

impl MembershipAcceptance {
    fn is_independent_insert(&self) -> bool {
        self.replacement_victims.is_empty()
            && self.capacity_victims.is_empty()
            && self.late_children.is_empty()
            && self.owner_loss.is_empty()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct OwnerLossPlan {
    terminal: BTreeSet<TxId>,
    remote_missing: BTreeMap<TxId, BTreeSet<CellId>>,
    released_cells: BTreeSet<CellId>,
}

impl OwnerLossPlan {
    fn is_empty(&self) -> bool {
        self.terminal.is_empty() && self.remote_missing.is_empty() && self.released_cells.is_empty()
    }

    fn terminal_dependents(&self, roots: &BTreeSet<TxId>) -> Vec<TxId> {
        self.terminal.difference(roots).copied().collect()
    }

    fn parent_request_effects(&self) -> Option<Vec<LogicalEffect>> {
        Self::parent_request_effects_for(&self.remote_missing)
    }

    fn parent_request_effects_for(
        remote_missing: &BTreeMap<TxId, BTreeSet<CellId>>,
    ) -> Option<Vec<LogicalEffect>> {
        remote_missing
            .iter()
            .map(|(transaction, missing)| {
                LogicalEffect::parent_transactions_requested(*transaction, missing.len())
            })
            .collect()
    }
}

/// Dependency consequences of removing a refetchable PreAccepted producer.
/// The producer itself is gone, but unrelated-source children are not: Remote
/// children return to an explicit missing/refetch state, while trusted
/// Proposal/Recovery children re-enter Resolve. This plan is derived from one
/// pre-Apply owner cut and adds no persistent authority state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RefetchableDependencyPlan {
    trusted_requeue: BTreeSet<TxId>,
    remote_missing: BTreeMap<TxId, BTreeSet<CellId>>,
}

impl RefetchableDependencyPlan {
    fn parent_request_effects(&self) -> Option<Vec<LogicalEffect>> {
        OwnerLossPlan::parent_request_effects_for(&self.remote_missing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CapacitySelection {
    Fits { victims: Vec<TxId> },
    CandidateEvicted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EvictionScore {
    fee: u64,
    bytes: u32,
    transaction: TxId,
}

impl EvictionScore {
    fn for_component(authority: &Omega, root: TxId, component: &BTreeSet<TxId>) -> Option<Self> {
        let (fee, bytes) = component
            .iter()
            .try_fold((0u64, 0u32), |(fee, bytes), id| {
                let transaction = &authority.authority.owners.get(id)?.transaction;
                Some((
                    fee.checked_add(transaction.fee)?,
                    bytes.checked_add(transaction.bytes)?,
                ))
            })?;
        Some(Self {
            fee,
            bytes,
            transaction: root,
        })
    }

    fn is_weaker_than(self, other: Self) -> bool {
        let left = u128::from(self.fee) * u128::from(other.bytes);
        let right = u128::from(other.fee) * u128::from(self.bytes);
        left < right || (left == right && self.transaction < other.transaction)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DirectAdmissionEvaluation {
    Duplicate,
    ProposalCollision,
    Membership(MembershipEvaluation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChainTransition {
    /// Exact authority cut against which this move-only receipt was built.
    pub(super) from: ChainView,
    /// The next tip may equal `from.tip`; the new revision still distinguishes
    /// a later ordered chain/proposal-context installation.
    pub(super) to_tip: ViewId,
    pub(super) committed: BTreeSet<TxId>,
    /// Exact chain-validated positive cell facts for this transition. These
    /// are raw chain-layer candidates, not a second authority: Apply still
    /// subtracts cells spent by the same chain move or surviving pool
    /// membership before publishing final dependency availability.
    pub(super) available_cells: BTreeSet<CellId>,
    /// Exact chain-validated positive header facts. Headers have no pool
    /// origin, so no membership-spender subtraction applies.
    pub(super) available_headers: BTreeSet<HeaderId>,
    /// Chain-origin cells and headers invalidated by a detach. Unlike an
    /// attached spend, this is a revalidation/recovery cause rather than a
    /// public conflict rejection.
    pub(super) lost_cells: BTreeSet<CellId>,
    pub(super) lost_headers: BTreeSet<HeaderId>,
    pub(super) conflicting_cells: BTreeSet<CellId>,
    pub(super) recovered: Vec<Transaction>,
    pub(super) proposed: BTreeSet<TxId>,
    pub(super) gap: BTreeSet<TxId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChainRemovalCause {
    Committed,
    ProposalExpired,
    Conflict(CellId),
    Recovery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum KernelCommand {
    Admit(Admission),
    Checkout,
    CheckoutContinuous(VerifyCapability),
    ContinueResolveThenVerify(ResolveContinuation),
    Complete(Completion),
    FinishExecution(Completion),
    SettleFinished(CapabilityId),
    BeginDirect {
        request: DirectRequestId,
        kind: DirectKind,
        transaction: Transaction,
    },
    CompleteDirect(DirectCompletion),
    CancelCapability(CapabilityId),
    CaptureReady {
        limit: usize,
    },
    FinalizeCaptured {
        capture: ReadyCapture,
        wall_time: u64,
    },
    FinalizeNext {
        wall_time: u64,
    },
    Remove {
        transaction: TxId,
    },
    BanPeer {
        peer: PeerId,
        observed_at: MonotonicTick,
    },
    ReconcileChain(ChainTransition),
    ReplaceGeneration {
        view: ViewId,
    },
    ExpireAccepted {
        wall_time: u64,
        residency: u64,
    },
    ExpireRemote {
        wall_time: u64,
        limit: NonZeroU16,
    },
    ClaimEffect,
    SettleEffect(EffectClaim),
}

impl KernelCommand {
    pub(super) fn allowed_during_initialization(&self) -> bool {
        match self {
            Self::Admit(Admission {
                source: RetainedSource::Recovery(_),
                ..
            })
            | Self::Checkout
            | Self::CheckoutContinuous(_)
            | Self::ContinueResolveThenVerify(_)
            | Self::Complete(_)
            | Self::FinishExecution(_)
            | Self::SettleFinished(_)
            | Self::CancelCapability(_)
            | Self::CaptureReady { .. }
            | Self::FinalizeCaptured { .. }
            | Self::FinalizeNext { .. }
            | Self::ReconcileChain(_)
            | Self::ReplaceGeneration { .. }
            | Self::ClaimEffect
            | Self::SettleEffect(_) => true,
            Self::Admit(_)
            | Self::BeginDirect { .. }
            | Self::CompleteDirect(_)
            | Self::Remove { .. }
            | Self::BanPeer { .. }
            | Self::ExpireAccepted { .. }
            | Self::ExpireRemote { .. } => false,
        }
    }

    pub(super) fn allowed_during_drain(&self) -> bool {
        matches!(
            self,
            Self::ContinueResolveThenVerify(_)
                | Self::Complete(_)
                | Self::FinishExecution(_)
                | Self::SettleFinished(_)
                | Self::CompleteDirect(_)
                | Self::CancelCapability(_)
                | Self::ClaimEffect
                | Self::SettleEffect(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum KernelDisposition {
    Retained(TxId),
    Duplicate(TxId),
    Promoted(TxId),
    PayloadVariant(TxId),
    ProposalCollision(TxId),
    StaleRecovery(TxId),
    ResourceRejected(TxId),
    CounterExhausted,
    Idle,
    CheckedOut(WorkCapability),
    ResolveContinued(WorkCapability),
    WorkTransitionRejected(CapabilityId),
    DirectCheckedOut(DirectCapability),
    Continued(TxId),
    Ready(TxId),
    Finished(CapabilityId),
    Waiting(TxId),
    Rejected(TxId),
    InvalidEvidenceRejected(TxId),
    StaleCapabilityRetired(CapabilityId),
    DirectValid(DirectRequestId),
    DirectDuplicate(DirectRequestId),
    DirectRejected(DirectRequestId, DirectNegativeReason),
    DirectResourceExcluded(DirectRequestId),
    DirectRelevantChange(DirectRequestId),
    EffectCapacityWait(TxId),
    PeerEffectCapacityWait(PeerId),
    DirectEffectCapacityWait(DirectRequestId),
    ReadyCaptured(ReadyCapture),
    ReadyCutChanged,
    AcceptedBatch(Vec<TxId>),
    ReplacementAccepted {
        winner: TxId,
        replacement_victims: Vec<TxId>,
        capacity_victims: Vec<TxId>,
        terminal_dependents: Vec<TxId>,
        history_retained: bool,
    },
    CapacityAccepted {
        winner: TxId,
        victims: Vec<TxId>,
        terminal_dependents: Vec<TxId>,
    },
    MembershipRejected(TxId),
    Accepted(TxId),
    Removed(Vec<TxId>),
    PeerBanned {
        peer: PeerId,
        removed: Vec<TxId>,
    },
    PeerRejected {
        transaction: TxId,
        peer: PeerId,
    },
    ChainReconciled {
        removed: Vec<TxId>,
        recovered: Vec<TxId>,
        recovery_excluded: Vec<TxId>,
    },
    StaleChainTransition {
        expected: ChainView,
        actual: ChainView,
    },
    ChainEffectCapacityWait,
    GenerationReplaced {
        removed: Vec<TxId>,
    },
    EffectClaimed(EffectClaim),
    EffectSettled(EffectClaim),
    EffectSuperseded(EffectClaim),
    StaleEffectClaim(EffectClaim),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum KernelStep {
    NoAuthorityCommit(KernelDisposition),
    AuthorityCommit {
        stamp: ApplyStamp,
        disposition: KernelDisposition,
    },
}

impl KernelStep {
    pub(super) fn disposition(&self) -> &KernelDisposition {
        match self {
            Self::NoAuthorityCommit(disposition) | Self::AuthorityCommit { disposition, .. } => {
                disposition
            }
        }
    }
}

impl Omega {
    pub(super) fn kernel_step(&mut self, command: KernelCommand) -> KernelStep {
        match command {
            KernelCommand::Admit(admission) => self.admit(admission),
            KernelCommand::Checkout => self.checkout(CheckoutRoute::Split),
            KernelCommand::CheckoutContinuous(capability) => {
                self.checkout(CheckoutRoute::Continuous(capability))
            }
            KernelCommand::ContinueResolveThenVerify(continuation) => {
                self.continue_resolve_then_verify(continuation)
            }
            KernelCommand::Complete(completion) => self.complete(completion),
            KernelCommand::FinishExecution(completion) => self.finish_execution(completion),
            KernelCommand::SettleFinished(capability) => self.settle_finished(capability),
            KernelCommand::BeginDirect {
                request,
                kind,
                transaction,
            } => self.begin_direct(request, kind, transaction),
            KernelCommand::CompleteDirect(completion) => self.complete_direct(completion),
            KernelCommand::CancelCapability(capability) => self.cancel_capability(capability),
            KernelCommand::CaptureReady { limit } => self.capture_ready(limit),
            KernelCommand::FinalizeCaptured { capture, wall_time } => {
                self.finalize_captured(capture, wall_time)
            }
            KernelCommand::FinalizeNext { wall_time } => self.finalize_next(wall_time),
            KernelCommand::Remove { transaction } => self.remove(transaction),
            KernelCommand::BanPeer { peer, observed_at } => self.ban_peer(peer, observed_at),
            KernelCommand::ReconcileChain(transition) => self.reconcile_chain(transition),
            KernelCommand::ReplaceGeneration { view } => self.replace_generation(view),
            KernelCommand::ExpireAccepted {
                wall_time,
                residency,
            } => self.expire_accepted(wall_time, residency),
            KernelCommand::ExpireRemote { wall_time, limit } => {
                self.expire_remote(RemoteDeadline(wall_time), limit)
            }
            KernelCommand::ClaimEffect => self.claim_effect(),
            KernelCommand::SettleEffect(claim) => self.settle_effect(claim),
        }
    }

    fn admit(&mut self, admission: Admission) -> KernelStep {
        let transaction = admission.transaction;
        if matches!(
            admission.source,
            RetainedSource::Recovery(generation) if generation != self.authority.generation
        ) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleRecovery(transaction.id));
        }
        let source = Source::from(admission.source);
        if let RetainedSource::Remote(residency) = admission.source
            && self.peer_ban_is_active(residency.peer, admission.observed_at)
        {
            let effect = LogicalEffect::IngressReleased(transaction.id);
            if !self.can_append_effects(EffectClass::Remote, std::slice::from_ref(&effect)) {
                return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(
                    transaction.id,
                ));
            }
            let mut next = self.clone();
            let Some(stamp) = next.reserve_apply() else {
                return counter_exhausted();
            };
            if !next.append_effects(EffectClass::Remote, stamp, vec![effect]) {
                return counter_exhausted();
            }
            let transaction = transaction.id;
            *self = next;
            return KernelStep::AuthorityCommit {
                stamp,
                disposition: KernelDisposition::PeerRejected {
                    transaction,
                    peer: residency.peer,
                },
            };
        }
        if let Some(existing) = self.authority.owners.get(&transaction.id) {
            if existing.transaction.witness != transaction.witness {
                return KernelStep::NoAuthorityCommit(KernelDisposition::PayloadVariant(
                    transaction.id,
                ));
            }
            if existing
                .retained_source()
                .is_some_and(|current| source.priority() < current.priority())
            {
                let mut next = self.clone();
                let Some(stamp) = next.reserve_apply() else {
                    return counter_exhausted();
                };
                let Some(owner) = next.authority.owners.get_mut(&transaction.id) else {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::Duplicate(
                        transaction.id,
                    ));
                };
                let OwnerLocation::Retained(retained) = &mut owner.location else {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::Duplicate(
                        transaction.id,
                    ));
                };
                retained.source = match source {
                    Source::Proposal {
                        base: ProposalBase::Trusted,
                    } => Source::Proposal {
                        base: match retained.source {
                            Source::Remote(residency) => ProposalBase::Remote(residency),
                            Source::Recovery(_) | Source::Proposal { .. } => ProposalBase::Trusted,
                        },
                    },
                    source => source,
                };
                if matches!(retained.phase, RetainedPhase::Waiting { .. }) {
                    // Missing-dependency policy changed from refetchable
                    // Remote to trusted Proposal. Preserve the same payload,
                    // owner identity and any active compute lease, but never
                    // retain an old source-dependent Waiting observation.
                    retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
                }
                *self = next;
                return KernelStep::AuthorityCommit {
                    stamp,
                    disposition: KernelDisposition::Promoted(transaction.id),
                };
            }
            return KernelStep::NoAuthorityCommit(KernelDisposition::Duplicate(transaction.id));
        }
        if self
            .authority
            .owners
            .values()
            .any(|owner| owner.transaction.proposal == transaction.proposal)
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ProposalCollision(
                transaction.id,
            ));
        }

        let Some(charge) = transaction.charge() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(
                transaction.id,
            ));
        };
        let Ok(current) = self.owner_usage() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(
                transaction.id,
            ));
        };
        let Ok(retained) = self.retained_usage() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(
                transaction.id,
            ));
        };
        if current
            .checked_add(charge)
            .is_none_or(|usage| !usage.fits(self.authority.limits.owners))
            || retained
                .checked_add(charge)
                .is_none_or(|usage| !usage.fits(self.authority.limits.retained))
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(
                transaction.id,
            ));
        }
        if let RetainedSource::Remote(residency) = admission.source {
            let Ok(peer_usage) = self.remote_peer_usage(residency.peer) else {
                return KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(
                    transaction.id,
                ));
            };
            if peer_usage
                .checked_add(charge)
                .is_none_or(|usage| !usage.fits(self.authority.limits.remote_per_peer))
            {
                return KernelStep::NoAuthorityCommit(KernelDisposition::ResourceRejected(
                    transaction.id,
                ));
            }
        }

        let mut next = self.clone();
        let Some((version, arrival, stamp)) = next.reserve_admission_identity() else {
            return counter_exhausted();
        };
        let id = transaction.id;
        next.authority.owners.insert(
            id,
            Owner {
                version,
                arrival,
                transaction,
                location: OwnerLocation::Retained(RetainedOwner {
                    source,
                    phase: RetainedPhase::Queued(WorkStage::Resolve),
                }),
            },
        );
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::Retained(id),
        }
    }

    fn checkout(&mut self, route: CheckoutRoute) -> KernelStep {
        let retained_worker_slots = self
            .linear
            .work
            .len()
            .checked_add(self.linear.finished_work.len());
        if self.linear.free_compute_permits == 0
            || retained_worker_slots
                .is_none_or(|slots| slots >= usize::from(self.authority.limits.compute_permits))
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        }
        let Some(transaction) = self.queued_order().into_iter().find(|transaction| {
            route == CheckoutRoute::Split
                || self.authority.owners.get(transaction).is_some_and(|owner| {
                    matches!(
                        &owner.location,
                        OwnerLocation::Retained(RetainedOwner {
                            phase: RetainedPhase::Queued(WorkStage::Resolve),
                            ..
                        })
                    )
                })
        }) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let Some(owner) = self.authority.owners.get(&transaction) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let OwnerLocation::Retained(RetainedOwner {
            phase: RetainedPhase::Queued(stage),
            ..
        }) = &owner.location
        else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        if let WorkStage::Verify(evidence) = stage
            && self.classify_evidence(transaction, evidence) != EvidenceValidity::Current
        {
            return self.requeue_stale_work(transaction);
        }
        let stage = stage.clone();
        let permit = match (&stage, route) {
            (WorkStage::Resolve, CheckoutRoute::Continuous(capability)) => {
                WorkPermit::ResolveThenVerify(capability)
            }
            (WorkStage::Resolve, CheckoutRoute::Split) => WorkPermit::ResolveOnly,
            (WorkStage::Verify(_), CheckoutRoute::Split) => {
                WorkPermit::VerifyOnly(VerifyCapability::Any)
            }
            (WorkStage::Verify(_), CheckoutRoute::Continuous(_)) => {
                return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
            }
        };
        let mut next = self.clone();
        let Some(capability_id) = next.reserve_capability() else {
            return counter_exhausted();
        };
        // Checkout creates the active-compute incarnation. Its owner and the
        // sole move-only capability must acquire the same fresh version so a
        // stale capability can never match a later phase of this owner.
        let Some(version) = next.reserve_owner_version() else {
            return counter_exhausted();
        };
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        let Some(free) = next.linear.free_compute_permits.checked_sub(1) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        next.linear.free_compute_permits = free;
        let Some(capability) = WorkCapability::for_checkout(
            capability_id,
            transaction,
            version,
            permit,
            stage,
            next.authority.chain,
            next.authority.rules,
        ) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        next.linear.work.insert(capability_id, capability.clone());
        let Some(owner) = next.authority.owners.get_mut(&transaction) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let OwnerLocation::Retained(retained) = &mut owner.location else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        retained.phase = RetainedPhase::Computing(permit);
        owner.version = version;
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::CheckedOut(capability),
        }
    }

    fn continue_resolve_then_verify(&mut self, continuation: ResolveContinuation) -> KernelStep {
        let Some(capability) = self.linear.work.get(&continuation.capability) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                continuation.capability,
            ));
        };
        if !self.capability_is_current(capability)
            || !matches!(capability.permit(), WorkPermit::ResolveThenVerify(_))
            || !matches!(capability.stage(), WorkStage::Resolve)
            || self.classify_evidence(capability.transaction, &continuation.evidence)
                != EvidenceValidity::Current
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
                continuation.capability,
            ));
        }

        let mut next = self.clone();
        let Some(capability) = next.linear.work.get_mut(&continuation.capability) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                continuation.capability,
            ));
        };
        if !capability.continue_resolve_then_verify(continuation.evidence) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
                continuation.capability,
            ));
        }
        let capability = capability.clone();
        *self = next;
        KernelStep::NoAuthorityCommit(KernelDisposition::ResolveContinued(capability))
    }

    fn begin_direct(
        &mut self,
        request: DirectRequestId,
        kind: DirectKind,
        transaction: Transaction,
    ) -> KernelStep {
        if self
            .linear
            .direct_work
            .values()
            .any(|capability| capability.request == request)
            || self.linear.free_compute_permits == 0
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        }
        let mut next = self.clone();
        let Some(capability_id) = next.reserve_capability() else {
            return counter_exhausted();
        };
        let Some(free) = next.linear.free_compute_permits.checked_sub(1) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        next.linear.free_compute_permits = free;
        let capability = DirectCapability {
            id: capability_id,
            request,
            kind,
            transaction,
            chain: next.authority.chain,
            rules: next.authority.rules,
        };
        next.linear
            .direct_work
            .insert(capability_id, capability.clone());
        *self = next;
        KernelStep::NoAuthorityCommit(KernelDisposition::DirectCheckedOut(capability))
    }

    fn complete_direct(&mut self, completion: DirectCompletion) -> KernelStep {
        let Some(capability) = self.linear.direct_work.get(&completion.capability).cloned() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                completion.capability,
            ));
        };
        if capability.chain != self.authority.chain || capability.rules != self.authority.rules {
            return self.retire_direct(
                completion.capability,
                KernelDisposition::DirectRelevantChange(capability.request),
            );
        }

        match completion.result {
            DirectWorkResult::Rejected(evidence) => {
                if !self.direct_negative_is_current(&capability, &evidence) {
                    return self.retire_direct(
                        completion.capability,
                        KernelDisposition::DirectRelevantChange(capability.request),
                    );
                }
                if capability.kind == DirectKind::TestAccept {
                    return self.retire_direct(
                        completion.capability,
                        KernelDisposition::DirectRejected(capability.request, evidence.reason),
                    );
                }
                let effect = LogicalEffect::validation_rejected(&capability.transaction, None);
                if !self.can_append_effects(EffectClass::Trusted, std::slice::from_ref(&effect)) {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::DirectEffectCapacityWait(capability.request),
                    );
                }
                let mut next = self.clone();
                let Some(stamp) = next.reserve_apply() else {
                    return counter_exhausted();
                };
                if !next.release_direct_capability(completion.capability) {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(completion.capability),
                    );
                }
                if !next.append_effects(EffectClass::Trusted, stamp, vec![effect]) {
                    return counter_exhausted();
                }
                *self = next;
                KernelStep::AuthorityCommit {
                    stamp,
                    disposition: KernelDisposition::DirectRejected(
                        capability.request,
                        evidence.reason,
                    ),
                }
            }
            DirectWorkResult::Verified(evidence) => {
                if self.classify_transaction_evidence(&capability.transaction, &evidence)
                    != EvidenceValidity::Current
                {
                    return self.retire_direct(
                        completion.capability,
                        KernelDisposition::DirectRelevantChange(capability.request),
                    );
                }
                let evaluation = self.evaluate_direct_admission(&capability, &evidence);
                if capability.kind == DirectKind::TestAccept {
                    let disposition = match evaluation {
                        DirectAdmissionEvaluation::Duplicate => {
                            KernelDisposition::DirectDuplicate(capability.request)
                        }
                        DirectAdmissionEvaluation::ProposalCollision => {
                            KernelDisposition::DirectResourceExcluded(capability.request)
                        }
                        DirectAdmissionEvaluation::Membership(MembershipEvaluation::Accepted(
                            _,
                        )) => KernelDisposition::DirectValid(capability.request),
                        DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                            MembershipRejection::Unavailable,
                        )) => KernelDisposition::DirectRelevantChange(capability.request),
                        DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                            MembershipRejection::Policy,
                        )) => KernelDisposition::DirectRejected(
                            capability.request,
                            DirectNegativeReason::Policy,
                        ),
                        DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                            MembershipRejection::Resource | MembershipRejection::CandidateEvicted,
                        )) => KernelDisposition::DirectResourceExcluded(capability.request),
                    };
                    return self.retire_direct(completion.capability, disposition);
                }
                self.finalize_direct_local(
                    capability,
                    completion.capability,
                    completion.wall_time,
                    evidence,
                    evaluation,
                )
            }
        }
    }

    fn cancel_capability(&mut self, capability: CapabilityId) -> KernelStep {
        if self.linear.direct_work.contains_key(&capability) {
            return self.retire_direct(
                capability,
                KernelDisposition::StaleCapabilityRetired(capability),
            );
        }
        if let Some(finished) = self.linear.finished_work.get(&capability) {
            return self
                .cancel_work_capability(finished.capability.clone(), CompletionLocation::Finished);
        }
        let Some(work) = self.linear.work.get(&capability).cloned() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability,
            ));
        };
        self.cancel_work_capability(work, CompletionLocation::Executing)
    }

    fn cancel_work_capability(
        &mut self,
        work: WorkCapability,
        location: CompletionLocation,
    ) -> KernelStep {
        let capability = work.id;
        let current = self.capability_is_current(&work);
        let effect_class = self
            .authority
            .owners
            .get(&work.transaction)
            .and_then(Owner::retained_source)
            .map_or(EffectClass::Trusted, Source::effect_class);
        let owner_loss = if current {
            self.owner_loss_plan(&BTreeSet::from([work.transaction]), &BTreeSet::new())
        } else {
            OwnerLossPlan::default()
        };
        let mut effect_plan = owner_loss
            .terminal
            .iter()
            .filter_map(|id| {
                self.authority.owners.get(id).and_then(|owner| {
                    owner
                        .retained_source()
                        .and_then(Source::ingress_peer)
                        .is_some()
                        .then_some(LogicalEffect::IngressReleased(*id))
                })
            })
            .collect::<Vec<_>>();
        let Some(parent_requests) = owner_loss.parent_request_effects() else {
            return counter_exhausted();
        };
        effect_plan.extend(parent_requests);
        if !self.can_append_effects(effect_class, &effect_plan) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(
                work.transaction,
            ));
        }
        let mut next = self.clone();
        if !next.retire_work_capability(capability, location) {
            return counter_exhausted();
        }
        if current {
            let Some(stamp) = next.reserve_apply() else {
                return counter_exhausted();
            };
            if !next.apply_owner_loss(&owner_loss) {
                return counter_exhausted();
            }
            if !next.append_effects(effect_class, stamp, effect_plan) {
                return counter_exhausted();
            }
            let removed = owner_loss.terminal.iter().copied().collect::<Vec<_>>();
            *self = next;
            return KernelStep::AuthorityCommit {
                stamp,
                disposition: KernelDisposition::Removed(removed),
            };
        }
        *self = next;
        KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(capability))
    }

    fn work_completion_plan(
        &self,
        capability: &WorkCapability,
        result: &WorkResult,
    ) -> Option<WorkCompletionPlan> {
        let permit = self
            .authority
            .owners
            .get(&capability.transaction)
            .and_then(|owner| match &owner.location {
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Computing(permit),
                    ..
                }) if *permit == capability.permit() => Some(*permit),
                OwnerLocation::Retained(_)
                | OwnerLocation::Accepted { .. }
                | OwnerLocation::ReplacementHistory { .. } => None,
            })?;
        Some(match (permit, capability.stage(), result) {
            (WorkPermit::ResolveOnly, WorkStage::Resolve, WorkResult::Resolved(evidence)) => {
                match self.classify_evidence(capability.transaction, evidence) {
                    EvidenceValidity::Current => {
                        WorkCompletionPlan::ContinueVerify(evidence.clone())
                    }
                    EvidenceValidity::RelevantChange => WorkCompletionPlan::RequeueResolve,
                    EvidenceValidity::Invalid => WorkCompletionPlan::Invalid,
                }
            }
            (
                WorkPermit::ResolveThenVerify(verify_capability),
                WorkStage::Resolve,
                WorkResult::Resolved(evidence),
            ) => match self.classify_evidence(capability.transaction, evidence) {
                EvidenceValidity::Current if verify_capability.permits(evidence.verify_class) => {
                    WorkCompletionPlan::PrivateContinuationRequired
                }
                EvidenceValidity::Current => WorkCompletionPlan::ContinueVerify(evidence.clone()),
                EvidenceValidity::RelevantChange => WorkCompletionPlan::RequeueResolve,
                EvidenceValidity::Invalid => WorkCompletionPlan::Invalid,
            },
            (
                WorkPermit::VerifyOnly(_) | WorkPermit::ResolveThenVerify(_),
                WorkStage::Verify(evidence),
                WorkResult::Verified,
            ) => match self.classify_evidence(capability.transaction, evidence) {
                EvidenceValidity::Current => WorkCompletionPlan::Ready(evidence.clone()),
                EvidenceValidity::RelevantChange => WorkCompletionPlan::RequeueResolve,
                EvidenceValidity::Invalid => WorkCompletionPlan::Invalid,
            },
            (
                WorkPermit::ResolveOnly | WorkPermit::ResolveThenVerify(_),
                WorkStage::Resolve,
                WorkResult::Missing(missing),
            ) if self
                .authority
                .owners
                .get(&capability.transaction)
                .is_some_and(|owner| missing.is_for(&owner.transaction)) =>
            {
                self.missing_completion_plan(capability.transaction, missing)
            }
            (_, _, WorkResult::Rejected) => WorkCompletionPlan::Reject,
            _ => WorkCompletionPlan::Invalid,
        })
    }

    fn finish_execution(&mut self, completion: Completion) -> KernelStep {
        let Some(capability) = self.linear.work.get(&completion.capability).cloned() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                completion.capability,
            ));
        };
        if !self.capability_is_current(&capability) {
            let mut next = self.clone();
            next.linear.work.remove(&completion.capability);
            let Some(free) = next.linear.free_compute_permits.checked_add(1) else {
                return counter_exhausted();
            };
            next.linear.free_compute_permits = free;
            *self = next;
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                completion.capability,
            ));
        }
        if matches!(
            self.work_completion_plan(&capability, &completion.result),
            Some(WorkCompletionPlan::PrivateContinuationRequired)
        ) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
                completion.capability,
            ));
        }
        let mut next = self.clone();
        next.linear.work.remove(&completion.capability);
        next.linear.finished_work.insert(
            completion.capability,
            FinishedWorkCapability {
                capability,
                result: completion.result,
            },
        );
        let Some(free) = next.linear.free_compute_permits.checked_add(1) else {
            return counter_exhausted();
        };
        next.linear.free_compute_permits = free;
        *self = next;
        KernelStep::NoAuthorityCommit(KernelDisposition::Finished(completion.capability))
    }

    fn settle_finished(&mut self, capability: CapabilityId) -> KernelStep {
        let Some(finished) = self.linear.finished_work.get(&capability).cloned() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability,
            ));
        };
        self.settle_work_completion(
            finished.capability,
            finished.result,
            CompletionLocation::Finished,
        )
    }

    fn complete(&mut self, completion: Completion) -> KernelStep {
        let Some(capability) = self.linear.work.get(&completion.capability).cloned() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                completion.capability,
            ));
        };
        self.settle_work_completion(capability, completion.result, CompletionLocation::Executing)
    }

    fn settle_work_completion(
        &mut self,
        capability: WorkCapability,
        result: WorkResult,
        location: CompletionLocation,
    ) -> KernelStep {
        if !self.capability_is_current(&capability) {
            let mut next = self.clone();
            if !next.retire_work_capability(capability.id, location) {
                return counter_exhausted();
            }
            *self = next;
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability.id,
            ));
        }
        let Some(effect_class) = self
            .authority
            .owners
            .get(&capability.transaction)
            .and_then(Owner::retained_source)
            .map(Source::effect_class)
        else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability.id,
            ));
        };

        let Some(plan) = self.work_completion_plan(&capability, &result) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability.id,
            ));
        };
        if plan == WorkCompletionPlan::PrivateContinuationRequired {
            return KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
                capability.id,
            ));
        }
        let terminal = matches!(
            plan,
            WorkCompletionPlan::Reject | WorkCompletionPlan::Invalid
        );
        let terminal_loss = if terminal {
            self.owner_loss_plan(&BTreeSet::from([capability.transaction]), &BTreeSet::new())
        } else {
            OwnerLossPlan::default()
        };
        let mut terminal_effects = terminal_loss
            .terminal
            .iter()
            .filter_map(|id| {
                self.authority.owners.get(id).map(|owner| {
                    LogicalEffect::validation_rejected(&owner.transaction, owner.ingress_peer())
                })
            })
            .collect::<Vec<_>>();
        let Some(parent_requests) = terminal_loss.parent_request_effects() else {
            return counter_exhausted();
        };
        terminal_effects.extend(parent_requests);
        let waiting_effects = match (&plan, self.authority.owners.get(&capability.transaction)) {
            (WorkCompletionPlan::Wait(missing), Some(owner))
                if matches!(owner.retained_source(), Some(Source::Remote(_)))
                    && !missing.cells().is_empty() =>
            {
                let Some(effect) = LogicalEffect::parent_transactions_requested(
                    capability.transaction,
                    missing.cells().len(),
                ) else {
                    return counter_exhausted();
                };
                vec![effect]
            }
            _ => Vec::new(),
        };
        if (terminal && !self.can_append_effects(effect_class, &terminal_effects))
            || !self.can_append_effects(effect_class, &waiting_effects)
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(
                capability.transaction,
            ));
        }

        let mut next = self.clone();
        let owner_version = if terminal {
            None
        } else {
            let Some(version) = next.reserve_owner_version() else {
                return counter_exhausted();
            };
            Some(version)
        };
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        if !next.retire_work_capability(capability.id, location) {
            return counter_exhausted();
        }
        let id = capability.transaction;
        let disposition = match plan {
            WorkCompletionPlan::ContinueVerify(evidence) => {
                let Some(owner) = next.authority.owners.get_mut(&id) else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                let OwnerLocation::Retained(retained) = &mut owner.location else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                retained.phase = RetainedPhase::Queued(WorkStage::Verify(evidence));
                KernelDisposition::Continued(id)
            }
            WorkCompletionPlan::Ready(evidence) => {
                let Some(owner) = next.authority.owners.get_mut(&id) else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                let OwnerLocation::Retained(retained) = &mut owner.location else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                retained.phase = RetainedPhase::Ready(evidence);
                KernelDisposition::Ready(id)
            }
            WorkCompletionPlan::RequeueResolve => {
                let Some(owner) = next.authority.owners.get_mut(&id) else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                let OwnerLocation::Retained(retained) = &mut owner.location else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
                KernelDisposition::Continued(id)
            }
            WorkCompletionPlan::Wait(missing) => {
                let Some(owner) = next.authority.owners.get_mut(&id) else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                let OwnerLocation::Retained(retained) = &mut owner.location else {
                    return KernelStep::NoAuthorityCommit(
                        KernelDisposition::StaleCapabilityRetired(capability.id),
                    );
                };
                retained.phase = RetainedPhase::Waiting { missing };
                KernelDisposition::Waiting(id)
            }
            WorkCompletionPlan::Reject => {
                if !next.terminalize_owner_loss(
                    effect_class,
                    stamp,
                    &terminal_loss,
                    terminal_effects,
                ) {
                    return counter_exhausted();
                }
                KernelDisposition::Rejected(id)
            }
            WorkCompletionPlan::Invalid => {
                if !next.terminalize_owner_loss(
                    effect_class,
                    stamp,
                    &terminal_loss,
                    terminal_effects,
                ) {
                    return counter_exhausted();
                }
                KernelDisposition::InvalidEvidenceRejected(id)
            }
            WorkCompletionPlan::PrivateContinuationRequired => {
                return KernelStep::NoAuthorityCommit(KernelDisposition::WorkTransitionRejected(
                    capability.id,
                ));
            }
        };
        if let Some(version) = owner_version {
            let Some(owner) = next.authority.owners.get_mut(&id) else {
                return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                    capability.id,
                ));
            };
            owner.version = version;
        }
        if !waiting_effects.is_empty() && !next.append_effects(effect_class, stamp, waiting_effects)
        {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit { stamp, disposition }
    }

    fn missing_completion_plan(
        &self,
        transaction: TxId,
        missing: &MissingDependencies,
    ) -> WorkCompletionPlan {
        // A positive pool producer appearing after Resolve changes the exact
        // result cut. Re-run Resolve rather than classifying the old negative
        // evidence under either source policy.
        if missing.cells().iter().any(|cell| {
            self.authority.owners.iter().any(|(id, owner)| {
                *id != transaction
                    && matches!(owner.location, OwnerLocation::Accepted { .. })
                    && owner.transaction.outputs.contains(cell)
            })
        }) {
            return WorkCompletionPlan::RequeueResolve;
        }

        let Some(owner) = self.authority.owners.get(&transaction) else {
            return WorkCompletionPlan::Invalid;
        };
        match owner.retained_source() {
            Some(Source::Remote(_)) => WorkCompletionPlan::Wait(missing.clone()),
            Some(Source::Proposal { .. } | Source::Recovery(_)) => {
                // Headers are chain-only and cannot acquire a PreAccepted
                // producer. A trusted cell miss may wait only when the exact
                // pool producer is already retained and can still settle.
                let every_cell_has_preaccepted_producer = missing.cells().iter().all(|cell| {
                    self.authority.owners.iter().any(|(id, owner)| {
                        *id != transaction
                            && matches!(owner.location, OwnerLocation::Retained(_))
                            && owner.transaction.outputs.contains(cell)
                    })
                });
                if missing.headers().is_empty() && every_cell_has_preaccepted_producer {
                    WorkCompletionPlan::Wait(missing.clone())
                } else {
                    WorkCompletionPlan::Reject
                }
            }
            None => WorkCompletionPlan::Invalid,
        }
    }

    fn capture_ready(&self, limit: usize) -> KernelStep {
        let keys = self
            .ready_keys()
            .into_iter()
            .take(limit)
            .collect::<Vec<_>>();
        KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCaptured(ReadyCapture { keys }))
    }

    fn finalize_captured(&mut self, capture: ReadyCapture, wall_time: u64) -> KernelStep {
        let current = self.ready_keys();
        let mut prefix = capture
            .keys
            .iter()
            .zip(&current)
            .take_while(|(captured, current)| captured == current)
            .map(|(key, _)| key.transaction)
            .collect::<Vec<_>>();
        if prefix.is_empty() {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
        }

        // M1 exposes the exact longest common strict-priority prefix. M2 owns
        // the proof that a production cohort compiler may apply that prefix
        // atomically as the same hidden sequential fold.
        if prefix.len() == 1 {
            return self.finalize_transaction(prefix[0], wall_time);
        }
        let Some(effect_class) = prefix
            .first()
            .and_then(|transaction| self.authority.owners.get(transaction))
            .and_then(Owner::retained_source)
            .map(Source::effect_class)
        else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
        };
        prefix.truncate(
            prefix
                .iter()
                .take_while(|transaction| {
                    self.authority
                        .owners
                        .get(transaction)
                        .and_then(Owner::retained_source)
                        .is_some_and(|source| source.effect_class() == effect_class)
                })
                .count(),
        );
        let mut simulated = self.clone();
        let mut accepted = Vec::new();
        for transaction in prefix {
            let Some(owner) = simulated.authority.owners.get(&transaction) else {
                break;
            };
            let OwnerLocation::Retained(RetainedOwner {
                phase: RetainedPhase::Ready(evidence),
                ..
            }) = &owner.location
            else {
                break;
            };
            if !matches!(
                simulated.evaluate_membership_candidate(&owner.transaction, evidence),
                MembershipEvaluation::Accepted(acceptance)
                    if acceptance.is_independent_insert()
            ) {
                break;
            }
            match simulated.finalize_transaction(transaction, wall_time) {
                KernelStep::AuthorityCommit {
                    disposition: KernelDisposition::Accepted(id),
                    ..
                } => accepted.push(id),
                _ => break,
            }
        }
        if accepted.is_empty() {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
        }

        // Recompile the accepted prefix with one Apply stamp. Intermediate
        // Apply stamps produced by the sequential oracle are deliberately not
        // observable in the batched result.
        let mut next = self.clone();
        let effect_plan = accepted
            .iter()
            .filter_map(|transaction| {
                self.authority.owners.get(transaction).map(|owner| {
                    LogicalEffect::admitted(
                        &owner.transaction,
                        AcceptedStatus::Pending,
                        owner.ingress_peer(),
                    )
                })
            })
            .collect::<Vec<_>>();
        if effect_plan.len() != accepted.len()
            || !next.can_append_effects(effect_class, &effect_plan)
        {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(
                accepted[0],
            ));
        }
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        for transaction in &accepted {
            let Some(version) = next.reserve_owner_version() else {
                return counter_exhausted();
            };
            let Some(owner) = next.authority.owners.get_mut(transaction) else {
                return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
            };
            let OwnerLocation::Retained(RetainedOwner {
                source,
                phase: RetainedPhase::Ready(evidence),
            }) = &owner.location
            else {
                return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
            };
            let provenance = source.accepted_provenance();
            let evidence = evidence.clone();
            owner.version = version;
            owner.location = OwnerLocation::Accepted {
                provenance,
                status: AcceptedStatus::Pending,
                accepted_at_wall: wall_time,
                evidence,
            };
        }
        if !next.append_effects(effect_class, stamp, effect_plan) {
            return counter_exhausted();
        }
        if !next.advance_dependency_and_wake() {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::AcceptedBatch(accepted),
        }
    }

    fn finalize_next(&mut self, wall_time: u64) -> KernelStep {
        let Some(transaction) = self.ready_order().first().copied() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        self.finalize_transaction(transaction, wall_time)
    }

    fn finalize_transaction(&mut self, transaction: TxId, wall_time: u64) -> KernelStep {
        let Some(owner) = self.authority.owners.get(&transaction) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let OwnerLocation::Retained(RetainedOwner {
            source,
            phase: RetainedPhase::Ready(evidence),
        }) = &owner.location
        else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        if self.classify_transaction_evidence(&owner.transaction, evidence)
            != EvidenceValidity::Current
        {
            return self.requeue_stale_ready(transaction);
        }
        let provenance = source.accepted_provenance();
        let effect_class = source.effect_class();
        let evidence = evidence.clone();
        let evaluation = self.evaluate_membership_candidate(&owner.transaction, &evidence);
        let rejection_roots = BTreeSet::from([transaction]);
        let rejection_loss = matches!(evaluation, MembershipEvaluation::Rejected(_))
            .then(|| self.owner_loss_plan(&rejection_roots, &BTreeSet::new()));
        let Some(mut effect_plan) = self.membership_effects(&owner.transaction, &evaluation) else {
            return counter_exhausted();
        };
        if let Some(owner_loss) = &rejection_loss {
            let Some(mut dependent_effects) =
                self.unavailable_owner_loss_effects(owner_loss, &rejection_roots)
            else {
                return counter_exhausted();
            };
            effect_plan.append(&mut dependent_effects);
        }
        if !self.can_append_effects(effect_class, &effect_plan) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(
                transaction,
            ));
        }

        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        let disposition = match evaluation {
            MembershipEvaluation::Accepted(acceptance) => {
                let Some(version) = next.reserve_owner_version() else {
                    return counter_exhausted();
                };
                let Some(disposition) = next.apply_membership_acceptance(transaction, &acceptance)
                else {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
                };
                let Some(winner) = next.authority.owners.get_mut(&transaction) else {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
                };
                winner.version = version;
                winner.location = OwnerLocation::Accepted {
                    provenance,
                    status: AcceptedStatus::Pending,
                    accepted_at_wall: wall_time,
                    evidence,
                };
                if !next.adopt_late_children(transaction, &acceptance.late_children) {
                    return counter_exhausted();
                }
                if next
                    .apply_dependency_availability(
                        &acceptance.owner_loss.released_cells,
                        &BTreeSet::new(),
                    )
                    .is_none()
                    || !next.advance_dependency_and_wake()
                {
                    return counter_exhausted();
                }
                disposition
            }
            MembershipEvaluation::Rejected(reason) => {
                let Some(owner_loss) = &rejection_loss else {
                    return counter_exhausted();
                };
                if !next.apply_owner_loss(owner_loss) {
                    return counter_exhausted();
                }
                match reason {
                    MembershipRejection::Unavailable => KernelDisposition::Rejected(transaction),
                    MembershipRejection::Policy => {
                        KernelDisposition::MembershipRejected(transaction)
                    }
                    MembershipRejection::Resource | MembershipRejection::CandidateEvicted => {
                        KernelDisposition::ResourceRejected(transaction)
                    }
                }
            }
        };
        if !next.append_effects(effect_class, stamp, effect_plan) {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit { stamp, disposition }
    }

    /// Evaluate the complete final membership policy without mutating owner,
    /// resource, graph, clock, or effect state. Ready, Local, and TestAccept
    /// all consume this function; only their source-specific wrappers decide
    /// whether the resulting disposition is applied or merely observed.
    fn evaluate_membership_candidate(
        &self,
        candidate: &Transaction,
        evidence: &ResolvedEvidence,
    ) -> MembershipEvaluation {
        let Some(candidate_charge) = candidate.charge() else {
            return MembershipEvaluation::Rejected(MembershipRejection::Resource);
        };
        let candidate_only = BTreeSet::from([candidate.id]);
        let direct_conflicts = candidate
            .inputs
            .iter()
            .filter_map(|cell| self.accepted_spender(*cell))
            .collect::<BTreeSet<_>>();
        let replacement_victims = self.accepted_descendant_closure(&direct_conflicts);
        if !direct_conflicts.is_empty() {
            let replaced_inputs = direct_conflicts
                .iter()
                .filter_map(|conflict| self.authority.owners.get(conflict))
                .flat_map(|owner| owner.transaction.inputs.iter().copied())
                .collect::<BTreeSet<_>>();
            let has_new_unconfirmed_input = evidence.input_origins.iter().any(|(cell, origin)| {
                matches!(origin, InputOrigin::Pool(_)) && !replaced_inputs.contains(cell)
            });
            let depends_on_victim = evidence
                .input_origins
                .values()
                .chain(evidence.dep_origins.values())
                .any(|origin| {
                    matches!(origin, InputOrigin::Pool(parent) if replacement_victims.contains(parent))
                });
            let required_fee = replacement_victims
                .iter()
                .try_fold(0u64, |total, victim| {
                    self.authority
                        .owners
                        .get(victim)
                        .and_then(|owner| total.checked_add(owner.transaction.fee))
                })
                .and_then(|fee| fee.checked_add(candidate.bytes.into()));
            if has_new_unconfirmed_input
                || depends_on_victim
                || required_fee.is_none_or(|required| candidate.fee < required)
            {
                return MembershipEvaluation::Rejected(MembershipRejection::Policy);
            }
        }

        let inputs_available = evidence
            .input_origins
            .iter()
            .all(|(cell, origin)| match origin {
                InputOrigin::Chain => self
                    .accepted_spender(*cell)
                    .is_none_or(|spender| replacement_victims.contains(&spender)),
                InputOrigin::Pool(parent) => {
                    !replacement_victims.contains(parent)
                        && self.pool_parent_produces(*parent, *cell)
                        && self.accepted_spender(*cell).is_none()
                }
            });
        let dependencies_available =
            evidence
                .dep_origins
                .iter()
                .all(|(cell, origin)| match origin {
                    InputOrigin::Chain => true,
                    InputOrigin::Pool(parent) => {
                        !replacement_victims.contains(parent)
                            && self.pool_parent_produces(*parent, *cell)
                    }
                });
        if !inputs_available || !dependencies_available {
            return MembershipEvaluation::Rejected(MembershipRejection::Unavailable);
        }

        let ancestors = self.accepted_ancestor_closure(evidence, &replacement_victims);
        let late_children = self.accepted_children_of_candidate(candidate, &replacement_victims);
        if late_children.iter().any(|child| ancestors.contains(child)) {
            return MembershipEvaluation::Rejected(MembershipRejection::Policy);
        }
        let capacity_victims = match self.select_capacity_victims(
            candidate,
            candidate_charge,
            &replacement_victims,
            &ancestors,
            &late_children,
        ) {
            Some(CapacitySelection::Fits { victims }) => victims,
            Some(CapacitySelection::CandidateEvicted) => {
                return MembershipEvaluation::Rejected(MembershipRejection::CandidateEvicted);
            }
            None => return MembershipEvaluation::Rejected(MembershipRejection::Resource),
        };

        let unavailable_roots = replacement_victims
            .iter()
            .chain(&capacity_victims)
            .copied()
            .collect::<BTreeSet<_>>();
        let owner_loss = self.owner_loss_plan(&unavailable_roots, &candidate.inputs);
        let terminal_dependents = owner_loss.terminal_dependents(&unavailable_roots);

        let mut owner_exclusions = candidate_only.clone();
        owner_exclusions.extend(capacity_victims.iter().copied());
        owner_exclusions.extend(terminal_dependents.iter().copied());
        let owner_with_history_fits = self
            .usage_excluding(&owner_exclusions, |_| true)
            .and_then(|usage| usage.checked_add(candidate_charge))
            .is_some_and(|usage| usage.fits(self.authority.limits.owners));
        let victim_history_charge = replacement_victims.iter().try_fold(
            super::state::ResourceVector::ZERO,
            |usage, victim| {
                self.authority
                    .owners
                    .get(victim)
                    .and_then(|owner| owner.transaction.charge())
                    .and_then(|charge| usage.checked_add(charge))
            },
        );
        let history_fits = self
            .usage_excluding(&candidate_only, |owner| {
                matches!(owner.location, OwnerLocation::ReplacementHistory { .. })
            })
            .zip(victim_history_charge)
            .and_then(|(current, victims)| current.checked_add(victims))
            .is_some_and(|usage| usage.fits(self.authority.limits.replacement_history));
        let history_retained = !replacement_victims.is_empty()
            && history_fits
            && owner_with_history_fits
            && self
                .replacement_history_triggers(candidate, &replacement_victims)
                .is_some();

        if !history_retained {
            owner_exclusions.extend(replacement_victims.iter().copied());
            let final_owner_fits = self
                .usage_excluding(&owner_exclusions, |_| true)
                .and_then(|usage| usage.checked_add(candidate_charge))
                .is_some_and(|usage| usage.fits(self.authority.limits.owners));
            if !final_owner_fits {
                return MembershipEvaluation::Rejected(MembershipRejection::Resource);
            }
        }

        let replacement_history = if history_retained {
            let Some(history) = self.replacement_history_triggers(candidate, &replacement_victims)
            else {
                return MembershipEvaluation::Rejected(MembershipRejection::Resource);
            };
            history
        } else {
            BTreeMap::new()
        };
        let mut replacement_victims = replacement_victims.into_iter().collect::<Vec<_>>();
        replacement_victims.sort_unstable();
        let capacity_set = capacity_victims.iter().copied().collect::<BTreeSet<_>>();
        let mut late_children = late_children
            .into_iter()
            .filter(|child| !capacity_set.contains(child))
            .collect::<Vec<_>>();
        late_children.sort_unstable();
        MembershipEvaluation::Accepted(MembershipAcceptance {
            replacement_victims,
            capacity_victims,
            late_children,
            owner_loss,
            replacement_history,
        })
    }

    fn apply_membership_acceptance(
        &mut self,
        transaction: TxId,
        acceptance: &MembershipAcceptance,
    ) -> Option<KernelDisposition> {
        self.apply_remote_dependency_wait(&acceptance.owner_loss.remote_missing)
            .then_some(())?;
        let roots = acceptance
            .replacement_victims
            .iter()
            .chain(&acceptance.capacity_victims)
            .copied()
            .collect::<BTreeSet<_>>();
        let terminal_dependents = acceptance.owner_loss.terminal_dependents(&roots);
        for dependent in &terminal_dependents {
            self.authority.owners.remove(dependent)?;
        }
        for victim in &acceptance.replacement_victims {
            if let Some(missing) = acceptance.replacement_history.get(victim) {
                let version = self.reserve_owner_version()?;
                let owner = self.authority.owners.get_mut(victim)?;
                owner.version = version;
                owner.location = OwnerLocation::ReplacementHistory {
                    missing: missing.clone(),
                };
            } else {
                self.authority.owners.remove(victim)?;
            }
        }
        for victim in &acceptance.capacity_victims {
            self.authority.owners.remove(victim)?;
        }
        let disposition = if !acceptance.replacement_victims.is_empty() {
            KernelDisposition::ReplacementAccepted {
                winner: transaction,
                replacement_victims: acceptance.replacement_victims.clone(),
                capacity_victims: acceptance.capacity_victims.clone(),
                terminal_dependents,
                history_retained: !acceptance.replacement_history.is_empty(),
            }
        } else if !acceptance.capacity_victims.is_empty() {
            KernelDisposition::CapacityAccepted {
                winner: transaction,
                victims: acceptance.capacity_victims.clone(),
                terminal_dependents,
            }
        } else {
            KernelDisposition::Accepted(transaction)
        };
        Some(disposition)
    }

    fn remove(&mut self, transaction: TxId) -> KernelStep {
        let Some(root_owner) = self.authority.owners.get(&transaction) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Removed(Vec::new()));
        };
        let owner_loss = self.owner_loss_plan(&BTreeSet::from([transaction]), &BTreeSet::new());
        let mut effect_plan = Vec::new();
        if root_owner
            .retained_source()
            .and_then(Source::ingress_peer)
            .is_some()
        {
            effect_plan.push(LogicalEffect::IngressReleased(transaction));
        }
        for id in owner_loss.terminal.iter().filter(|id| **id != transaction) {
            let Some(owner) = self.authority.owners.get(id) else {
                continue;
            };
            if matches!(owner.location, OwnerLocation::Retained(_)) {
                effect_plan.push(LogicalEffect::validation_rejected(
                    &owner.transaction,
                    owner.ingress_peer(),
                ));
            }
        }
        let Some(parent_requests) = owner_loss.parent_request_effects() else {
            return counter_exhausted();
        };
        effect_plan.extend(parent_requests);
        self.commit_removal(EffectClass::Trusted, transaction, owner_loss, effect_plan)
    }

    fn commit_removal(
        &mut self,
        effect_class: EffectClass,
        root: TxId,
        owner_loss: OwnerLossPlan,
        effect_plan: Vec<LogicalEffect>,
    ) -> KernelStep {
        if !self.can_append_effects(effect_class, &effect_plan) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(root));
        }
        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        if !next.apply_owner_loss(&owner_loss) {
            return counter_exhausted();
        }
        let removed = owner_loss.terminal.iter().copied().collect::<Vec<_>>();
        if !next.append_effects(effect_class, stamp, effect_plan) {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::Removed(removed),
        }
    }

    fn ban_peer(&mut self, peer: PeerId, observed_at: MonotonicTick) -> KernelStep {
        let removed = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                (owner.retained_source().and_then(Source::ingress_peer) == Some(peer))
                    .then_some(*id)
            })
            .collect::<Vec<_>>();
        let already_active = self.peer_ban_is_active(peer, observed_at);
        if removed.is_empty() && already_active {
            return KernelStep::NoAuthorityCommit(KernelDisposition::PeerBanned { peer, removed });
        }
        let removed_set = removed.iter().copied().collect::<BTreeSet<_>>();
        let dependency = self.refetchable_dependency_plan(&removed_set);
        let Some(mut parent_requests) = dependency.parent_request_effects() else {
            return counter_exhausted();
        };
        let mut effect_plan = vec![LogicalEffect::PeerCohortRevoked(peer)];
        effect_plan.append(&mut parent_requests);
        if !self.can_append_effects(EffectClass::Critical, &effect_plan) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::PeerEffectCapacityWait(peer));
        }

        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        if !already_active && !next.record_peer_ban(peer, observed_at, stamp) {
            return counter_exhausted();
        }
        if !next.apply_refetchable_dependency_plan(&dependency) {
            return counter_exhausted();
        }
        for id in &removed {
            next.authority.owners.remove(id);
        }
        if !next.append_effects(EffectClass::Critical, stamp, effect_plan) {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::PeerBanned { peer, removed },
        }
    }

    fn reconcile_chain(&mut self, transition: ChainTransition) -> KernelStep {
        if self.authority.chain != transition.from {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleChainTransition {
                expected: transition.from,
                actual: self.authority.chain,
            });
        }
        let Some(next_chain) = transition.from.advance(transition.to_tip) else {
            return counter_exhausted();
        };
        let detached_inputs = transition
            .recovered
            .iter()
            .flat_map(|transaction| transaction.inputs.iter().copied())
            .collect::<BTreeSet<_>>();
        let conflict_roots = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                if transition.committed.contains(id)
                    || matches!(owner.location, OwnerLocation::ReplacementHistory { .. })
                {
                    return None;
                }
                owner
                    .transaction
                    .inputs
                    .iter()
                    .chain(&owner.transaction.deps)
                    .find(|cell| transition.conflicting_cells.contains(cell))
                    .copied()
                    .map(|cell| (*id, cell))
            })
            .collect::<BTreeMap<_, _>>();
        let conflict_closure = self.chain_conflict_closure(&conflict_roots);
        let mut removal_causes = conflict_closure
            .into_iter()
            .map(|(id, cell)| (id, ChainRemovalCause::Conflict(cell)))
            .collect::<BTreeMap<_, _>>();
        let recovery_roots = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                    return None;
                };
                let lost_chain_cell = evidence
                    .input_origins
                    .iter()
                    .chain(&evidence.dep_origins)
                    .any(|(cell, origin)| {
                        *origin == InputOrigin::Chain && transition.lost_cells.contains(cell)
                    });
                let lost_header = owner
                    .transaction
                    .header_deps
                    .iter()
                    .any(|header| transition.lost_headers.contains(header));
                (lost_chain_cell || lost_header).then_some(*id)
            })
            .collect::<BTreeSet<_>>();
        for id in self.accepted_descendant_closure(&recovery_roots) {
            removal_causes
                .entry(id)
                .or_insert(ChainRemovalCause::Recovery);
        }
        removal_causes.extend(
            transition
                .committed
                .iter()
                .copied()
                .filter(|id| self.authority.owners.contains_key(id))
                .map(|id| (id, ChainRemovalCause::Committed)),
        );
        let mut proposal_demotions = BTreeMap::new();
        let mut proposal_expirations = BTreeSet::new();
        for (id, owner) in &self.authority.owners {
            if removal_causes.contains_key(id)
                || !matches!(owner.location, OwnerLocation::Retained(_))
                || transition.proposed.contains(id)
                || transition.gap.contains(id)
            {
                continue;
            }
            let Some(Source::Proposal { base }) = owner.retained_source() else {
                continue;
            };
            match base {
                ProposalBase::Remote(residency) => {
                    proposal_demotions.insert(*id, residency);
                }
                ProposalBase::Trusted => {
                    proposal_expirations.insert(*id);
                }
            }
        }
        removal_causes.extend(
            proposal_expirations
                .iter()
                .copied()
                .map(|id| (id, ChainRemovalCause::ProposalExpired)),
        );
        let removed_set = removal_causes.keys().copied().collect::<BTreeSet<_>>();
        let preaccepted_requeues = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                if removed_set.contains(id) {
                    return None;
                }
                let OwnerLocation::Retained(retained) = &owner.location else {
                    return None;
                };
                let lost_cell = owner
                    .transaction
                    .inputs
                    .iter()
                    .chain(&owner.transaction.deps)
                    .any(|cell| transition.lost_cells.contains(cell));
                let lost_header = owner
                    .transaction
                    .header_deps
                    .iter()
                    .any(|header| transition.lost_headers.contains(header));
                if !(lost_cell || lost_header) {
                    return None;
                }
                let already_missing = match &retained.phase {
                    RetainedPhase::Waiting { missing } => {
                        missing
                            .cells()
                            .iter()
                            .any(|cell| transition.lost_cells.contains(cell))
                            || missing
                                .headers()
                                .iter()
                                .any(|header| transition.lost_headers.contains(header))
                    }
                    RetainedPhase::Queued(WorkStage::Resolve) => true,
                    RetainedPhase::Queued(WorkStage::Verify(_))
                    | RetainedPhase::Computing(_)
                    | RetainedPhase::Ready(_) => false,
                };
                (!already_missing).then_some(*id)
            })
            .collect::<Vec<_>>();

        let status_changes = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                if removed_set.contains(id) {
                    return None;
                }
                let OwnerLocation::Accepted { status, .. } = owner.location else {
                    return None;
                };
                let after = chain_status(*id, &transition.proposed, &transition.gap);
                (status != after).then_some((*id, after))
            })
            .collect::<BTreeMap<_, _>>();

        // Effects are a sealed projection of the same pre-Apply owner cut.
        // Capacity is checked once for the complete indivisible batch; Apply
        // only appends this canonical plan after every other decision succeeds.
        let mut effect_plan = Vec::new();
        for (id, cause) in &removal_causes {
            let Some(owner) = self.authority.owners.get(id) else {
                continue;
            };
            match cause {
                ChainRemovalCause::ProposalExpired => {
                    effect_plan.push(LogicalEffect::IngressReleased(*id));
                }
                ChainRemovalCause::Committed => {
                    if owner
                        .retained_source()
                        .and_then(Source::ingress_peer)
                        .is_some()
                    {
                        effect_plan.push(LogicalEffect::ChainCommitted(*id));
                    }
                }
                ChainRemovalCause::Conflict(cell) => {
                    effect_plan.push(LogicalEffect::chain_conflict(
                        &owner.transaction,
                        *cell,
                        matches!(owner.location, OwnerLocation::Accepted { .. }),
                    ));
                }
                ChainRemovalCause::Recovery => {}
            }
        }
        for (id, status) in &status_changes {
            let Some(owner) = self.authority.owners.get(id) else {
                continue;
            };
            effect_plan.push(LogicalEffect::status_changed(&owner.transaction, *status));
        }
        let proposal_dependency = self.refetchable_dependency_plan(&proposal_expirations);
        let Some(parent_requests) = proposal_dependency.parent_request_effects() else {
            return counter_exhausted();
        };
        effect_plan.extend(parent_requests);
        // Chain detail is rebuildable. Saturation collapses it to the reserved
        // constant-size reset instead of turning a legal chain transition into
        // an effect-capacity wait.
        if !self.can_append_effects(EffectClass::Critical, &effect_plan) {
            effect_plan = vec![LogicalEffect::GenerationReset];
        }

        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        next.authority.chain = next_chain;

        let committed_outputs = transition
            .committed
            .iter()
            .filter_map(|id| self.authority.owners.get(id))
            .flat_map(|owner| owner.transaction.outputs.iter().copied())
            .collect::<BTreeSet<_>>();
        if !next.apply_refetchable_dependency_plan(&proposal_dependency) {
            return counter_exhausted();
        }
        let mut removed = removed_set.iter().copied().collect::<Vec<_>>();
        for id in &removed {
            next.authority.owners.remove(id);
        }
        for id in &preaccepted_requeues {
            let Some((version, arrival)) = next.reserve_owner_identity() else {
                return counter_exhausted();
            };
            let Some(owner) = next.authority.owners.get_mut(id) else {
                return counter_exhausted();
            };
            let OwnerLocation::Retained(retained) = &mut owner.location else {
                return counter_exhausted();
            };
            owner.version = version;
            owner.arrival = arrival;
            retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
        }
        for (id, residency) in &proposal_demotions {
            let Some(owner) = next.authority.owners.get_mut(id) else {
                continue;
            };
            if let OwnerLocation::Retained(retained) = &mut owner.location
                && matches!(retained.source, Source::Proposal { .. })
            {
                retained.source = Source::Remote(*residency);
            }
        }

        // A chain revision invalidates the positive evidence of every current
        // Computing owner, independent of whether its linear capability is
        // still executing or has already released its permit into
        // `finished_work`. Reclassify the semantic owner phase instead of
        // enumerating capability containers; both capability locations remain
        // linearly owned but stale and retire exactly once later. Ready and
        // queued Verify evidence remain resident and are lazily requeued by
        // their exact consumers; Waiting and queued Resolve owners contain no
        // positive chain proof.
        for owner in next.authority.owners.values_mut() {
            if let OwnerLocation::Retained(retained) = &mut owner.location
                && matches!(retained.phase, RetainedPhase::Computing(_))
            {
                retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
            }
        }

        let accepted_changes = next
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                    return None;
                };
                let origin_changes = evidence
                    .input_origins
                    .iter()
                    .chain(&evidence.dep_origins)
                    .any(|(cell, origin)| {
                        matches!(origin, InputOrigin::Pool(parent) if transition.committed.contains(parent))
                            && committed_outputs.contains(cell)
                    });
                (origin_changes || status_changes.contains_key(id)).then_some(*id)
            })
            .collect::<Vec<_>>();
        let current_chain = next.authority.chain;
        for id in accepted_changes {
            let Some(version) = next.reserve_owner_version() else {
                return counter_exhausted();
            };
            let Some(owner) = next.authority.owners.get_mut(&id) else {
                continue;
            };
            let OwnerLocation::Accepted {
                status, evidence, ..
            } = &mut owner.location
            else {
                continue;
            };
            let origin_changed = evidence
                .input_origins
                .iter()
                .chain(&evidence.dep_origins)
                .any(|(cell, origin)| {
                    matches!(origin, InputOrigin::Pool(parent) if transition.committed.contains(parent))
                        && committed_outputs.contains(cell)
                });
            for (cell, origin) in &mut evidence.input_origins {
                if matches!(origin, InputOrigin::Pool(parent) if transition.committed.contains(parent))
                    && committed_outputs.contains(cell)
                {
                    *origin = InputOrigin::Chain;
                }
            }
            for (cell, origin) in &mut evidence.dep_origins {
                if matches!(origin, InputOrigin::Pool(parent) if transition.committed.contains(parent))
                    && committed_outputs.contains(cell)
                {
                    *origin = InputOrigin::Chain;
                }
            }
            if origin_changed {
                evidence.context.chain = current_chain;
            }
            if let Some(after) = status_changes.get(&id) {
                *status = *after;
            }
            owner.version = version;
        }

        let mut recovered = Vec::new();
        let mut recovery_excluded = Vec::new();
        let explicit_recovery_ids = transition
            .recovered
            .iter()
            .map(|transaction| transaction.id)
            .collect::<BTreeSet<_>>();
        let mut recovery_transactions = transition.recovered;
        recovery_transactions.extend(removal_causes.iter().filter_map(|(id, cause)| {
            if !matches!(cause, ChainRemovalCause::Recovery) || explicit_recovery_ids.contains(id) {
                return None;
            }
            self.authority
                .owners
                .get(id)
                .map(|owner| owner.transaction.clone())
        }));
        let (recovered_transactions, cyclic_recovery) =
            canonical_recovery_transactions(recovery_transactions);
        recovery_excluded.extend(cyclic_recovery);
        for (transaction, parents) in recovered_transactions {
            let id = transaction.id;
            if next.authority.owners.contains_key(&id)
                || next
                    .authority
                    .owners
                    .values()
                    .any(|owner| owner.transaction.proposal == transaction.proposal)
                || parents
                    .iter()
                    .any(|parent| !next.authority.owners.contains_key(parent))
            {
                recovery_excluded.push(id);
                continue;
            }
            let Some(charge) = transaction.charge() else {
                recovery_excluded.push(id);
                continue;
            };
            let fits = next
                .owner_usage()
                .ok()
                .and_then(|usage| usage.checked_add(charge))
                .is_some_and(|usage| usage.fits(next.authority.limits.owners))
                && next
                    .retained_usage()
                    .ok()
                    .and_then(|usage| usage.checked_add(charge))
                    .is_some_and(|usage| usage.fits(next.authority.limits.retained));
            if !fits {
                recovery_excluded.push(id);
                continue;
            }
            let Some((version, arrival)) = next.reserve_owner_identity() else {
                return counter_exhausted();
            };
            next.authority.owners.insert(
                id,
                Owner {
                    version,
                    arrival,
                    transaction,
                    location: OwnerLocation::Retained(RetainedOwner {
                        source: Source::Recovery(next.authority.generation),
                        phase: RetainedPhase::Queued(WorkStage::Resolve),
                    }),
                },
            );
            recovered.push(id);
        }

        let recovered_fit = next
            .retained_usage()
            .is_ok_and(|usage| usage.fits(next.authority.limits.retained));
        if !recovered_fit {
            for id in &recovered {
                next.authority.owners.remove(id);
                recovery_excluded.push(*id);
            }
            recovered.clear();
        }
        let available = transition
            .available_cells
            .iter()
            .chain(&committed_outputs)
            .chain(&detached_inputs)
            .copied()
            .filter(|cell| {
                !transition.conflicting_cells.contains(cell)
                    && !transition.lost_cells.contains(cell)
                    && next.accepted_spender(*cell).is_none()
            })
            .collect::<BTreeSet<_>>();
        let available_headers = transition
            .available_headers
            .difference(&transition.lost_headers)
            .copied()
            .collect::<BTreeSet<_>>();
        let Some(history_recovery) =
            next.apply_dependency_availability(&available, &available_headers)
        else {
            return counter_exhausted();
        };
        recovered.extend(history_recovery);
        if !next.append_effects(EffectClass::Critical, stamp, effect_plan) {
            return counter_exhausted();
        }
        removed.sort_unstable();
        removed.dedup();
        recovered.sort_unstable();
        recovery_excluded.sort_unstable();
        recovery_excluded.dedup();
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::ChainReconciled {
                removed,
                recovered,
                recovery_excluded,
            },
        }
    }

    fn replace_generation(&mut self, view: ViewId) -> KernelStep {
        let removed = self.authority.owners.keys().copied().collect::<Vec<_>>();
        let effect = LogicalEffect::GenerationReset;
        if !self.can_append_effects(EffectClass::Critical, std::slice::from_ref(&effect)) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::ChainEffectCapacityWait);
        }
        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        let Some(generation) = next.authority.generation.0.checked_add(1) else {
            return counter_exhausted();
        };
        let Some(chain) = next.authority.chain.advance(view) else {
            return counter_exhausted();
        };
        next.authority.generation = PoolGeneration(generation);
        next.authority.chain = chain;
        next.authority.owners.clear();
        if !next.append_effects(EffectClass::Critical, stamp, vec![effect]) {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::GenerationReplaced { removed },
        }
    }

    fn evaluate_direct_admission(
        &self,
        capability: &DirectCapability,
        evidence: &ResolvedEvidence,
    ) -> DirectAdmissionEvaluation {
        let existing = self.authority.owners.get(&capability.transaction.id);
        let duplicate = match capability.kind {
            DirectKind::TestAccept => existing.is_some(),
            DirectKind::Local => existing
                .is_some_and(|owner| matches!(owner.location, OwnerLocation::Accepted { .. })),
        };
        if duplicate {
            return DirectAdmissionEvaluation::Duplicate;
        }
        if self.authority.owners.iter().any(|(id, owner)| {
            *id != capability.transaction.id
                && owner.transaction.proposal == capability.transaction.proposal
        }) {
            return DirectAdmissionEvaluation::ProposalCollision;
        }
        DirectAdmissionEvaluation::Membership(
            self.evaluate_membership_candidate(&capability.transaction, evidence),
        )
    }

    fn finalize_direct_local(
        &mut self,
        capability: DirectCapability,
        capability_id: CapabilityId,
        wall_time: u64,
        evidence: ResolvedEvidence,
        evaluation: DirectAdmissionEvaluation,
    ) -> KernelStep {
        match &evaluation {
            DirectAdmissionEvaluation::ProposalCollision => {
                return self.retire_direct(
                    capability_id,
                    KernelDisposition::DirectResourceExcluded(capability.request),
                );
            }
            DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                MembershipRejection::Unavailable,
            )) => {
                return self.retire_direct(
                    capability_id,
                    KernelDisposition::DirectRelevantChange(capability.request),
                );
            }
            DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                MembershipRejection::Resource | MembershipRejection::CandidateEvicted,
            )) => {
                return self.retire_direct(
                    capability_id,
                    KernelDisposition::DirectResourceExcluded(capability.request),
                );
            }
            _ => {}
        }
        let Some(effect_plan) = (match &evaluation {
            DirectAdmissionEvaluation::Duplicate => Some(vec![LogicalEffect::accepted_duplicate(
                capability.transaction.id,
                None,
            )]),
            DirectAdmissionEvaluation::ProposalCollision => None,
            DirectAdmissionEvaluation::Membership(evaluation) => {
                self.membership_effects(&capability.transaction, evaluation)
            }
        }) else {
            return counter_exhausted();
        };
        if !self.can_append_effects(EffectClass::Trusted, &effect_plan) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::DirectEffectCapacityWait(
                capability.request,
            ));
        }

        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        let transaction = capability.transaction.id;
        let direct_disposition = match evaluation {
            DirectAdmissionEvaluation::Duplicate => {
                KernelDisposition::DirectDuplicate(capability.request)
            }
            DirectAdmissionEvaluation::ProposalCollision => {
                return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
            }
            DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                MembershipRejection::Policy,
            )) => {
                KernelDisposition::DirectRejected(capability.request, DirectNegativeReason::Policy)
            }
            DirectAdmissionEvaluation::Membership(MembershipEvaluation::Rejected(
                MembershipRejection::Unavailable
                | MembershipRejection::Resource
                | MembershipRejection::CandidateEvicted,
            )) => return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged),
            DirectAdmissionEvaluation::Membership(MembershipEvaluation::Accepted(acceptance)) => {
                let existing = next.authority.owners.get(&transaction);
                let arrival = existing.map(|owner| owner.arrival);
                let provenance = existing
                    .and_then(Owner::ingress_peer)
                    .map_or(AcceptedProvenance::Trusted, AcceptedProvenance::Peer);
                let Some(version) = next.reserve_owner_version() else {
                    return counter_exhausted();
                };
                let arrival = match arrival {
                    Some(arrival) => arrival,
                    None => {
                        let Some(arrival) = next.reserve_arrival() else {
                            return counter_exhausted();
                        };
                        arrival
                    }
                };
                if next
                    .apply_membership_acceptance(transaction, &acceptance)
                    .is_none()
                {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::ReadyCutChanged);
                }
                next.authority.owners.insert(
                    transaction,
                    Owner {
                        version,
                        arrival,
                        transaction: capability.transaction,
                        location: OwnerLocation::Accepted {
                            provenance,
                            status: AcceptedStatus::Pending,
                            accepted_at_wall: wall_time,
                            evidence,
                        },
                    },
                );
                if !next.adopt_late_children(transaction, &acceptance.late_children) {
                    return counter_exhausted();
                }
                if next
                    .apply_dependency_availability(
                        &acceptance.owner_loss.released_cells,
                        &BTreeSet::new(),
                    )
                    .is_none()
                    || !next.advance_dependency_and_wake()
                {
                    return counter_exhausted();
                }
                KernelDisposition::DirectValid(capability.request)
            }
        };
        if !next.append_effects(EffectClass::Trusted, stamp, effect_plan) {
            return counter_exhausted();
        }
        if !next.release_direct_capability(capability_id) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability_id,
            ));
        }
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: direct_disposition,
        }
    }

    fn expire_remote(&mut self, cutoff: RemoteDeadline, limit: NonZeroU16) -> KernelStep {
        let due = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                let deadline = owner.retained_source()?.active_remote_deadline()?;
                (deadline <= cutoff).then_some((deadline, *id))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(usize::from(limit.get()))
            .map(|(_, id)| id)
            .collect::<Vec<_>>();
        if due.is_empty() {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        }
        let due_set = due.iter().copied().collect::<BTreeSet<_>>();
        let dependency = self.refetchable_dependency_plan(&due_set);
        let mut effect_plan = due
            .iter()
            .copied()
            .map(LogicalEffect::RemoteExpired)
            .collect::<Vec<_>>();
        let Some(parent_requests) = dependency.parent_request_effects() else {
            return counter_exhausted();
        };
        effect_plan.extend(parent_requests);
        if !self.can_append_effects(EffectClass::Remote, &effect_plan) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectCapacityWait(due[0]));
        }

        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        if !next.apply_refetchable_dependency_plan(&dependency) {
            return counter_exhausted();
        }
        for id in &due {
            next.authority.owners.remove(id);
        }
        if !next.append_effects(EffectClass::Remote, stamp, effect_plan) {
            return counter_exhausted();
        }
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::Removed(due),
        }
    }

    fn expire_accepted(&mut self, wall_time: u64, residency: u64) -> KernelStep {
        let Some(cutoff) = wall_time.checked_sub(residency) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let candidate = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| match owner.location {
                OwnerLocation::Accepted {
                    accepted_at_wall, ..
                } if accepted_at_wall <= cutoff => Some((accepted_at_wall, *id)),
                _ => None,
            })
            .min()
            .map(|(_, id)| id);
        candidate.map_or_else(
            || KernelStep::NoAuthorityCommit(KernelDisposition::Idle),
            |id| self.expire_accepted_root(id),
        )
    }

    fn expire_accepted_root(&mut self, root: TxId) -> KernelStep {
        let owner_loss = self.owner_loss_plan(&BTreeSet::from([root]), &BTreeSet::new());
        let mut effect_plan = owner_loss
            .terminal
            .iter()
            .filter_map(|id| {
                self.authority.owners.get(id).map(|owner| {
                    if matches!(owner.location, OwnerLocation::Accepted { .. }) {
                        LogicalEffect::expired(&owner.transaction)
                    } else {
                        LogicalEffect::validation_rejected(&owner.transaction, owner.ingress_peer())
                    }
                })
            })
            .collect::<Vec<_>>();
        let Some(parent_requests) = owner_loss.parent_request_effects() else {
            return counter_exhausted();
        };
        effect_plan.extend(parent_requests);
        self.commit_removal(EffectClass::Critical, root, owner_loss, effect_plan)
    }

    fn claim_effect(&mut self) -> KernelStep {
        if let Some(claim) = self.linear.effect_claim {
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim));
        }
        let Some((source, effect)) = self.next_effect_record() else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let claim = EffectClaim {
            source,
            stamp: effect.stamp,
            ordinal: effect.ordinal,
        };
        let mut next = self.clone();
        next.linear.effect_claim = Some(claim);
        *self = next;
        KernelStep::NoAuthorityCommit(KernelDisposition::EffectClaimed(claim))
    }

    fn settle_effect(&mut self, claim: EffectClaim) -> KernelStep {
        if self.linear.effect_claim != Some(claim) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleEffectClaim(claim));
        }
        let mut next = self.clone();
        if claim.source == EffectClaimSource::GenerationReset
            && next
                .authority
                .latest_generation_reset
                .as_ref()
                .is_some_and(|reset| reset.stamp > claim.stamp)
        {
            next.linear.effect_claim = None;
            *self = next;
            return KernelStep::NoAuthorityCommit(KernelDisposition::EffectSuperseded(claim));
        }
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        match claim.source {
            EffectClaimSource::Queued => {
                let Some(effect) = next.authority.effects.front() else {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::StaleEffectClaim(
                        claim,
                    ));
                };
                if (effect.stamp, effect.ordinal) != (claim.stamp, claim.ordinal) {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::StaleEffectClaim(
                        claim,
                    ));
                }
                next.authority.effects.pop_front();
            }
            EffectClaimSource::GenerationReset => {
                let Some(reset) = next.authority.latest_generation_reset.as_ref() else {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::StaleEffectClaim(
                        claim,
                    ));
                };
                if (reset.stamp, reset.ordinal) != (claim.stamp, claim.ordinal) {
                    return KernelStep::NoAuthorityCommit(KernelDisposition::StaleEffectClaim(
                        claim,
                    ));
                }
                next.authority.latest_generation_reset = None;
            }
        }
        next.linear.effect_claim = None;
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::EffectSettled(claim),
        }
    }

    fn requeue_stale_ready(&mut self, transaction: TxId) -> KernelStep {
        self.requeue_stale_work(transaction)
    }

    fn requeue_stale_work(&mut self, transaction: TxId) -> KernelStep {
        let mut next = self.clone();
        let Some(stamp) = next.reserve_apply() else {
            return counter_exhausted();
        };
        let Some(owner) = next.authority.owners.get_mut(&transaction) else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        let OwnerLocation::Retained(retained) = &mut owner.location else {
            return KernelStep::NoAuthorityCommit(KernelDisposition::Idle);
        };
        retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
        *self = next;
        KernelStep::AuthorityCommit {
            stamp,
            disposition: KernelDisposition::Continued(transaction),
        }
    }

    pub(super) fn capture_direct_negative(
        &self,
        transaction: &Transaction,
        reason: DirectNegativeReason,
    ) -> DirectNegativeEvidence {
        DirectNegativeEvidence {
            chain: self.authority.chain,
            rules: self.authority.rules,
            witness: transaction.witness,
            reason,
            reads: self.observe_cells(
                transaction
                    .inputs
                    .iter()
                    .chain(transaction.deps.iter())
                    .copied(),
            ),
        }
    }

    fn direct_negative_is_current(
        &self,
        capability: &DirectCapability,
        evidence: &DirectNegativeEvidence,
    ) -> bool {
        evidence.chain == self.authority.chain
            && evidence.rules == self.authority.rules
            && evidence.witness == capability.transaction.witness
            && evidence.reads
                == self.observe_cells(
                    capability
                        .transaction
                        .inputs
                        .iter()
                        .chain(capability.transaction.deps.iter())
                        .copied(),
                )
    }

    fn observe_cells(
        &self,
        cells: impl IntoIterator<Item = CellId>,
    ) -> BTreeMap<CellId, CellObservation> {
        cells
            .into_iter()
            .map(|cell| {
                let producer = self.authority.owners.iter().find_map(|(id, owner)| {
                    (matches!(owner.location, OwnerLocation::Accepted { .. })
                        && owner.transaction.outputs.contains(&cell))
                    .then_some((*id, owner.version))
                });
                let spender = self.authority.owners.iter().find_map(|(id, owner)| {
                    (matches!(owner.location, OwnerLocation::Accepted { .. })
                        && owner.transaction.inputs.contains(&cell))
                    .then_some((*id, owner.version))
                });
                (cell, CellObservation { producer, spender })
            })
            .collect()
    }

    fn retire_direct(
        &mut self,
        capability: CapabilityId,
        disposition: KernelDisposition,
    ) -> KernelStep {
        let mut next = self.clone();
        if !next.release_direct_capability(capability) {
            return KernelStep::NoAuthorityCommit(KernelDisposition::StaleCapabilityRetired(
                capability,
            ));
        }
        *self = next;
        KernelStep::NoAuthorityCommit(disposition)
    }

    fn retire_work_capability(
        &mut self,
        capability: CapabilityId,
        location: CompletionLocation,
    ) -> bool {
        match location {
            CompletionLocation::Executing => {
                if self.linear.work.remove(&capability).is_none() {
                    return false;
                }
                let Some(free) = self.linear.free_compute_permits.checked_add(1) else {
                    return false;
                };
                self.linear.free_compute_permits = free;
            }
            CompletionLocation::Finished => {
                if self.linear.finished_work.remove(&capability).is_none() {
                    return false;
                }
            }
        }
        true
    }

    fn release_direct_capability(&mut self, capability: CapabilityId) -> bool {
        if self.linear.direct_work.remove(&capability).is_none() {
            return false;
        }
        let Some(free) = self.linear.free_compute_permits.checked_add(1) else {
            return false;
        };
        self.linear.free_compute_permits = free;
        true
    }

    fn accepted_spender(&self, cell: super::state::CellId) -> Option<TxId> {
        self.authority.owners.iter().find_map(|(id, owner)| {
            matches!(owner.location, OwnerLocation::Accepted { .. })
                .then(|| owner.transaction.inputs.contains(&cell).then_some(*id))
                .flatten()
        })
    }

    fn pool_parent_produces(&self, parent: TxId, cell: CellId) -> bool {
        self.authority.owners.get(&parent).is_some_and(|owner| {
            matches!(owner.location, OwnerLocation::Accepted { .. })
                && owner.transaction.outputs.contains(&cell)
        })
    }

    fn usage_excluding(
        &self,
        excluded: &BTreeSet<TxId>,
        include: impl Fn(&Owner) -> bool,
    ) -> Option<super::state::ResourceVector> {
        self.authority
            .owners
            .iter()
            .filter(|(id, owner)| !excluded.contains(id) && include(owner))
            .try_fold(super::state::ResourceVector::ZERO, |usage, (_, owner)| {
                owner
                    .transaction
                    .charge()
                    .and_then(|charge| usage.checked_add(charge))
            })
    }

    fn ready_keys(&self) -> Vec<ReadyKey> {
        self.ready_order()
            .into_iter()
            .filter_map(|transaction| {
                self.authority.owners.get(&transaction).and_then(|owner| {
                    Some(ReadyKey {
                        source_priority: owner.retained_source()?.priority(),
                        fee: owner.transaction.fee,
                        serialized_bytes: owner.transaction.bytes,
                        arrival: owner.arrival,
                        transaction,
                        version: owner.version,
                    })
                })
            })
            .collect()
    }

    fn accepted_ancestor_closure(
        &self,
        evidence: &ResolvedEvidence,
        excluded: &BTreeSet<TxId>,
    ) -> BTreeSet<TxId> {
        let mut ancestors = evidence
            .input_origins
            .values()
            .chain(evidence.dep_origins.values())
            .filter_map(|origin| match origin {
                InputOrigin::Pool(parent) if !excluded.contains(parent) => Some(*parent),
                InputOrigin::Chain | InputOrigin::Pool(_) => None,
            })
            .collect::<BTreeSet<_>>();
        loop {
            let before = ancestors.len();
            for ancestor in ancestors.clone() {
                let Some(OwnerLocation::Accepted { evidence, .. }) = self
                    .authority
                    .owners
                    .get(&ancestor)
                    .map(|owner| &owner.location)
                else {
                    continue;
                };
                ancestors.extend(
                    evidence
                        .input_origins
                        .values()
                        .chain(evidence.dep_origins.values())
                        .filter_map(|origin| match origin {
                            InputOrigin::Pool(parent) if !excluded.contains(parent) => {
                                Some(*parent)
                            }
                            InputOrigin::Chain | InputOrigin::Pool(_) => None,
                        }),
                );
            }
            if ancestors.len() == before {
                return ancestors;
            }
        }
    }

    fn accepted_children_of_candidate(
        &self,
        candidate: &Transaction,
        excluded: &BTreeSet<TxId>,
    ) -> BTreeSet<TxId> {
        self.authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                (!excluded.contains(id)
                    && matches!(owner.location, OwnerLocation::Accepted { .. })
                    && owner
                        .transaction
                        .inputs
                        .iter()
                        .chain(&owner.transaction.deps)
                        .any(|cell| candidate.outputs.contains(cell)))
                .then_some(*id)
            })
            .collect()
    }

    fn replacement_history_triggers(
        &self,
        candidate: &Transaction,
        victims: &BTreeSet<TxId>,
    ) -> Option<BTreeMap<TxId, MissingDependencies>> {
        victims
            .iter()
            .map(|victim| {
                let owner = self.authority.owners.get(victim)?;
                let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                    return None;
                };
                let missing = evidence
                    .input_origins
                    .iter()
                    .chain(&evidence.dep_origins)
                    .filter_map(|(cell, origin)| {
                        (candidate.inputs.contains(cell)
                            || matches!(origin, InputOrigin::Pool(parent) if victims.contains(parent)))
                        .then_some(*cell)
                    })
                    .collect::<BTreeSet<_>>();
                Some((
                    *victim,
                    MissingDependencies::for_transaction(&owner.transaction, missing)?,
                ))
            })
            .collect()
    }

    fn select_capacity_victims(
        &self,
        candidate: &Transaction,
        candidate_charge: super::state::ResourceVector,
        mandatory: &BTreeSet<TxId>,
        protected_ancestors: &BTreeSet<TxId>,
        late_children: &BTreeSet<TxId>,
    ) -> Option<CapacitySelection> {
        let mut removed = mandatory.clone();
        let mut capacity_victims = BTreeSet::new();
        loop {
            let mut exclusions = removed.clone();
            exclusions.insert(candidate.id);
            let projected = self
                .usage_excluding(&exclusions, |owner| {
                    matches!(owner.location, OwnerLocation::Accepted { .. })
                })?
                .checked_add(candidate_charge)?;
            if projected.fits(self.authority.limits.accepted) {
                return Some(CapacitySelection::Fits {
                    victims: capacity_victims.into_iter().collect(),
                });
            }

            let candidate_descendants = self
                .accepted_descendant_closure_excluding(late_children, &removed)
                .into_iter()
                .filter(|id| !removed.contains(id))
                .collect::<BTreeSet<_>>();
            let (candidate_fee, candidate_bytes) = candidate_descendants.iter().try_fold(
                (candidate.fee, candidate.bytes),
                |(fee, bytes), id| {
                    let transaction = &self.authority.owners.get(id)?.transaction;
                    Some((
                        fee.checked_add(transaction.fee)?,
                        bytes.checked_add(transaction.bytes)?,
                    ))
                },
            )?;
            let mut weakest = (
                true,
                EvictionScore {
                    fee: candidate_fee,
                    bytes: candidate_bytes,
                    transaction: candidate.id,
                },
                BTreeSet::new(),
            );

            for (id, owner) in &self.authority.owners {
                if removed.contains(id)
                    || protected_ancestors.contains(id)
                    || !matches!(owner.location, OwnerLocation::Accepted { .. })
                {
                    continue;
                }
                let component =
                    self.accepted_descendant_closure_excluding(&BTreeSet::from([*id]), &removed);
                let score = EvictionScore::for_component(self, *id, &component)?;
                if score.is_weaker_than(weakest.1) {
                    weakest = (false, score, component);
                }
            }

            if weakest.0 {
                return Some(CapacitySelection::CandidateEvicted);
            }
            if weakest.2.is_empty() {
                return None;
            }
            removed.extend(weakest.2.iter().copied());
            capacity_victims.extend(weakest.2);
        }
    }

    fn adopt_late_children(&mut self, parent: TxId, children: &[TxId]) -> bool {
        let Some(outputs) = self
            .authority
            .owners
            .get(&parent)
            .map(|owner| owner.transaction.outputs.clone())
        else {
            return false;
        };
        for child in children {
            let Some(version) = self.reserve_owner_version() else {
                return false;
            };
            let Some(owner) = self.authority.owners.get_mut(child) else {
                return false;
            };
            let OwnerLocation::Accepted { evidence, .. } = &mut owner.location else {
                return false;
            };
            for (cell, origin) in evidence
                .input_origins
                .iter_mut()
                .chain(evidence.dep_origins.iter_mut())
            {
                if outputs.contains(cell) {
                    *origin = InputOrigin::Pool(parent);
                }
            }
            owner.version = version;
        }
        true
    }

    fn accepted_descendant_closure(&self, roots: &BTreeSet<TxId>) -> BTreeSet<TxId> {
        self.accepted_descendant_closure_excluding(roots, &BTreeSet::new())
    }

    fn accepted_descendant_closure_excluding(
        &self,
        roots: &BTreeSet<TxId>,
        excluded: &BTreeSet<TxId>,
    ) -> BTreeSet<TxId> {
        let mut closure = roots.difference(excluded).copied().collect::<BTreeSet<_>>();
        loop {
            let before = closure.len();
            for (id, owner) in &self.authority.owners {
                if excluded.contains(id) {
                    continue;
                }
                let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                    continue;
                };
                if evidence
                    .input_origins
                    .values()
                    .chain(evidence.dep_origins.values())
                    .any(|origin| {
                        matches!(origin, InputOrigin::Pool(parent) if closure.contains(parent))
                    })
                {
                    closure.insert(*id);
                }
            }
            if closure.len() == before {
                return closure;
            }
        }
    }

    fn chain_conflict_closure(&self, roots: &BTreeMap<TxId, CellId>) -> BTreeMap<TxId, CellId> {
        let mut closure = roots.clone();
        loop {
            let before = closure.len();
            let lost_outputs = closure
                .iter()
                .filter_map(|(id, cause)| {
                    self.authority.owners.get(id).map(|owner| (owner, *cause))
                })
                .flat_map(|(owner, cause)| {
                    owner
                        .transaction
                        .outputs
                        .iter()
                        .copied()
                        .map(move |output| (output, cause))
                })
                .collect::<BTreeMap<_, _>>();
            for (id, owner) in &self.authority.owners {
                if closure.contains_key(id)
                    || matches!(owner.location, OwnerLocation::ReplacementHistory { .. })
                {
                    continue;
                }
                if let Some(cause) = owner
                    .transaction
                    .inputs
                    .iter()
                    .chain(&owner.transaction.deps)
                    .filter_map(|cell| lost_outputs.get(cell))
                    .min()
                    .copied()
                {
                    closure.insert(*id, cause);
                }
            }
            if closure.len() == before {
                return closure;
            }
        }
    }

    fn owner_loss_plan(
        &self,
        roots: &BTreeSet<TxId>,
        newly_spent: &BTreeSet<CellId>,
    ) -> OwnerLossPlan {
        let mut terminal = roots
            .iter()
            .filter(|id| self.authority.owners.contains_key(id))
            .copied()
            .collect::<BTreeSet<_>>();
        loop {
            let before = terminal.len();
            let lost_outputs = self.outputs_of(&terminal);
            for (id, owner) in &self.authority.owners {
                if terminal.contains(id)
                    || matches!(owner.location, OwnerLocation::ReplacementHistory { .. })
                {
                    continue;
                }
                let terminal_on_loss = matches!(owner.location, OwnerLocation::Accepted { .. })
                    || matches!(
                        owner.location,
                        OwnerLocation::Retained(RetainedOwner {
                            source: Source::Recovery(_) | Source::Proposal { .. },
                            ..
                        })
                    );
                if terminal_on_loss
                    && owner
                        .transaction
                        .inputs
                        .iter()
                        .chain(&owner.transaction.deps)
                        .any(|cell| lost_outputs.contains(cell))
                {
                    terminal.insert(*id);
                }
            }
            if terminal.len() == before {
                break;
            }
        }
        let remote_missing = self.remote_dependency_wait_plan(&terminal);
        let released_cells = terminal
            .iter()
            .filter_map(|id| self.authority.owners.get(id))
            .filter_map(|owner| match &owner.location {
                OwnerLocation::Accepted { evidence, .. } => Some(evidence),
                OwnerLocation::Retained(_) | OwnerLocation::ReplacementHistory { .. } => None,
            })
            .flat_map(|evidence| evidence.input_origins.iter())
            .filter_map(|(cell, origin)| match origin {
                InputOrigin::Chain => Some(*cell),
                InputOrigin::Pool(parent) if !terminal.contains(parent) => Some(*cell),
                InputOrigin::Pool(_) => None,
            })
            .filter(|cell| !newly_spent.contains(cell))
            .collect::<BTreeSet<_>>();
        OwnerLossPlan {
            terminal,
            remote_missing,
            released_cells,
        }
    }

    fn outputs_of(&self, owners: &BTreeSet<TxId>) -> BTreeSet<CellId> {
        owners
            .iter()
            .filter_map(|id| self.authority.owners.get(id))
            .flat_map(|owner| owner.transaction.outputs.iter().copied())
            .collect()
    }

    fn remote_dependency_wait_plan(
        &self,
        unavailable: &BTreeSet<TxId>,
    ) -> BTreeMap<TxId, BTreeSet<CellId>> {
        let lost_outputs = self.outputs_of(unavailable);
        self.authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                if unavailable.contains(id)
                    || !matches!(
                        owner.location,
                        OwnerLocation::Retained(RetainedOwner {
                            source: Source::Remote(_),
                            ..
                        })
                    )
                {
                    return None;
                }
                let missing = owner
                    .transaction
                    .inputs
                    .iter()
                    .chain(&owner.transaction.deps)
                    .filter(|cell| lost_outputs.contains(cell))
                    .copied()
                    .collect::<BTreeSet<_>>();
                (!missing.is_empty()).then_some((*id, missing))
            })
            .collect()
    }

    fn refetchable_dependency_plan(&self, removed: &BTreeSet<TxId>) -> RefetchableDependencyPlan {
        let lost_outputs = self.outputs_of(removed);
        let trusted_requeue = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| {
                if removed.contains(id) {
                    return None;
                }
                let OwnerLocation::Retained(retained) = &owner.location else {
                    return None;
                };
                if matches!(retained.source, Source::Remote(_))
                    || matches!(retained.phase, RetainedPhase::Queued(WorkStage::Resolve))
                {
                    return None;
                }
                owner
                    .transaction
                    .inputs
                    .iter()
                    .chain(&owner.transaction.deps)
                    .any(|cell| lost_outputs.contains(cell))
                    .then_some(*id)
            })
            .collect();
        RefetchableDependencyPlan {
            trusted_requeue,
            remote_missing: self.remote_dependency_wait_plan(removed),
        }
    }

    fn apply_remote_dependency_wait(&mut self, waiting: &BTreeMap<TxId, BTreeSet<CellId>>) -> bool {
        for (id, newly_missing) in waiting {
            let computing = self.authority.owners.get(id).is_some_and(|owner| {
                matches!(
                    owner.location,
                    OwnerLocation::Retained(RetainedOwner {
                        phase: RetainedPhase::Computing(_),
                        ..
                    })
                )
            });
            let version = if computing {
                let Some(version) = self.reserve_owner_version() else {
                    return false;
                };
                Some(version)
            } else {
                None
            };
            let Some(owner) = self.authority.owners.get_mut(id) else {
                return false;
            };
            let Owner {
                transaction,
                location,
                ..
            } = owner;
            let OwnerLocation::Retained(retained) = location else {
                return false;
            };
            match &mut retained.phase {
                RetainedPhase::Waiting { missing } => {
                    if !missing.extend(transaction, newly_missing) {
                        return false;
                    }
                }
                RetainedPhase::Queued(_)
                | RetainedPhase::Computing(_)
                | RetainedPhase::Ready(_) => {
                    let Some(missing) =
                        MissingDependencies::for_transaction(transaction, newly_missing.clone())
                    else {
                        return false;
                    };
                    retained.phase = RetainedPhase::Waiting { missing };
                }
            }
            if let Some(version) = version {
                owner.version = version;
            }
        }
        true
    }

    fn apply_refetchable_dependency_plan(
        &mut self,
        dependency: &RefetchableDependencyPlan,
    ) -> bool {
        if !self.apply_remote_dependency_wait(&dependency.remote_missing) {
            return false;
        }
        for id in &dependency.trusted_requeue {
            let Some(version) = self.reserve_owner_version() else {
                return false;
            };
            let Some(owner) = self.authority.owners.get_mut(id) else {
                return false;
            };
            let OwnerLocation::Retained(retained) = &mut owner.location else {
                return false;
            };
            if matches!(retained.source, Source::Remote(_)) {
                return false;
            }
            retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
            owner.version = version;
        }
        true
    }

    fn apply_owner_loss(&mut self, owner_loss: &OwnerLossPlan) -> bool {
        if !self.apply_remote_dependency_wait(&owner_loss.remote_missing) {
            return false;
        }
        for id in &owner_loss.terminal {
            self.authority.owners.remove(id);
        }
        self.apply_dependency_availability(&owner_loss.released_cells, &BTreeSet::new())
            .is_some()
    }

    fn terminalize_owner_loss(
        &mut self,
        effect_class: EffectClass,
        stamp: ApplyStamp,
        owner_loss: &OwnerLossPlan,
        effects: Vec<LogicalEffect>,
    ) -> bool {
        if !self.apply_owner_loss(owner_loss) {
            return false;
        }
        self.append_effects(effect_class, stamp, effects)
    }

    fn apply_dependency_availability(
        &mut self,
        available_cells: &BTreeSet<CellId>,
        available_headers: &BTreeSet<HeaderId>,
    ) -> Option<Vec<TxId>> {
        if available_cells.is_empty() && available_headers.is_empty() {
            return Some(Vec::new());
        }
        // Waiting and ReplacementHistory are inert owner locations: no work
        // capability or externally reusable receipt can name their internal
        // blocker set. Shrinking that set therefore preserves the owner
        // incarnation. Recovery, which creates executable work again, receives
        // a fresh version and arrival below.
        for owner in self.authority.owners.values_mut() {
            let OwnerLocation::Retained(RetainedOwner {
                phase: RetainedPhase::Waiting { missing, .. },
                ..
            }) = &mut owner.location
            else {
                continue;
            };
            missing.retain_unavailable(available_cells);
            missing.retain_unavailable_headers(available_headers);
            if missing.is_empty() {
                let OwnerLocation::Retained(retained) = &mut owner.location else {
                    return None;
                };
                retained.phase = RetainedPhase::Queued(WorkStage::Resolve);
            }
        }

        let history = self
            .authority
            .owners
            .iter()
            .filter_map(|(id, owner)| match &owner.location {
                OwnerLocation::ReplacementHistory { missing } => {
                    let mut remaining = missing.clone();
                    remaining.retain_unavailable(available_cells);
                    remaining.retain_unavailable_headers(available_headers);
                    (remaining != *missing).then_some((*id, remaining))
                }
                OwnerLocation::Retained(_) | OwnerLocation::Accepted { .. } => None,
            })
            .collect::<Vec<_>>();
        let mut recovered = Vec::new();
        for (id, remaining) in history {
            if !remaining.is_empty() {
                let owner = self.authority.owners.get_mut(&id)?;
                owner.location = OwnerLocation::ReplacementHistory { missing: remaining };
                continue;
            }
            let (version, arrival) = self.reserve_owner_identity()?;
            let generation = self.authority.generation;
            let owner = self.authority.owners.get_mut(&id)?;
            owner.version = version;
            owner.arrival = arrival;
            owner.location = OwnerLocation::Retained(RetainedOwner {
                source: Source::Recovery(generation),
                phase: RetainedPhase::Queued(WorkStage::Resolve),
            });
            recovered.push(id);
        }
        if !self
            .retained_usage()
            .is_ok_and(|usage| usage.fits(self.authority.limits.retained))
        {
            for id in &recovered {
                self.authority.owners.remove(id);
            }
            recovered.clear();
        }
        Some(recovered)
    }

    fn advance_dependency_and_wake(&mut self) -> bool {
        let available = self
            .authority
            .owners
            .values()
            .filter(|owner| matches!(owner.location, OwnerLocation::Accepted { .. }))
            .flat_map(|owner| owner.transaction.outputs.iter().copied())
            .filter(|cell| self.accepted_spender(*cell).is_none())
            .collect::<BTreeSet<_>>();
        self.apply_dependency_availability(&available, &BTreeSet::new())
            .is_some()
    }

    fn peer_ban_is_active(&self, peer: PeerId, observed_at: MonotonicTick) -> bool {
        self.authority
            .peer_bans
            .get(&peer)
            .is_some_and(|record| record.deadline.is_active_at(observed_at))
    }

    fn record_peer_ban(
        &mut self,
        peer: PeerId,
        observed_at: MonotonicTick,
        order: ApplyStamp,
    ) -> bool {
        self.authority
            .peer_bans
            .retain(|_, record| record.deadline.is_active_at(observed_at));
        while u16::try_from(self.authority.peer_bans.len())
            .ok()
            .is_some_and(|count| count >= self.authority.limits.peer_ban_fences)
        {
            let Some(oldest) = self
                .authority
                .peer_bans
                .iter()
                .min_by_key(|(_, record)| record.order)
                .map(|(peer, _)| *peer)
            else {
                return false;
            };
            self.authority.peer_bans.remove(&oldest);
        }
        self.authority.peer_bans.insert(
            peer,
            PeerBanRecord {
                deadline: PeerBanDeadline::after(
                    observed_at,
                    self.authority.limits.peer_ban_duration,
                ),
                order,
            },
        );
        true
    }

    fn capability_is_current(&self, capability: &WorkCapability) -> bool {
        self.authority
            .owners
            .get(&capability.transaction)
            .is_some_and(|owner| {
                owner.version == capability.version
                    && matches!(
                        &owner.location,
                        OwnerLocation::Retained(RetainedOwner {
                            phase: RetainedPhase::Computing(permit),
                            ..
                        }) if *permit == capability.permit() && capability.is_compatible()
                    )
                    && capability.chain == self.authority.chain
                    && capability.rules == self.authority.rules
            })
    }

    fn classify_evidence(
        &self,
        transaction: TxId,
        evidence: &ResolvedEvidence,
    ) -> EvidenceValidity {
        let Some(owner) = self.authority.owners.get(&transaction) else {
            return EvidenceValidity::Invalid;
        };
        self.classify_transaction_evidence(&owner.transaction, evidence)
    }

    fn classify_transaction_evidence(
        &self,
        transaction: &Transaction,
        evidence: &ResolvedEvidence,
    ) -> EvidenceValidity {
        if !evidence.has_transaction_shape(transaction, self.authority.rules) {
            return EvidenceValidity::Invalid;
        }
        if evidence.context.chain != self.authority.chain {
            return EvidenceValidity::RelevantChange;
        }
        let changed_origin = evidence
            .input_origins
            .iter()
            .chain(&evidence.dep_origins)
            .any(|(cell, origin)| match origin {
                InputOrigin::Pool(parent) => !self.pool_parent_produces(*parent, *cell),
                InputOrigin::Chain => self.authority.owners.values().any(|candidate| {
                    matches!(candidate.location, OwnerLocation::Accepted { .. })
                        && candidate.transaction.outputs.contains(cell)
                }),
            });
        if changed_origin {
            EvidenceValidity::RelevantChange
        } else {
            EvidenceValidity::Current
        }
    }

    fn membership_effects(
        &self,
        candidate: &Transaction,
        evaluation: &MembershipEvaluation,
    ) -> Option<Vec<LogicalEffect>> {
        let ingress_peer = self
            .authority
            .owners
            .get(&candidate.id)
            .and_then(Owner::ingress_peer);
        let MembershipEvaluation::Accepted(acceptance) = evaluation else {
            let MembershipEvaluation::Rejected(reason) = evaluation else {
                return None;
            };
            return Some(vec![LogicalEffect::membership_rejected(
                candidate,
                ingress_peer,
                *reason,
            )]);
        };

        let roots = acceptance
            .replacement_victims
            .iter()
            .chain(&acceptance.capacity_victims)
            .copied()
            .collect::<BTreeSet<_>>();
        let mut effects = Vec::with_capacity(
            1usize
                .checked_add(acceptance.replacement_victims.len())?
                .checked_add(acceptance.capacity_victims.len())?
                .checked_add(acceptance.owner_loss.terminal_dependents(&roots).len())?
                .checked_add(acceptance.owner_loss.remote_missing.len())?,
        );
        effects.push(LogicalEffect::admitted(
            candidate,
            AcceptedStatus::Pending,
            ingress_peer,
        ));
        for victim in &acceptance.replacement_victims {
            let owner = self.authority.owners.get(victim)?;
            effects.push(LogicalEffect::replaced(&owner.transaction, candidate.id));
        }
        for victim in &acceptance.capacity_victims {
            let owner = self.authority.owners.get(victim)?;
            effects.push(LogicalEffect::capacity_evicted(&owner.transaction));
        }
        effects.extend(self.unavailable_owner_loss_effects(&acceptance.owner_loss, &roots)?);
        Some(effects)
    }

    fn unavailable_owner_loss_effects(
        &self,
        owner_loss: &OwnerLossPlan,
        roots: &BTreeSet<TxId>,
    ) -> Option<Vec<LogicalEffect>> {
        let mut effects = owner_loss
            .terminal_dependents(roots)
            .into_iter()
            .map(|dependent| {
                let owner = self.authority.owners.get(&dependent)?;
                Some(LogicalEffect::membership_rejected(
                    &owner.transaction,
                    owner.ingress_peer(),
                    MembershipRejection::Unavailable,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        effects.extend(owner_loss.parent_request_effects()?);
        Some(effects)
    }

    fn append_effects(
        &mut self,
        class: EffectClass,
        stamp: ApplyStamp,
        effects: Vec<LogicalEffect>,
    ) -> bool {
        if effects.is_empty() {
            return true;
        }
        if !self.can_append_effects(class, &effects) {
            return false;
        }
        let Some(records) = EffectRecord::from_batch(stamp, class, effects) else {
            return false;
        };
        self.install_effect_records(records)
    }

    fn reserve_apply(&mut self) -> Option<ApplyStamp> {
        let next = self.authority.last_apply.0.checked_add(1)?;
        let stamp = ApplyStamp(next);
        self.authority.last_apply = stamp;
        Some(stamp)
    }

    fn reserve_admission_identity(&mut self) -> Option<(EntryVersion, Arrival, ApplyStamp)> {
        let (version, arrival) = self.reserve_owner_identity()?;
        let stamp = self.reserve_apply()?;
        Some((version, arrival, stamp))
    }

    fn reserve_owner_identity(&mut self) -> Option<(EntryVersion, Arrival)> {
        Some((self.reserve_owner_version()?, self.reserve_arrival()?))
    }

    fn reserve_owner_version(&mut self) -> Option<EntryVersion> {
        let version = EntryVersion(self.authority.next_version);
        self.authority.next_version = self.authority.next_version.checked_add(1)?;
        Some(version)
    }

    fn reserve_arrival(&mut self) -> Option<Arrival> {
        let arrival = Arrival(self.authority.next_arrival);
        self.authority.next_arrival = self.authority.next_arrival.checked_add(1)?;
        Some(arrival)
    }

    fn reserve_capability(&mut self) -> Option<CapabilityId> {
        let capability = CapabilityId(self.linear.next_capability);
        self.linear.next_capability = self.linear.next_capability.checked_add(1)?;
        Some(capability)
    }
}

fn canonical_recovery_transactions(
    mut transactions: Vec<Transaction>,
) -> (Vec<(Transaction, BTreeSet<TxId>)>, Vec<TxId>) {
    transactions.sort_by_key(|transaction| (transaction.id, transaction.witness));
    transactions.dedup_by_key(|transaction| transaction.id);
    let producers = transactions
        .iter()
        .flat_map(|transaction| {
            transaction
                .outputs
                .iter()
                .copied()
                .map(|cell| (cell, transaction.id))
        })
        .collect::<BTreeMap<_, _>>();
    let parents = transactions
        .iter()
        .map(|transaction| {
            let parents = transaction
                .inputs
                .iter()
                .chain(transaction.deps.iter())
                .filter_map(|cell| producers.get(cell).copied())
                .filter(|parent| *parent != transaction.id)
                .collect::<BTreeSet<_>>();
            (transaction.id, parents)
        })
        .collect::<BTreeMap<_, _>>();
    let mut remaining = transactions
        .into_iter()
        .map(|transaction| (transaction.id, transaction))
        .collect::<BTreeMap<_, _>>();
    let mut emitted = BTreeSet::new();
    let mut ordered = Vec::new();
    loop {
        let candidate = remaining.keys().copied().find(|id| {
            parents
                .get(id)
                .is_none_or(|required| required.iter().all(|parent| emitted.contains(parent)))
        });
        let Some(id) = candidate else {
            break;
        };
        let Some(transaction) = remaining.remove(&id) else {
            break;
        };
        let required = parents.get(&id).cloned().unwrap_or_default();
        emitted.insert(id);
        ordered.push((transaction, required));
    }
    (ordered, remaining.into_keys().collect())
}

fn chain_status(
    transaction: TxId,
    proposed: &BTreeSet<TxId>,
    gap: &BTreeSet<TxId>,
) -> AcceptedStatus {
    if proposed.contains(&transaction) {
        AcceptedStatus::Proposed
    } else if gap.contains(&transaction) {
        AcceptedStatus::Gap
    } else {
        AcceptedStatus::Pending
    }
}

fn counter_exhausted() -> KernelStep {
    KernelStep::NoAuthorityCommit(KernelDisposition::CounterExhausted)
}

pub(super) fn invariant_after_each(
    omega: &mut Omega,
    commands: impl IntoIterator<Item = KernelCommand>,
) -> Result<Vec<KernelStep>, ModelInvariantError> {
    let mut steps = Vec::new();
    omega.check_invariants()?;
    for command in commands {
        steps.push(omega.kernel_step(command));
        omega.check_invariants()?;
    }
    Ok(steps)
}
