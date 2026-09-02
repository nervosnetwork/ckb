use super::{
    AuthorityClocks, AuthorityFault, CheckoutEligibility, ClockPlanReservation,
    ConcurrentIndependentError, DerivedOwnerDelta, IndependentDelta, IndependentOwnerAction,
    IndependentOwnerCut, OwnerLocalSettlement, OwnerPrestate, PlanError, PreparedIndependentApply,
    SettlementClassification, TxPoolAuthority, settlement_dependency_inputs,
};
use crate::authority::{
    dependency::{DependencyBatchDelta, SettlementDependencyEvidence},
    exchange::{AuthorityComputeExecutionPermit, ComputeWorkerGrant, ComputeWorkerSlot},
    resources::{
        ActiveWorkAvailability, ActiveWorkOperation, ChargeRecord, OrderedResourceProjection,
        ResourceBatchPlan, ResourceError,
    },
    runtime::{AuthorityComputeAftermath, AuthorityFinishedCompute},
    scheduler::{CheckoutTicket, SchedulerBatchDelta, SchedulerExchangeWave},
    state::{EntryVersion, OwnedTx, PreAcceptedPhase, RawTxHash, WorkPermit},
    work::{CheckedOutWork, ComputeSettlement, SettlementNext, SettlementToken},
};
use ckb_network::PeerIndex;
use std::{collections::HashMap, num::NonZeroUsize, sync::Arc};

/// One finished worker slot and the exact move-only settlement evidence it
/// owns. The coordinator may submit the value to one exchange or retain it;
/// it cannot separate slot availability from capability settlement.
#[derive(Debug)]
#[must_use = "a finished compute slot must be exchanged, settled, or discharged"]
pub(in crate::authority) struct ComputeExchangeCompletion {
    slot: ComputeWorkerSlot,
    finished: AuthorityFinishedCompute,
}

/// Canonically ordered, duplicate-free linear compute capabilities bounded by
/// the configured worker topology. Construction is the sole shape gate shared
/// by the exclusive and true-shard exchange implementations.
#[must_use = "validated compute capabilities must be exchanged or recovered exactly once"]
pub(in crate::authority) struct ValidatedComputeExchangeInputs {
    completions: Vec<ComputeExchangeCompletion>,
    grants: Vec<ComputeWorkerGrant>,
}

impl ValidatedComputeExchangeInputs {
    #[cfg(feature = "profiling")]
    pub(in crate::authority) fn completion_len(&self) -> usize {
        self.completions.len()
    }

    pub(in crate::authority) fn grant_len(&self) -> usize {
        self.grants.len()
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (Vec<ComputeExchangeCompletion>, Vec<ComputeWorkerGrant>) {
        (self.completions, self.grants)
    }
}

impl ComputeExchangeCompletion {
    pub(in crate::authority) fn from_finished(
        slot: ComputeWorkerSlot,
        finished: AuthorityFinishedCompute,
    ) -> Self {
        Self { slot, finished }
    }

    pub(in crate::authority) fn version(&self) -> EntryVersion {
        self.finished.settlement().token.version
    }

    pub(in crate::authority) fn slot(&self) -> ComputeWorkerSlot {
        self.slot
    }

    pub(in crate::authority) fn permits_immediate_refill(&self) -> bool {
        self.finished.aftermath().permits_immediate_refill()
    }

