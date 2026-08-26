use super::apply_seal::{ApplyToken, OwnerResourceUpdate, PreparedOwnerResourceDelta};
use super::{
    ApplyClockReservation, ApplyRetirement, AuthorityClocks, AuthorityDelta, AuthorityFault,
    CheckoutEligibility, DerivedOwnerDelta, OwnerLocalSettlement, PlanError, PreparedApply,
    SettlementClassification, TxPoolAuthority,
};
use crate::authority::{
    dependency::DependencyBatchDelta,
    exchange::{AuthorityComputeExecutionPermit, ComputeWorkerGrant, ComputeWorkerSlot},
    resources::{
        ActiveWorkAvailability, ChargeRecord, OrderedResourceProjection, ResourceBatchPlan,
        ResourceError,
    },
    runtime::{AuthorityComputeAftermath, AuthorityFinishedCompute},
    scheduler::{CheckoutTicket, SchedulerBatchDelta, SchedulerExchangeWave},
    state::{EntryVersion, OwnedTx, PreAcceptedPhase, RawTxHash, WorkPermit},
    work::{CheckedOutWork, ComputeSettlement, SettlementNext, SettlementToken},
};
use ckb_network::PeerIndex;
use std::{collections::HashMap, num::NonZeroUsize};

/// One finished worker slot and the exact move-only settlement evidence it
/// owns. The coordinator may submit the value to one exchange or retain it;
/// it cannot separate slot availability from capability settlement.
#[derive(Debug)]
#[must_use = "a finished compute slot must be exchanged, settled, or discharged"]
pub(in crate::authority) struct ComputeExchangeCompletion {
    slot: ComputeWorkerSlot,
    finished: AuthorityFinishedCompute,
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
}