    pub(in crate::authority) fn into_parts(self) -> (ComputeWorkerSlot, AuthorityFinishedCompute) {
        (self.slot, self.finished)
    }
}

#[derive(Debug)]
#[must_use = "a settled worker slot must release its post-commit consequence"]
pub(in crate::authority) struct ComputeExchangeSettled {
    slot: ComputeWorkerSlot,
    aftermath: AuthorityComputeAftermath,
}

impl ComputeExchangeSettled {
    pub(in crate::authority) fn into_parts(self) -> (ComputeWorkerSlot, AuthorityComputeAftermath) {
        (self.slot, self.aftermath)
    }
}

impl PartialEq<ComputeWorkerSlot> for ComputeExchangeSettled {
    fn eq(&self, other: &ComputeWorkerSlot) -> bool {
        self.slot == *other
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum ComputeExchangeDeferredRoute {
    ExactSettlement,
    ExchangeRetry,
    ExchangeAfterEffect,
}

#[derive(Debug)]
#[must_use = "a deferred completion must follow its sealed settlement route"]
pub(in crate::authority) struct ComputeExchangeDeferred {
    route: ComputeExchangeDeferredRoute,
    completion: ComputeExchangeCompletion,
}

impl ComputeExchangeDeferred {
    pub(in crate::authority) fn from_settlement(
        route: ComputeExchangeDeferredRoute,
        slot: ComputeWorkerSlot,
        settlement: ComputeSettlement,
        aftermath: AuthorityComputeAftermath,
    ) -> Self {
        Self {
            route,
            completion: ComputeExchangeCompletion::from_finished(
                slot,
                AuthorityFinishedCompute::from_parts(settlement, aftermath),
            ),
        }
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (ComputeExchangeDeferredRoute, ComputeExchangeCompletion) {
        (self.route, self.completion)
    }
}

#[derive(Debug)]
#[must_use = "an exchange assignment owns checked-out work for one stable worker slot"]
pub(in crate::authority) struct ComputeExchangeAssignment {
    grant: ComputeWorkerGrant,
    work: CheckedOutWork,
}

impl ComputeExchangeAssignment {
    pub(in crate::authority) fn into_parts(
        self,
    ) -> (
        ComputeWorkerSlot,
        AuthorityComputeExecutionPermit,
        CheckedOutWork,
    ) {
        let (slot, execution) = self.grant.into_parts();
        (slot, execution, self.work)
    }

    pub(in crate::authority) fn into_grant_before_commit(self) -> ComputeWorkerGrant {
        let Self { grant, work } = self;
        drop(work);
        grant
    }
}

struct OwnerLocalPremise {
    before: OwnedTx,
    settlement: OwnerLocalSettlement,
    dependency: SettlementDependencyEvidence,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the bounded classifier owns the canonical settlement premise and exact recovery capability; boxing would add an untracked allocation failure"
)]
enum ClassifiedCompletion {
    OwnerLocal {
        slot: ComputeWorkerSlot,
        token: SettlementToken,
        ingress_peer: Option<PeerIndex>,
        premise: Option<OwnerLocalPremise>,
        aftermath: AuthorityComputeAftermath,
    },
    Deferred {
        slot: ComputeWorkerSlot,
        settlement: ComputeSettlement,
        aftermath: AuthorityComputeAftermath,
        route: ComputeExchangeDeferredRoute,
    },
    Obsolete {
        slot: ComputeWorkerSlot,
        /// Retained only until the shared classifier has proved that the
        /// complete batch is owner-local. Exclusive Apply and ordinary
        /// recovery need only the stable worker slot, while a non-local member
        /// must return the exact validated cohort before mutation.
        finished: Option<AuthorityFinishedCompute>,
    },
}

impl ClassifiedCompletion {
    fn recover_into<S: ComputeExchangeRecoverySink>(self, sink: &mut S) -> Result<(), S::Error> {
        match self {
            Self::OwnerLocal {
                slot,
                token,
                aftermath,
                ..
            } => sink.recover_settlement(ComputeExchangeCompletion::from_finished(
                slot,
                AuthorityFinishedCompute::from_parts(
                    ComputeSettlement {
                        token,
                        next: SettlementNext::Retry,
                    },
                    aftermath,
                ),
            )),
            Self::Deferred {
                slot,
                settlement,
                aftermath,
                ..
            } => sink.recover_settlement(ComputeExchangeCompletion::from_finished(
                slot,
                AuthorityFinishedCompute::from_parts(settlement, aftermath),
            )),
            Self::Obsolete { slot, finished } => {
                drop(finished);
                sink.recover_obsolete(slot)
            }
        }
    }
}

#[must_use = "exchange planning failure still owns every submitted worker slot"]
pub(in crate::authority) struct ComputeExchangePlanFailure {
    error: PlanError,
    classified: std::vec::IntoIter<ClassifiedCompletion>,
    remaining: std::vec::IntoIter<ComputeExchangeCompletion>,
    grants: std::vec::IntoIter<ComputeWorkerGrant>,
}

impl std::fmt::Debug for ComputeExchangePlanFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComputeExchangePlanFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Linear failure router. It is deliberately a visitor rather than a large
/// tagged recovery value: every capability moves directly to its owner without
/// boxing or allocating in the allocator/effect-backpressure path.
pub(in crate::authority) trait ComputeExchangeRecoverySink {
    type Error;

    fn recover_settlement(
        &mut self,
        completion: ComputeExchangeCompletion,
    ) -> Result<(), Self::Error>;

    fn recover_obsolete(&mut self, slot: ComputeWorkerSlot) -> Result<(), Self::Error>;

    fn recover_grant(&mut self, grant: ComputeWorkerGrant) -> Result<(), Self::Error>;
}

#[must_use = "every failed exchange capability must be routed exactly once"]
pub(in crate::authority) struct ComputeExchangeRecoveries {
    classified: std::vec::IntoIter<ClassifiedCompletion>,
    remaining: std::vec::IntoIter<ComputeExchangeCompletion>,
    grants: std::vec::IntoIter<ComputeWorkerGrant>,
}

impl ComputeExchangeRecoveries {
    pub(in crate::authority) fn recover_into<S: ComputeExchangeRecoverySink>(
        self,
        sink: &mut S,
    ) -> Result<(), S::Error> {
        for completion in self.classified {
            completion.recover_into(sink)?;
        }
        for completion in self.remaining {
            sink.recover_settlement(completion)?;
        }
        for grant in self.grants {
            sink.recover_grant(grant)?;
        }
        Ok(())
    }
}

impl ComputeExchangePlanFailure {
    pub(in crate::authority) fn error(&self) -> &PlanError {
        &self.error
    }

    pub(in crate::authority) fn into_recovery(self) -> (PlanError, ComputeExchangeRecoveries) {
        (
            self.error,
            ComputeExchangeRecoveries {
                classified: self.classified,
                remaining: self.remaining,
                grants: self.grants,
            },
        )
    }
}

pub(super) struct ComputeExchangeDelta {
    owner_cuts: Vec<IndependentOwnerCut>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    clocks: AuthorityClocks,
    retired: super::RetiredOwners,
}

impl ComputeExchangeDelta {
    fn into_independent(self) -> IndependentDelta {
        IndependentDelta {
            owner_cuts: self.owner_cuts,
            owners: self.owners,
            resource: Some(self.resources),
            projection: super::ProjectionDelta::empty(),
            scheduler: self.scheduler,
            dependency: self.dependency,
            effect: super::EffectDelta::default(),
            clocks: self.clocks,
            async_process_starts: Vec::new(),
            removals: Vec::new(),
            retired: self.retired,
        }
    }
}

struct OwnerChange {
    key: RawTxHash,
    before: OwnedTx,
    after: OwnedTx,
}

struct OwnerOverlay {
    positions: HashMap<RawTxHash, usize>,
    changes: Vec<OwnerChange>,
}

impl OwnerOverlay {
    fn new(maximum_changes: usize) -> Result<Self, PlanError> {
        let mut positions = HashMap::new();
        positions
            .try_reserve(maximum_changes)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut changes = Vec::new();
        changes
            .try_reserve(maximum_changes)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        Ok(Self { positions, changes })
    }

    fn current(&self, authority: &TxPoolAuthority, key: &RawTxHash) -> Result<OwnedTx, PlanError> {
        match self.positions.get(key).copied() {
            Some(position) => self
                .changes
                .get(position)
                .map(|change| change.after.clone())
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection)),
            None => authority
                .entries
                .get(key)
                .as_deref()
                .cloned()
                .ok_or(PlanError::Stale(super::StalePlan::Missing)),
        }
    }

    fn base_is_current(&self, authority: &TxPoolAuthority) -> bool {
        self.changes.iter().all(|change| {
            authority
                .entries
                .get(&change.key)
                .is_some_and(|owner| owner.record().version == change.before.record().version)
        })
    }

    fn replace_ticket(
        &mut self,
        key: RawTxHash,
        before: OwnedTx,
        after: OwnedTx,
    ) -> Result<(), PlanError> {
        if let Some(position) = self.positions.get(&key).copied() {
            let change = self
                .changes
                .get_mut(position)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            if change.after.record().version != before.record().version {
                return Err(PlanError::Stale(super::StalePlan::Version));
            }
            change.after = after;
            return Ok(());
        }
        self.replace_captured(key, before, after)
    }

    fn replace_captured(
        &mut self,
        key: RawTxHash,
        before: OwnedTx,
        after: OwnedTx,
    ) -> Result<(), PlanError> {
        if before.record().identity.raw != key
            || self.positions.contains_key(&key)
            || after.record().identity.raw != key
        {
            return Err(PlanError::Stale(super::StalePlan::Version));
        }
        let position = self.changes.len();
        self.positions.insert(key.clone(), position);
        self.changes.push(OwnerChange { key, before, after });
        Ok(())
    }
}

struct CheckoutReservation {
    before: OwnedTx,
    grant: crate::authority::resources::ComputeGrant,
    after_charge: crate::authority::resources::ResourceVector,
}

struct PlannedAssignment {
    permit: WorkPermit,
    ticket: CheckoutTicket,
    reservation: CheckoutReservation,
}

enum CandidateUnavailable {
    SkipOwner,
    Stop,
}

type CandidateCheckout = Result<CheckoutReservation, CandidateUnavailable>;

struct CompiledComputeExchange {
    delta: Option<ComputeExchangeDelta>,
    assignments: Vec<ComputeExchangeAssignment>,
    unused_grants: Vec<ComputeWorkerGrant>,
}

struct ComputeExchangeCompileFailure {
    error: PlanError,
    grants: Vec<ComputeWorkerGrant>,
}

struct ComputeExchangeOutcomeBuffers {
    settled: Vec<ComputeExchangeSettled>,
    obsolete: Vec<ComputeWorkerSlot>,
    deferred: Vec<ComputeExchangeDeferred>,
}

struct PreparedSharedComputeExchangeOutputs {
    classified: Vec<ClassifiedCompletion>,
    assignments: Vec<ComputeExchangeAssignment>,
    unused_grants: Vec<ComputeWorkerGrant>,
    outcomes: ComputeExchangeOutcomeBuffers,
}

impl ComputeExchangeOutcomeBuffers {
    fn new(maximum_completions: usize) -> Result<Self, PlanError> {
        let mut settled = Vec::new();
        settled
            .try_reserve(maximum_completions)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut obsolete = Vec::new();
        obsolete
            .try_reserve(maximum_completions)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut deferred = Vec::new();
        deferred
            .try_reserve(maximum_completions)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        Ok(Self {
            settled,
            obsolete,
            deferred,
        })
    }
}

#[must_use = "committed exchange consequences must leave the authority guard"]
pub(in crate::authority) struct CommittedComputeExchange {
    pub(in crate::authority) retirement: Option<super::CommittedDelta>,
    pub(in crate::authority) settled: Vec<ComputeExchangeSettled>,
    pub(in crate::authority) obsolete: Vec<ComputeWorkerSlot>,
    pub(in crate::authority) deferred: Vec<ComputeExchangeDeferred>,
    pub(in crate::authority) assignments: Vec<ComputeExchangeAssignment>,
    pub(in crate::authority) unused_grants: Vec<ComputeWorkerGrant>,
}

/// Capabilities returned by a shared exact-shard Apply that proved stale or
/// faulted before owner mutation. Owner-local completions are sealed to retry,
/// exact deferrals retain their route, and every refill grant is returned.
#[must_use = "a recovered compute exchange must route every completion and worker grant"]
pub(in crate::authority) struct RecoveredComputeExchange {
    pub(in crate::authority) obsolete: Vec<ComputeWorkerSlot>,
    pub(in crate::authority) deferred: Vec<ComputeExchangeDeferred>,
    pub(in crate::authority) unused_grants: Vec<ComputeWorkerGrant>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the committed and recovered variants own bounded preallocated linear capabilities; boxing would allocate at the Apply terminal"
)]
#[must_use = "a shared compute exchange terminal must be published or recovered"]
pub(in crate::authority) enum SharedComputeExchangeOutcome {
    Committed {
        exchange: CommittedComputeExchange,
        post_commit_fault: Option<AuthorityFault>,
    },
    RetryProbe(RecoveredComputeExchange),
    Fault {
        fault: AuthorityFault,
        recovered: RecoveredComputeExchange,
    },
}

#[must_use = "a prepared shared compute exchange must Apply or return every capability"]
pub(in crate::authority) struct PreparedSharedComputeExchange<'authority> {
    apply: SharedComputeApply<'authority>,
    outputs: PreparedSharedComputeExchangeOutputs,
    recovery_grants: Vec<ComputeWorkerGrant>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the bounded shared terminal owns its already-prepared Apply; boxing would add an untracked allocation failure after planning"
)]
enum SharedComputeApply<'authority> {
    CommitNoop,
    Apply(PreparedIndependentApply<'authority>),
    RetryProbe,
}

impl PreparedSharedComputeExchangeOutputs {
    fn commit(self, retirement: Option<super::CommittedDelta>) -> CommittedComputeExchange {
        let ComputeExchangeOutcomeBuffers {
            mut settled,
            mut obsolete,
            mut deferred,
        } = self.outcomes;
        for classified in self.classified {
            match classified {
                ClassifiedCompletion::OwnerLocal {
                    slot, aftermath, ..
                } => settled.push(ComputeExchangeSettled { slot, aftermath }),
                ClassifiedCompletion::Deferred {
                    slot,
                    settlement,
                    aftermath,
                    route,
                } => deferred.push(ComputeExchangeDeferred {
                    route,
                    completion: ComputeExchangeCompletion::from_finished(
                        slot,
                        AuthorityFinishedCompute::from_parts(settlement, aftermath),
                    ),
                }),
                ClassifiedCompletion::Obsolete { slot, finished } => {
                    drop(finished);
                    obsolete.push(slot);
                }
            }
        }
        CommittedComputeExchange {
            retirement,
            settled,
            obsolete,
            deferred,
            assignments: self.assignments,
            unused_grants: self.unused_grants,
        }
    }

    fn recover_before_owner_mutation(
        self,
        mut recovery_grants: Vec<ComputeWorkerGrant>,
    ) -> RecoveredComputeExchange {
        let ComputeExchangeOutcomeBuffers {
            settled,
            mut obsolete,
            mut deferred,
        } = self.outcomes;
        debug_assert!(
            settled.is_empty(),
            "shared owner-local compilation cannot pre-settle an exclusive completion"
        );
        drop(settled);
        for classified in self.classified {
            match classified {
                ClassifiedCompletion::OwnerLocal {
                    slot,
                    token,
                    aftermath,
                    ..
                } => deferred.push(ComputeExchangeDeferred::from_settlement(
                    ComputeExchangeDeferredRoute::ExchangeRetry,
                    slot,
                    ComputeSettlement {
                        token,
                        next: SettlementNext::Retry,
                    },
                    aftermath,
                )),
                ClassifiedCompletion::Deferred {
                    slot,
                    settlement,
                    aftermath,
                    route,
                } => deferred.push(ComputeExchangeDeferred::from_settlement(
                    route, slot, settlement, aftermath,
                )),
                ClassifiedCompletion::Obsolete { slot, finished } => {
                    drop(finished);
                    obsolete.push(slot);
                }
            }
        }
        recovery_grants.extend(
            self.assignments
                .into_iter()
                .map(ComputeExchangeAssignment::into_grant_before_commit),
        );
        recovery_grants.extend(self.unused_grants);
        RecoveredComputeExchange {
            obsolete,
            deferred,
            unused_grants: recovery_grants,
        }
    }
}