enum ClassifiedCompletion {
    OwnerLocal {
        slot: ComputeWorkerSlot,
        token: SettlementToken,
        ingress_peer: Option<PeerIndex>,
        settlement: Option<OwnerLocalSettlement>,
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
            Self::Obsolete { slot } => sink.recover_obsolete(slot),
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

struct ComputeExchangeUpdate {
    key: RawTxHash,
    after: OwnedTx,
}

pub(super) struct ComputeExchangeDelta {
    updates: Vec<ComputeExchangeUpdate>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
    clocks: AuthorityClocks,
    retired: super::RetiredOwners,
}

impl ComputeExchangeDelta {
    pub(super) fn releases_preaccepted_active_work(&self) -> bool {
        self.resources.releases_preaccepted_active_work()
    }
}

pub(super) fn apply_compute_exchange(
    authority: &mut TxPoolAuthority,
    token: &ApplyToken,
    delta: ComputeExchangeDelta,
) -> ApplyRetirement {
    let mut retired = delta.retired;
    let status_counts = crate::authority::shard::ShardStatusCountPlan::default();
    let support = authority.entries.owner_resource_write_support(
        delta.updates.iter().map(|update| &update.key),
        &status_counts,
        delta.resources.shard_plan(),
    );
    let updates = delta
        .updates
        .into_iter()
        .map(|update| OwnerResourceUpdate::new(update.key, Some(update.after)));
    authority.commit_owner_resources(
        token,
        PreparedOwnerResourceDelta::batch(updates, delta.resources, status_counts, support),
        &mut retired,
    );
    let authority = authority.write(token);
    authority.indexes.apply(delta.owners.indexes);
    authority.source_versions.apply(delta.owners.sources);
    authority.scheduler.apply_batch(delta.scheduler);
    authority.dependencies.apply_batch(delta.dependency);
    let _reserved_clock_high_water = delta.clocks;
    ApplyRetirement {
        async_process_observations: super::AsyncProcessObservations::None,
        removals: Vec::new(),
        retired,
        retired_effect: None,
        retired_generation: None,
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

    fn replace(
        &mut self,
        authority: &TxPoolAuthority,
        key: RawTxHash,
        after: OwnedTx,
    ) -> Result<(), PlanError> {
        if let Some(position) = self.positions.get(&key).copied() {
            let change = self
                .changes
                .get_mut(position)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            change.after = after;
            return Ok(());
        }
        let before = authority
            .entries
            .get(&key)
            .as_deref()
            .cloned()
            .ok_or(PlanError::Stale(super::StalePlan::Missing))?;
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

struct CompiledComputeExchange<'authority> {
    plan: Option<PreparedApply<'authority>>,
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

/// Stack-local one-stamp exchange. A successful Plan is deliberately not
/// exposed to callers: it already owns the unique completion capabilities and
/// must Apply before this stack frame can return. Only planning failure may
/// return those capabilities unchanged.
struct PreparedComputeExchange<'authority> {
    plan: Option<PreparedApply<'authority>>,
    classified: Vec<ClassifiedCompletion>,
    exclusive_settled: Option<ComputeExchangeSettled>,
    assignments: Vec<ComputeExchangeAssignment>,
    unused_grants: Vec<ComputeWorkerGrant>,
    outcomes: ComputeExchangeOutcomeBuffers,
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

impl PreparedComputeExchange<'_> {
    pub(in crate::authority) fn apply(self) -> CommittedComputeExchange {
        let retirement = self.plan.map(PreparedApply::apply);
        let ComputeExchangeOutcomeBuffers {
            mut settled,
            mut obsolete,
            mut deferred,
        } = self.outcomes;
        if let Some(completion) = self.exclusive_settled {
            settled.push(completion);
        }
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
                ClassifiedCompletion::Obsolete { slot } => obsolete.push(slot),
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

fn provisional_version(first: EntryVersion, offset: usize) -> Result<EntryVersion, PlanError> {
    let offset =
        u128::try_from(offset).map_err(|_| PlanError::Fault(AuthorityFault::CounterExhausted))?;
    first
        .0
        .checked_add(offset)
        .map(EntryVersion)
        .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))
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
    let previous = std::mem::replace(member, ClassifiedCompletion::Obsolete { slot });
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

fn defer_completion(
    completion: ComputeExchangeCompletion,
    route: ComputeExchangeDeferredRoute,
) -> ClassifiedCompletion {
    let ComputeExchangeCompletion { slot, finished } = completion;
    let (settlement, aftermath) = finished.into_parts();
    ClassifiedCompletion::Deferred {
        slot,
        settlement,
        aftermath,
        route,
    }
}

impl TxPoolAuthority {
    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    pub(in crate::authority) fn apply_compute_exchange(
        &mut self,
        completions: Vec<ComputeExchangeCompletion>,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<CommittedComputeExchange, ComputeExchangePlanFailure> {
        self.prepare_compute_exchange(completions, grants)
            .map(PreparedComputeExchange::apply)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    fn prepare_compute_exchange(
        &mut self,
        mut completions: Vec<ComputeExchangeCompletion>,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<PreparedComputeExchange<'_>, ComputeExchangePlanFailure> {
        let outcomes = match ComputeExchangeOutcomeBuffers::new(completions.len()) {
            Ok(outcomes) => outcomes,
            Err(error) => {
                return Err(exchange_failure(error, Vec::new(), completions, grants));
            }
        };
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

        let mut classified = Vec::new();
        if classified.try_reserve(completions.len()).is_err() {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                classified,
                completions,
                grants,
            ));
        }
        let mut blocked_revocation_peers = Vec::new();
        if blocked_revocation_peers
            .try_reserve(completion_slots.len())
            .is_err()
        {
            return Err(exchange_failure(
                PlanError::Backpressure(super::Backpressure::Allocation),
                classified,
                completions,
                grants,
            ));
        }
        let mut blocked_revocation = false;
        let mut remaining = completions.into_iter();
        while let Some(completion) = remaining.next() {
            let ComputeExchangeCompletion { slot, finished } = completion;
            let (settlement, aftermath) = finished.into_parts();
            let ComputeSettlement { token, next } = settlement;
            let existing = match self.entries.get(&token.hash).as_deref().cloned() {
                Some(existing) if existing.record().version == token.version => existing,
                Some(_) | None => {
                    drop(next);
                    classified.push(ClassifiedCompletion::Obsolete { slot });
                    continue;
                }
            };
            let OwnedTx::PreAccepted(preaccepted) = &existing else {
                drop(next);
                classified.push(ClassifiedCompletion::Obsolete { slot });
                continue;
            };
            let PreAcceptedPhase::Computing(active) = &preaccepted.phase else {
                drop(next);
                classified.push(ClassifiedCompletion::Obsolete { slot });
                continue;
            };
            if preaccepted.charge.active_work != 1 {
                classified.push(ClassifiedCompletion::OwnerLocal {
                    slot,
                    token,
                    ingress_peer: preaccepted.source.ingress_peer(),
                    settlement: None,
                    aftermath,
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
            match self.classify_settlement(preaccepted, active, next) {
                Ok(SettlementClassification::OwnerLocal(settlement)) => {
                    classified.push(ClassifiedCompletion::OwnerLocal {
                        slot,
                        token,
                        ingress_peer: preaccepted.source.ingress_peer(),
                        settlement: Some(settlement),
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
                    let Some(peer) = revocation_peer else {
                        classified.push(ClassifiedCompletion::Deferred {
                            slot,
                            settlement: ComputeSettlement {
                                token,
                                next: deferred_next(nonlocal),
                            },
                            aftermath,
                            route: ComputeExchangeDeferredRoute::ExactSettlement,
                        });
                        continue;
                    };
                    let rejection = match nonlocal {
                        super::NonLocalSettlement::Rejected(rejection) => rejection,
                        super::NonLocalSettlement::VerificationRejected {
                            rejection,
                            resolved,
                        } => {
                            drop(resolved);
                            crate::authority::work::SettlementRejection::ChainBound(rejection)
                        }
                        super::NonLocalSettlement::Waiting(missing) => {
                            classified.push(ClassifiedCompletion::Deferred {
                                slot,
                                settlement: ComputeSettlement {
                                    token,
                                    next: SettlementNext::Waiting(missing),
                                },
                                aftermath,
                                route: ComputeExchangeDeferredRoute::ExactSettlement,
                            });
                            continue;
                        }
                    };
                    let retry = rejection.clone();
                    if blocked_revocation {
                        if !blocked_revocation_peers.contains(&peer) {
                            blocked_revocation_peers.push(peer);
                        }
                        classified.push(ClassifiedCompletion::Deferred {
                            slot,
                            settlement: ComputeSettlement {
                                token,
                                next: SettlementNext::Rejected(retry),
                            },
                            aftermath,
                            route: ComputeExchangeDeferredRoute::ExchangeAfterEffect,
                        });
                        continue;
                    }
                    match self.compile_peer_revocation(
                        peer,
                        token.hash.clone(),
                        rejection.into_public(),
                    ) {
                        Ok(delta) => {
                            for member in &mut classified {
                                defer_owner_local(
                                    member,
                                    None,
                                    ComputeExchangeDeferredRoute::ExchangeRetry,
                                );
                            }
                            classified.extend(remaining.map(|completion| {
                                defer_completion(
                                    completion,
                                    ComputeExchangeDeferredRoute::ExchangeRetry,
                                )
                            }));
                            return Ok(PreparedComputeExchange {
                                plan: Some(PreparedApply {
                                    authority: self,
                                    delta: AuthorityDelta::Admin(delta),
                                }),
                                classified,
                                exclusive_settled: Some(ComputeExchangeSettled { slot, aftermath }),
                                assignments: Vec::new(),
                                unused_grants: grants,
                                outcomes,
                            });
                        }
                        Err(PlanError::Backpressure(super::Backpressure::EffectCapacity)) => {
                            blocked_revocation = true;
                            blocked_revocation_peers.push(peer);
                            for member in &mut classified {
                                defer_owner_local(
                                    member,
                                    Some(peer),
                                    ComputeExchangeDeferredRoute::ExchangeAfterEffect,
                                );
                            }
                            classified.push(ClassifiedCompletion::Deferred {
                                slot,
                                settlement: ComputeSettlement {
                                    token,
                                    next: SettlementNext::Rejected(retry),
                                },
                                aftermath,
                                route: ComputeExchangeDeferredRoute::ExchangeAfterEffect,
                            });
                        }
                        Err(error) => {
                            classified.push(ClassifiedCompletion::Deferred {
                                slot,
                                settlement: ComputeSettlement {
                                    token,
                                    next: SettlementNext::Rejected(retry),
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
                Err(error) => {
                    classified.push(ClassifiedCompletion::OwnerLocal {
                        slot,
                        token,
                        ingress_peer: preaccepted.source.ingress_peer(),
                        settlement: None,
                        aftermath,
                    });
                    return Err(exchange_failure_from_iter(
                        error, classified, remaining, grants,
                    ));
                }
            }
        }

        self.compile_compute_exchange(classified, grants, blocked_revocation_peers, outcomes)
    }

    #[expect(
        clippy::result_large_err,
        reason = "the error is the linear recovery carrier for every bounded completion and worker grant"
    )]
    fn compile_compute_exchange(
        &mut self,
        mut classified: Vec<ClassifiedCompletion>,
        grants: Vec<ComputeWorkerGrant>,
        blocked_revocation_peers: Vec<PeerIndex>,
        outcomes: ComputeExchangeOutcomeBuffers,
    ) -> Result<PreparedComputeExchange<'_>, ComputeExchangePlanFailure> {
        match self.compile_compute_exchange_inner(
            &mut classified,
            grants,
            &blocked_revocation_peers,
        ) {
            Ok(CompiledComputeExchange {
                plan,
                assignments,
                unused_grants,
            }) => Ok(PreparedComputeExchange {
                plan,
                classified,
                exclusive_settled: None,
                assignments,
                unused_grants,
                outcomes,
            }),
            Err(ComputeExchangeCompileFailure { error, grants }) => {
                Err(exchange_failure(error, classified, Vec::new(), grants))
            }
        }
    }

    fn compile_compute_exchange_inner<'authority>(
        &'authority mut self,
        classified: &mut [ClassifiedCompletion],
        mut grants: Vec<ComputeWorkerGrant>,
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<CompiledComputeExchange<'authority>, ComputeExchangeCompileFailure> {
        grants.sort_unstable_by_key(|grant| grant.slot().canonical_key());
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

        let (plan, work) =
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
            plan,
            assignments: committed_assignments,
            unused_grants,
        })
    }

    fn compile_compute_exchange_state<'authority>(
        &'authority mut self,
        classified: &mut [ClassifiedCompletion],
        grant_slots: &[ComputeWorkerSlot],
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<
        (
            Option<PreparedApply<'authority>>,
            Vec<Option<CheckedOutWork>>,
        ),
        PlanError,
    > {
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
        let first_version = self.clocks.snapshot().next_version;

        let mut local_count = 0usize;
        for member in classified.iter_mut() {
            let slot = match member {
                ClassifiedCompletion::OwnerLocal { slot, .. } => *slot,
                ClassifiedCompletion::Deferred { .. } | ClassifiedCompletion::Obsolete { .. } => {
                    continue;
                }
            };
            let ClassifiedCompletion::OwnerLocal {
                slot,
                token,
                ingress_peer,
                settlement,
                aftermath,
            } = std::mem::replace(member, ClassifiedCompletion::Obsolete { slot })
            else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let Some(local) = settlement else {
                *member = ClassifiedCompletion::OwnerLocal {
                    slot,
                    token,
                    ingress_peer,
                    settlement: None,
                    aftermath,
                };
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let before = match self.entries.get(&token.hash).as_deref().cloned() {
                Some(before) => before,
                None => {
                    *member = ClassifiedCompletion::OwnerLocal {
                        slot,
                        token,
                        ingress_peer,
                        settlement: Some(local),
                        aftermath,
                    };
                    return Err(PlanError::Stale(super::StalePlan::Missing));
                }
            };
            let desired_charge = ChargeRecord::PreAccepted {
                resources: local.charge,
                residency_peer: ingress_peer,
                compute_peer: None,
            };
            match resources.replace(
                self.resources.read(&self.entries),
                Some(before.charge_record()),
                Some(desired_charge),
            ) {
                Ok(()) => {}
                Err(
                    ResourceError::PreAcceptedLimit
                    | ResourceError::RemoteLimit
                    | ResourceError::PeerLimit(_),
                ) => {
                    let next = local.into_exact_next();
                    *member = ClassifiedCompletion::Deferred {
                        slot,
                        settlement: ComputeSettlement { token, next },
                        aftermath,
                        route: ComputeExchangeDeferredRoute::ExactSettlement,
                    };
                    continue;
                }
                Err(error) => {
                    *member = ClassifiedCompletion::OwnerLocal {
                        slot,
                        token,
                        ingress_peer,
                        settlement: Some(local),
                        aftermath,
                    };
                    return Err(error.into());
                }
            }
            let version = provisional_version(first_version, local_count)?;
            local_count = local_count
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let after = before
                .with_preaccepted_phase(local.phase.into_preaccepted(), version, local.charge)
                .map_err(PlanError::Stale)?;
            owners.replace(self, token.hash.clone(), after)?;
            *member = ClassifiedCompletion::OwnerLocal {
                slot,
                token,
                ingress_peer,
                settlement: None,
                aftermath,
            };
        }

        let mut wave = self.scheduler.exchange_wave_after(
            owners.changes.iter().map(|change| &change.after),
            grant_slots.len(),
        )?;
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

        for slot in grant_slots.iter().copied() {
            if blocked_slots.binary_search(&slot.id()).is_ok() {
                assignments.push(None);
                continue;
            }
            let selected = match self.search_exchange_permit(
                &owners,
                &mut resources,
                &mut wave,
                slot.primary_permit(),
                blocked_revocation_peers,
            )? {
                Some(assignment) => Some(assignment),
                None => match slot.fallback_permit() {
                    Some(permit) => self.search_exchange_permit(
                        &owners,
                        &mut resources,
                        &mut wave,
                        permit,
                        blocked_revocation_peers,
                    )?,
                    None => None,
                },
            };
            assignments.push(selected);
        }

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
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let (mut versions, clocks) = clocks.replacements(
            NonZeroUsize::new(transition_count)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?,
        )?;
        for _ in 0..local_count {
            let _ = versions.next();
        }

        let mut jobs = Vec::new();
        jobs.try_reserve(grant_slots.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for assignment in assignments {
            let Some(assignment) = assignment else {
                jobs.push(None);
                continue;
            };
            let PlannedAssignment {
                permit,
                ticket,
                reservation,
            } = assignment;
            let version = versions
                .next()
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
            let key = ticket.hash().clone();
            let OwnedTx::PreAccepted(preaccepted) = &reservation.before else {
                return Err(PlanError::Fault(AuthorityFault::SchedulerProjection));
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
            let after = reservation
                .before
                .with_preaccepted_phase(
                    PreAcceptedPhase::Computing(active),
                    version,
                    reservation.after_charge,
                )
                .map_err(PlanError::Stale)?;
            owners.replace(self, key, after)?;
            jobs.push(Some(work));
        }

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
        let resources = self.resources_for_plan().plan_batch(resource_changes)?;
        let indexes = self.indexes_for_plan().plan_replacements(
            owners
                .changes
                .iter()
                .map(|change| (&change.key, Some(&change.before), Some(&change.after))),
        )?;
        let sources = self.source_versions.plan_replacements(
            owners
                .changes
                .iter()
                .map(|change| (Some(&change.before), Some(&change.after))),
            sequence,
        );
        let scheduler = self.scheduler.plan_exchange_batch(
            owners
                .changes
                .iter()
                .map(|change| (Some(&change.before), Some(&change.after))),
            exchange_cursor,
        )?;
        let dependency = self.dependencies.plan_replacements(
            owners
                .changes
                .iter()
                .map(|change| (Some(&change.before), Some(&change.after))),
        )?;
        let retired = super::retired_buffer(owners.changes.len())?;
        let mut updates = Vec::new();
        updates
            .try_reserve(owners.changes.len())
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        for change in owners.changes {
            updates.push(ComputeExchangeUpdate {
                key: change.key,
                after: change.after,
            });
        }
        Ok((
            Some(PreparedApply {
                authority: self,
                delta: AuthorityDelta::ComputeExchange(ComputeExchangeDelta {
                    updates,
                    owners: DerivedOwnerDelta { indexes, sources },
                    resources,
                    scheduler,
                    dependency,
                    clocks: clocks.finish(),
                    retired,
                }),
            }),
            jobs,
        ))
    }

    fn search_exchange_permit(
        &self,
        owners: &OwnerOverlay,
        resources: &mut OrderedResourceProjection,
        wave: &mut SchedulerExchangeWave<'_>,
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
            return Err(PlanError::Fault(AuthorityFault::SchedulerProjection));
        }
        let OwnedTx::PreAccepted(preaccepted) = &before else {
            return Err(PlanError::Fault(AuthorityFault::SchedulerProjection));
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