impl PreparedSharedComputeExchange<'_> {
    pub(in crate::authority) fn apply(self) -> SharedComputeExchangeOutcome {
        let Self {
            apply,
            outputs,
            recovery_grants,
        } = self;
        let plan = match apply {
            SharedComputeApply::CommitNoop => {
                return SharedComputeExchangeOutcome::Committed {
                    exchange: outputs.commit(None),
                    post_commit_fault: None,
                };
            }
            SharedComputeApply::RetryProbe => {
                return SharedComputeExchangeOutcome::RetryProbe(
                    outputs.recover_before_owner_mutation(recovery_grants),
                );
            }
            SharedComputeApply::Apply(plan) => plan,
        };
        match plan.apply() {
            Ok(committed) => {
                let (retirement, post_commit_fault) = committed.into_parts();
                SharedComputeExchangeOutcome::Committed {
                    exchange: outputs.commit(Some(retirement)),
                    post_commit_fault,
                }
            }
            Err(ConcurrentIndependentError::ChangedCut(_)) => {
                SharedComputeExchangeOutcome::RetryProbe(
                    outputs.recover_before_owner_mutation(recovery_grants),
                )
            }
            Err(ConcurrentIndependentError::Fault(fault)) => SharedComputeExchangeOutcome::Fault {
                fault,
                recovered: outputs.recover_before_owner_mutation(recovery_grants),
            },
        }
    }
}

fn deferred_next(nonlocal: super::NonLocalSettlement) -> SettlementNext {
    match nonlocal {
        super::NonLocalSettlement::Waiting(missing) => SettlementNext::Waiting(missing),
        super::NonLocalSettlement::Rejected(rejection) => SettlementNext::Rejected(rejection),
        super::NonLocalSettlement::VerificationRejected {
            rejection,
            resolved,
        } => SettlementNext::VerificationRejected {
            rejection,
            resolved,
        },
    }
}

fn defer_owner_local(
    member: &mut ClassifiedCompletion,
    peer: Option<PeerIndex>,
    route: ComputeExchangeDeferredRoute,
) {
    let should_defer = matches!(
        member,
        ClassifiedCompletion::OwnerLocal { ingress_peer, .. }
            if peer.is_none() || *ingress_peer == peer
    );
    if !should_defer {
        return;
    }
    let slot = match member {
        ClassifiedCompletion::OwnerLocal { slot, .. } => *slot,
        ClassifiedCompletion::Deferred { .. } | ClassifiedCompletion::Obsolete { .. } => return,
    };
    let previous = std::mem::replace(
        member,
        ClassifiedCompletion::Obsolete {
            slot,
            finished: None,
        },
    );
    let ClassifiedCompletion::OwnerLocal {
        slot,
        token,
        aftermath,
        ..
    } = previous
    else {
        return;
    };
    *member = ClassifiedCompletion::Deferred {
        slot,
        settlement: ComputeSettlement {
            token,
            next: SettlementNext::Retry,
        },
        aftermath,
        route,
    };
}

impl TxPoolAuthority {
    fn compute_exchange_derived_error<E>(&self, owners: &OwnerOverlay, error: E) -> PlanError
    where
        E: Into<PlanError>,
    {
        let error = error.into();
        let optimistic_prestate_error = matches!(
            error,
            PlanError::Backpressure(
                super::Backpressure::ProposalCollision
                    | super::Backpressure::TotalResources
                    | super::Backpressure::RemoteResources
                    | super::Backpressure::PeerResources
                    | super::Backpressure::AcceptedResources
                    | super::Backpressure::ComputeResources
            ) | PlanError::Fault(
                AuthorityFault::ResourceProjection
                    | AuthorityFault::MembershipProjection
                    | AuthorityFault::IndexProjection
                    | AuthorityFault::SchedulerProjection
                    | AuthorityFault::DependencyProjection
            )
        );
        if optimistic_prestate_error && !owners.base_is_current(self) {
            PlanError::Stale(super::StalePlan::Version)
        } else {
            error
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    #[cfg(test)]
    pub(in crate::authority) fn apply_compute_exchange(
        &mut self,
        completions: Vec<ComputeExchangeCompletion>,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<CommittedComputeExchange, ComputeExchangePlanFailure> {
        let inputs = self.validate_compute_exchange_inputs(completions, grants)?;
        let prepared = self.prepare_shared_compute_exchange(inputs)?;
        match prepared.apply() {
            SharedComputeExchangeOutcome::Committed {
                exchange,
                post_commit_fault,
            } => {
                assert!(
                    post_commit_fault.is_none(),
                    "the no-interleave production-path oracle cannot hide a post-commit fault"
                );
                Ok(exchange)
            }
            SharedComputeExchangeOutcome::RetryProbe(recovered) => {
                drop(recovered);
                panic!("the no-interleave production-path oracle cannot observe a changed cut")
            }
            SharedComputeExchangeOutcome::Fault { fault, recovered } => {
                drop(recovered);
                panic!("the no-interleave production-path oracle faulted: {fault:?}")
            }
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    pub(in crate::authority) fn validate_compute_exchange_inputs(
        &self,
        mut completions: Vec<ComputeExchangeCompletion>,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<ValidatedComputeExchangeInputs, ComputeExchangePlanFailure> {
        // Completions and refill grants are distinct linear partitions. A
        // finished slot may contribute one of each in the same settle/refill
        // exchange, so their valid aggregate bound is `2 * P`, not the
        // unrelated membership-component limit. Validate each partition
        // directly against the configured `P` active-work slots.
        let active_work_limit = self.resources.limits().active_work_limit();
        if completions.len() > active_work_limit || grants.len() > active_work_limit {
            return Err(exchange_failure(
                PlanError::Fault(AuthorityFault::SchedulerProjection),
                Vec::new(),
                completions,
                grants,
            ));
        }
        completions.sort_unstable_by_key(ComputeExchangeCompletion::version);
        if completions
            .array_windows::<2>()
            .any(|[left, right]| left.version() == right.version())
        {
            return Err(exchange_failure(
                PlanError::Fault(AuthorityFault::SchedulerProjection),
                Vec::new(),
                completions,
                grants,
            ));
        }

        let mut completion_slots = Vec::new();
        if completion_slots.try_reserve(completions.len()).is_err() {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                Vec::new(),
                completions,
                grants,
            ));
        }
        completion_slots.extend(completions.iter().map(|completion| completion.slot.id()));
        completion_slots.sort_unstable();
        let duplicate_completion = completion_slots
            .array_windows::<2>()
            .any(|[left, right]| left == right);
        let mut grant_slots = Vec::new();
        if grant_slots.try_reserve(grants.len()).is_err() {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                Vec::new(),
                completions,
                grants,
            ));
        }
        grant_slots.extend(grants.iter().map(|grant| grant.slot().id()));
        grant_slots.sort_unstable();
        let duplicate_grant = grant_slots
            .array_windows::<2>()
            .any(|[left, right]| left == right);
        if duplicate_completion || duplicate_grant {
            return Err(exchange_failure(
                PlanError::Fault(AuthorityFault::SchedulerProjection),
                Vec::new(),
                completions,
                grants,
            ));
        }
        Ok(ValidatedComputeExchangeInputs {
            completions,
            grants,
        })
    }

    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    pub(in crate::authority) fn prepare_shared_compute_exchange(
        &self,
        inputs: ValidatedComputeExchangeInputs,
    ) -> Result<PreparedSharedComputeExchange<'_>, ComputeExchangePlanFailure> {
        Self::prepare_compute_exchange_with(self, inputs)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    fn prepare_compute_exchange_with<'authority>(
        authority: &'authority TxPoolAuthority,
        inputs: ValidatedComputeExchangeInputs,
    ) -> Result<PreparedSharedComputeExchange<'authority>, ComputeExchangePlanFailure> {
        let (completions, grants) = inputs.into_parts();
        if let Err(error) = authority.effects.lock().ensure_open() {
            return Err(exchange_failure(
                error.into(),
                Vec::new(),
                completions,
                grants,
            ));
        }
        let completion_count = completions.len();
        let outcomes = match ComputeExchangeOutcomeBuffers::new(completions.len()) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                return Err(exchange_failure(error, Vec::new(), completions, grants));
            }
        };
        let mut classified = Vec::new();
        if classified.try_reserve(completions.len()).is_err() {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                classified,
                completions,
                grants,
            ));
        }
        let mut recovery_grants = Vec::new();
        if recovery_grants.try_reserve(grants.len()).is_err() {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                classified,
                completions,
                grants,
            ));
        }
        let mut blocked_revocation_peers = Vec::new();
        if blocked_revocation_peers
            .try_reserve(completion_count)
            .is_err()
        {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                classified,
                completions,
                grants,
            ));
        }
        let mut remaining = completions.into_iter();
        while let Some(completion) = remaining.next() {
            let ComputeExchangeCompletion { slot, finished } = completion;
            let (settlement, aftermath) = finished.into_parts();
            let ComputeSettlement { token, next } = settlement;
            let existing = match authority.entries.get(&token.hash).as_deref().cloned() {
                Some(existing) if existing.record().version == token.version => existing,
                Some(_) | None => {
                    classified.push(ClassifiedCompletion::Obsolete {
                        slot,
                        finished: Some(AuthorityFinishedCompute::from_parts(
                            ComputeSettlement { token, next },
                            aftermath,
                        )),
                    });
                    continue;
                }
            };
            let OwnedTx::PreAccepted(preaccepted) = &existing else {
                classified.push(ClassifiedCompletion::Obsolete {
                    slot,
                    finished: Some(AuthorityFinishedCompute::from_parts(
                        ComputeSettlement { token, next },
                        aftermath,
                    )),
                });
                continue;
            };
            let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
                classified.push(ClassifiedCompletion::Obsolete {
                    slot,
                    finished: Some(AuthorityFinishedCompute::from_parts(
                        ComputeSettlement { token, next },
                        aftermath,
                    )),
                });
                continue;
            };
            if preaccepted.charge.active_work != 1 {
                classified.push(ClassifiedCompletion::Deferred {
                    slot,
                    settlement: ComputeSettlement {
                        token,
                        next: SettlementNext::Retry,
                    },
                    aftermath,
                    route: ComputeExchangeDeferredRoute::ExchangeRetry,
                });
                return Err(exchange_failure_from_iter(
                    PlanError::Fault(AuthorityFault::ResourceProjection),
                    classified,
                    remaining,
                    grants,
                ));
            }
            if preaccepted
                .source
                .ingress_peer()
                .is_some_and(|peer| blocked_revocation_peers.contains(&peer))
            {
                classified.push(ClassifiedCompletion::Deferred {
                    slot,
                    settlement: ComputeSettlement { token, next },
                    aftermath,
                    route: ComputeExchangeDeferredRoute::ExchangeAfterEffect,
                });
                continue;
            }
            let (candidate_dependencies, missing_dependencies) =
                settlement_dependency_inputs(&next);
            let dependency = match authority.dependencies.capture_settlement_evidence(
                &token.hash,
                preaccepted.dependencies(),
                candidate_dependencies,
                missing_dependencies,
            ) {
                Ok(dependency) => dependency,
                Err(error) => {
                    classified.push(ClassifiedCompletion::Deferred {
                        slot,
                        settlement: ComputeSettlement {
                            token,
                            next: SettlementNext::Retry,
                        },
                        aftermath,
                        route: ComputeExchangeDeferredRoute::ExchangeRetry,
                    });
                    return Err(exchange_failure_from_iter(
                        error.into(),
                        classified,
                        remaining,
                        grants,
                    ));
                }
            };
            match authority.classify_settlement(preaccepted, active, &dependency, next) {
                Ok(SettlementClassification::OwnerLocal(settlement)) => {
                    classified.push(ClassifiedCompletion::OwnerLocal {
                        slot,
                        token,
                        ingress_peer: preaccepted.source.ingress_peer(),
                        premise: Some(OwnerLocalPremise {
                            before: existing,
                            settlement,
                            dependency,
                        }),
                        aftermath,
                    });
                }
                Ok(SettlementClassification::NonLocal(nonlocal)) => {
                    let revocation_peer = match &nonlocal {
                        super::NonLocalSettlement::Rejected(rejection)
                            if rejection.is_malformed() =>
                        {
                            preaccepted.source.payload_blame_peer()
                        }
                        super::NonLocalSettlement::VerificationRejected { rejection, .. }
                            if rejection.is_malformed() =>
                        {
                            preaccepted.source.payload_blame_peer()
                        }
                        super::NonLocalSettlement::Waiting(_)
                        | super::NonLocalSettlement::Rejected(_)
                        | super::NonLocalSettlement::VerificationRejected { .. } => None,
                    };
                    if let Some(peer) = revocation_peer {
                        if !blocked_revocation_peers.contains(&peer) {
                            blocked_revocation_peers.push(peer);
                        }
                        for member in &mut classified {
                            defer_owner_local(
                                member,
                                Some(peer),
                                ComputeExchangeDeferredRoute::ExchangeAfterEffect,
                            );
                        }
                    }
                    classified.push(ClassifiedCompletion::Deferred {
                        slot,
                        settlement: ComputeSettlement {
                            token,
                            next: deferred_next(nonlocal),
                        },
                        aftermath,
                        route: ComputeExchangeDeferredRoute::ExactSettlement,
                    });
                }
                Err(error) => {
                    classified.push(ClassifiedCompletion::Deferred {
                        slot,
                        settlement: ComputeSettlement {
                            token,
                            next: SettlementNext::Retry,
                        },
                        aftermath,
                        route: ComputeExchangeDeferredRoute::ExchangeRetry,
                    });
                    return Err(exchange_failure_from_iter(
                        error, classified, remaining, grants,
                    ));
                }
            }
        }

        match Self::compile_compute_exchange_inner(
            authority,
            &mut classified,
            grants,
            &blocked_revocation_peers,
        ) {
            Ok(CompiledComputeExchange {
                delta,
                assignments,
                unused_grants,
            }) => {
                let outputs = PreparedSharedComputeExchangeOutputs {
                    classified,
                    assignments,
                    unused_grants,
                    outcomes,
                };
                let apply = delta.map_or(SharedComputeApply::CommitNoop, |delta| {
                    let delta = delta.into_independent();
                    let support = delta.physical_support(authority);
                    SharedComputeApply::Apply(PreparedIndependentApply::Shared {
                        authority,
                        delta,
                        support,
                        staged_effect: None,
                    })
                });
                Ok(PreparedSharedComputeExchange {
                    apply,
                    outputs,
                    recovery_grants,
                })
            }
            Err(ComputeExchangeCompileFailure {
                error: PlanError::Stale(_),
                grants,
            }) => Ok(PreparedSharedComputeExchange {
                apply: SharedComputeApply::RetryProbe,
                outputs: PreparedSharedComputeExchangeOutputs {
                    classified,
                    assignments: Vec::new(),
                    unused_grants: grants,
                    outcomes,
                },
                recovery_grants,
            }),
            Err(ComputeExchangeCompileFailure { error, grants }) => {
                Err(exchange_failure(error, classified, Vec::new(), grants))
            }
        }
    }

    fn compile_compute_exchange_inner(
        &self,
        classified: &mut [ClassifiedCompletion],
        mut grants: Vec<ComputeWorkerGrant>,
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<CompiledComputeExchange, ComputeExchangeCompileFailure> {
        grants.sort_unstable_by_key(|grant| grant.slot().work_selection_key());
        let mut slots = Vec::new();
        if slots.try_reserve(grants.len()).is_err() {
            return Err(ComputeExchangeCompileFailure {
                error: PlanError::Backpressure(super::Backpressure::Allocation),
                grants,
            });
        }
        slots.extend(grants.iter().map(ComputeWorkerGrant::slot));
        let mut committed_assignments = Vec::new();
        if committed_assignments.try_reserve(grants.len()).is_err() {
            return Err(ComputeExchangeCompileFailure {
                error: PlanError::Backpressure(super::Backpressure::Allocation),
                grants,
            });
        }
        let mut unused_grants = Vec::new();
        if unused_grants.try_reserve(grants.len()).is_err() {
            return Err(ComputeExchangeCompileFailure {
                error: PlanError::Backpressure(super::Backpressure::Allocation),
                grants,
            });
        }

        let (delta, work) =
            match self.compile_compute_exchange_state(classified, &slots, blocked_revocation_peers)
            {
                Ok(result) => result,
                Err(error) => return Err(ComputeExchangeCompileFailure { error, grants }),
            };
        for (grant, work) in grants.into_iter().zip(work) {
            match work {
                Some(work) => committed_assignments.push(ComputeExchangeAssignment { grant, work }),
                None => unused_grants.push(grant),
            }
        }
        Ok(CompiledComputeExchange {
            delta,
            assignments: committed_assignments,
            unused_grants,
        })
    }

    fn compile_compute_exchange_state(
        &self,
        classified: &mut [ClassifiedCompletion],
        grant_slots: &[ComputeWorkerSlot],
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<(Option<ComputeExchangeDelta>, Vec<Option<CheckedOutWork>>), PlanError> {
        let owner_local_bound = classified
            .iter()
            .filter(|member| matches!(member, ClassifiedCompletion::OwnerLocal { .. }))
            .count();
        let transition_bound = owner_local_bound
            .checked_add(classified.len())
            .and_then(|count| count.checked_add(grant_slots.len()))
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        let mut owners = OwnerOverlay::new(transition_bound)?;
        let mut resources = self
            .resources
            .ordered_projection(&self.entries, transition_bound)?;
        let active_work_revision = resources.active_work_revision();
        let capacity_observation = resources.capacity_observation();
        let mut clock_plan = ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks));
        #[cfg(test)]
        self.entries.enter_compute_exchange_probe(
            crate::authority::shard::ComputeExchangeProbePhase::AfterClassification,
        );
        let mut settlement_evidence = Vec::new();
        settlement_evidence
            .try_reserve(owner_local_bound)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;

        let mut local_count = 0usize;
        for member in classified.iter_mut() {
            let (token, ingress_peer, premise) = match member {
                ClassifiedCompletion::OwnerLocal {
                    token,
                    ingress_peer,
                    premise: Some(premise),
                    ..
                } => (token, *ingress_peer, premise),
                ClassifiedCompletion::OwnerLocal { .. } => {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                ClassifiedCompletion::Deferred { .. } | ClassifiedCompletion::Obsolete { .. } => {
                    continue;
                }
            };
            if premise.before.record().version != token.version
                || premise.before.record().identity.raw != token.hash
            {
                return Err(PlanError::Stale(super::StalePlan::Version));
            }
            let current = self
                .entries
                .get(&token.hash)
                .ok_or(PlanError::Stale(super::StalePlan::Missing))?;
            if current.record().version != token.version {
                return Err(PlanError::Stale(super::StalePlan::Version));
            }
            let OwnedTx::PreAccepted(preaccepted) = &premise.before else {
                return Err(PlanError::Stale(super::StalePlan::Phase));
            };
            if !matches!(&preaccepted.phase, PreAcceptedPhase::Computing(_)) {
                return Err(PlanError::Stale(super::StalePlan::Phase));
            }
            if preaccepted.charge.active_work != 1 {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
            drop(current);
            let desired_charge = ChargeRecord::PreAccepted {
                resources: premise.settlement.charge,
                residency_peer: ingress_peer,
                compute_peer: None,
            };
            let resource_result = resources.replace(
                self.resources.read(&self.entries),
                Some(premise.before.charge_record()),
                Some(desired_charge),
            );
            if self
                .entries
                .get(&token.hash)
                .is_none_or(|owner| owner.record().version != token.version)
            {
                return Err(PlanError::Stale(super::StalePlan::Version));
            }
            match resource_result {
                Ok(()) => {}
                Err(
                    ResourceError::PreAcceptedLimit
                    | ResourceError::RemoteLimit
                    | ResourceError::PeerLimit(_),
                ) => {
                    let slot = match member {
                        ClassifiedCompletion::OwnerLocal { slot, .. } => *slot,
                        ClassifiedCompletion::Deferred { .. }
                        | ClassifiedCompletion::Obsolete { .. } => {
                            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                        }
                    };
                    let previous = std::mem::replace(
                        member,
                        ClassifiedCompletion::Obsolete {
                            slot,
                            finished: None,
                        },
                    );
                    let ClassifiedCompletion::OwnerLocal {
                        slot,
                        token,
                        premise: Some(premise),
                        aftermath,
                        ..
                    } = previous
                    else {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    };
                    *member = ClassifiedCompletion::Deferred {
                        slot,
                        settlement: ComputeSettlement {
                            token,
                            next: premise.settlement.into_exact_next(),
                        },
                        aftermath,
                        route: ComputeExchangeDeferredRoute::ExactSettlement,
                    };
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            local_count = local_count
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        }

        if let Some(local_count) = NonZeroUsize::new(local_count) {
            let (mut versions, reserved) = clock_plan.replacements(local_count)?;
            clock_plan = reserved;
            for member in classified.iter_mut() {
                let ClassifiedCompletion::OwnerLocal { token, premise, .. } = member else {
                    continue;
                };
                let premise = premise
                    .take()
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                let version = versions
                    .next()
                    .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
                let OwnerLocalPremise {
                    before,
                    settlement,
                    dependency,
                } = premise;
                let after = before
                    .with_preaccepted_phase(
                        settlement.phase.into_preaccepted(),
                        version,
                        settlement.charge,
                    )
                    .map_err(PlanError::Stale)?;
                owners.replace_captured(token.hash.clone(), before, after)?;
                settlement_evidence.push(dependency);
            }
            debug_assert!(versions.next().is_none());
        }

        let mut wave = SchedulerExchangeWave::after(
            Arc::clone(&self.scheduler),
            owners.changes.iter().map(|change| &change.after),
            grant_slots.len(),
        )
        .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        let mut blocked_slots = Vec::new();
        blocked_slots
            .try_reserve(classified.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for member in classified.iter() {
            if let ClassifiedCompletion::Deferred { slot, .. } = member {
                blocked_slots.push(slot.id());
            }
        }
        blocked_slots.sort_unstable();
        let mut assignments = Vec::new();
        assignments
            .try_reserve(grant_slots.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        assignments.resize_with(grant_slots.len(), || None);

        // Give every verifier primary lane one complete resource-aware pass
        // before any verifier can consume Resolve fallback capacity. This
        // preserves the Small-only lane and prevents a fallback from blocking
        // an already-runnable Large Verify owned by another verifier.
        for (assignment, slot) in assignments.iter_mut().zip(grant_slots.iter().copied()) {
            if slot.fallback_permit().is_none() || blocked_slots.binary_search(&slot.id()).is_ok() {
                continue;
            }
            *assignment = self
                .search_exchange_permit(
                    &owners,
                    &mut resources,
                    &mut wave,
                    slot.primary_permit(),
                    blocked_revocation_peers,
                )
                .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        }
        for (assignment, slot) in assignments.iter_mut().zip(grant_slots.iter().copied()) {
            if assignment.is_some() || blocked_slots.binary_search(&slot.id()).is_ok() {
                continue;
            }
            let Some(fallback) = slot.fallback_permit() else {
                continue;
            };
            *assignment = self
                .search_exchange_permit(
                    &owners,
                    &mut resources,
                    &mut wave,
                    fallback,
                    blocked_revocation_peers,
                )
                .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        }
        for (assignment, slot) in assignments.iter_mut().zip(grant_slots.iter().copied()) {
            if slot.fallback_permit().is_some() || blocked_slots.binary_search(&slot.id()).is_ok() {
                continue;
            }
            *assignment = self
                .search_exchange_permit(
                    &owners,
                    &mut resources,
                    &mut wave,
                    slot.primary_permit(),
                    blocked_revocation_peers,
                )
                .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        }

        #[cfg(test)]
        self.entries.enter_compute_exchange_probe(
            crate::authority::shard::ComputeExchangeProbePhase::AfterSchedulerWave,
        );

        let transition_count = local_count
            .checked_add(
                assignments
                    .iter()
                    .filter(|assignment| assignment.is_some())
                    .count(),
            )
            .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        if transition_count == 0 {
            let mut jobs = Vec::new();
            jobs.try_reserve(grant_slots.len())
                .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
            jobs.resize_with(grant_slots.len(), || None);
            return Ok((None, jobs));
        }
        let mut jobs = Vec::new();
        jobs.try_reserve(grant_slots.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let assignment_count = assignments
            .iter()
            .filter(|assignment| assignment.is_some())
            .count();
        let (mut assignment_versions, clock_plan) =
            if let Some(assignment_count) = NonZeroUsize::new(assignment_count) {
                let (versions, clock_plan) = clock_plan.replacements(assignment_count)?;
                (Some(versions), clock_plan)
            } else {
                (None, clock_plan)
            };
        let clocks = clock_plan.commit()?;
        let sequence = clocks.sequence();
        for assignment in assignments {
            let Some(assignment) = assignment else {
                jobs.push(None);
                continue;
            };
            let version = assignment_versions
                .as_mut()
                .and_then(Iterator::next)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let PlannedAssignment {
                permit,
                ticket,
                reservation,
            } = assignment;
            let key = ticket.hash().clone();
            let OwnedTx::PreAccepted(preaccepted) = &reservation.before else {
                return Err(PlanError::Stale(super::StalePlan::Phase));
            };
            let (work, active) = CheckedOutWork::from_owner(
                version,
                self.chain_view.clone(),
                crate::authority::state::DependencyCut(sequence),
                permit,
                reservation.grant,
                preaccepted,
            )
            .map_err(|_| PlanError::Fault(AuthorityFault::SchedulerProjection))?;
            let before = reservation.before;
            let after = before
                .with_preaccepted_phase(
                    PreAcceptedPhase::Computing(active),
                    version,
                    reservation.after_charge,
                )
                .map_err(PlanError::Stale)?;
            owners.replace_ticket(key, before, after)?;
            jobs.push(Some(work));
        }
        debug_assert!(
            assignment_versions
                .as_mut()
                .is_none_or(|versions| versions.next().is_none())
        );
        let clocks = clocks.finish();

        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve(owners.changes.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for change in &owners.changes {
            resource_changes.push((
                change.key.clone(),
                Some(change.before.charge_record()),
                Some(change.after.charge_record()),
            ));
        }
        let exchange_cursor = wave.into_cursor();
        let mut resource_plan = self
            .resources_for_plan()
            .plan_batch(resource_changes)
            .map_err(|error| {
                let is_contended_limit = matches!(
                    error,
                    ResourceError::PreAcceptedLimit
                        | ResourceError::RemoteLimit
                        | ResourceError::PeerLimit(_)
                        | ResourceError::AcceptedLimit
                );
                let current_observation = self.resources_for_plan().capacity_observation();
                let contention_explains_limit =
                    current_observation.explains_limit(capacity_observation, &error);
                let error = self.compute_exchange_derived_error(&owners, error);
                if is_contended_limit
                    && !matches!(error, PlanError::Stale(_))
                    && contention_explains_limit
                {
                    PlanError::ResourceContended(self.resources_for_plan().capacity_wait_identity())
                } else {
                    error
                }
            })?;
        let active_work_transition = owners.changes.iter().filter_map(|change| {
            let (
                ChargeRecord::PreAccepted {
                    resources: before, ..
                },
                ChargeRecord::PreAccepted {
                    resources: after, ..
                },
            ) = (change.before.charge_record(), change.after.charge_record())
            else {
                return None;
            };
            match (before.active_work, after.active_work) {
                (1, 0) => Some((change.after.record().version, ActiveWorkOperation::Release)),
                (0, 1) => Some((change.after.record().version, ActiveWorkOperation::Acquire)),
                _ => None,
            }
        });
        if let Some((version, operation)) = active_work_transition.last() {
            resource_plan
                .seal_active_work_revision(active_work_revision.seal(version, operation))
                .map_err(|_| PlanError::Fault(AuthorityFault::ResourceProjection))?;
        }
        let indexes = self
            .indexes_for_plan()
            .plan_replacements(
                owners
                    .changes
                    .iter()
                    .map(|change| (&change.key, Some(&change.before), Some(&change.after))),
            )
            .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        let sources = self.source_versions.plan_replacements(
            owners
                .changes
                .iter()
                .map(|change| (Some(&change.before), Some(&change.after))),
            sequence,
        );
        let template_sources = self
            .plan_owner_sources(
                owners
                    .changes
                    .iter()
                    .map(|change| (&change.key, Some(&change.before), Some(&change.after))),
            )
            .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        let scheduler = self
            .scheduler
            .lock()
            .plan_exchange_batch(
                owners
                    .changes
                    .iter()
                    .map(|change| (Some(&change.before), Some(&change.after))),
                exchange_cursor,
            )
            .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        let dependency = self
            .dependencies
            .plan_settlement_replacements(
                owners
                    .changes
                    .iter()
                    .map(|change| (Some(&change.before), Some(&change.after))),
                settlement_evidence,
            )
            .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        let retired = super::retired_buffer(owners.changes.len())?;
        let mut owner_cuts = Vec::new();
        owner_cuts
            .try_reserve(owners.changes.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for change in owners.changes {
            owner_cuts.push(IndependentOwnerCut {
                key: change.key,
                expected: OwnerPrestate::from_owner(&change.before),
                action: IndependentOwnerAction::Replace(Some(change.after)),
            });
        }
        Ok((
            Some(ComputeExchangeDelta {
                owner_cuts,
                owners: DerivedOwnerDelta {
                    indexes,
                    sources,
                    template_sources,
                },
                resources: resource_plan,
                scheduler,
                dependency,
                clocks,
                retired,
            }),
            jobs,
        ))
    }

    fn search_exchange_permit(
        &self,
        owners: &OwnerOverlay,
        resources: &mut OrderedResourceProjection,
        wave: &mut SchedulerExchangeWave,
        permit: WorkPermit,
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<Option<PlannedAssignment>, PlanError> {
        let mut cursor = None;
        for _ in 0..wave.owner_count(permit)? {
            let ticket = match cursor {
                Some(owner) => wave.next_after(permit, owner),
                None => wave.next(permit),
            };
            let Some(ticket) = ticket else {
                return Ok(None);
            };
            cursor = Some(ticket.owner());
            match self.exchange_checkout_resource(
                owners,
                resources,
                &ticket,
                permit,
                blocked_revocation_peers,
            )? {
                Ok(reservation) => {
                    wave.select(&ticket)?;
                    return Ok(Some(PlannedAssignment {
                        permit,
                        ticket,
                        reservation,
                    }));
                }
                Err(CandidateUnavailable::SkipOwner) => {}
                Err(CandidateUnavailable::Stop) => return Ok(None),
            }
        }
        Ok(None)
    }

    fn exchange_checkout_resource(
        &self,
        owners: &OwnerOverlay,
        resources: &mut OrderedResourceProjection,
        ticket: &CheckoutTicket,
        permit: WorkPermit,
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<CandidateCheckout, PlanError> {
        let before = owners.current(self, ticket.hash())?;
        if before.record().version != ticket.version() {
            return Err(PlanError::Stale(super::StalePlan::Version));
        }
        let OwnedTx::PreAccepted(preaccepted) = &before else {
            return Err(PlanError::Stale(super::StalePlan::Phase));
        };
        if preaccepted
            .source
            .ingress_peer()
            .is_some_and(|peer| blocked_revocation_peers.contains(&peer))
        {
            return Ok(Err(CandidateUnavailable::SkipOwner));
        }
        let attribution = preaccepted.source.compute_attribution();
        match resources.active_work_availability(self.resources.read(&self.entries), attribution)? {
            ActiveWorkAvailability::Available => {}
            ActiveWorkAvailability::PeerExhausted(_) => {
                return Ok(Err(CandidateUnavailable::SkipOwner));
            }
            ActiveWorkAvailability::PreAcceptedExhausted
            | ActiveWorkAvailability::RemoteExhausted => {
                return Ok(Err(CandidateUnavailable::Stop));
            }
        }
        let (grant, after_charge) = match self.checkout_eligibility(preaccepted, permit)? {
            CheckoutEligibility::Ready {
                grant,
                after_charge,
            } => (grant, after_charge),
            CheckoutEligibility::StaleDependency => {
                return Ok(Err(CandidateUnavailable::SkipOwner));
            }
        };
        let desired = ChargeRecord::PreAccepted {
            resources: after_charge,
            residency_peer: preaccepted.source.ingress_peer(),
            compute_peer: attribution.peer(),
        };
        match resources.replace(
            self.resources.read(&self.entries),
            Some(before.charge_record()),
            Some(desired),
        ) {
            Ok(()) => Ok(Ok(CheckoutReservation {
                before,
                grant,
                after_charge,
            })),
            Err(
                ResourceError::PreAcceptedLimit
                | ResourceError::RemoteLimit
                | ResourceError::PeerLimit(_),
            ) => Err(PlanError::Fault(AuthorityFault::ResourceProjection)),
            Err(error) => Err(error.into()),
        }
    }
}

fn exchange_failure(
    error: PlanError,
    classified: Vec<ClassifiedCompletion>,
    remaining: Vec<ComputeExchangeCompletion>,
    grants: Vec<ComputeWorkerGrant>,
) -> ComputeExchangePlanFailure {
    ComputeExchangePlanFailure {
        error,
        classified: classified.into_iter(),
        remaining: remaining.into_iter(),
        grants: grants.into_iter(),
    }
}

fn exchange_failure_from_iter(
    error: PlanError,
    classified: Vec<ClassifiedCompletion>,
    remaining: std::vec::IntoIter<ComputeExchangeCompletion>,
    grants: Vec<ComputeWorkerGrant>,
) -> ComputeExchangePlanFailure {
    ComputeExchangePlanFailure {
        error,
        classified: classified.into_iter(),
        remaining,
        grants: grants.into_iter(),
    }
}

#[cfg(test)]
#[path = "../tests/support/compute_exchange.rs"]
pub(super) mod test_support;
