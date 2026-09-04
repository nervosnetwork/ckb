use super::{
    AuthorityFault, CheckoutEligibility, ClockPlanReservation, ConcurrentIndependentError,
    DerivedOwnerDelta, IndependentDelta, IndependentOwnerAction, IndependentOwnerCut,
    OwnerPrestate, PlanError, PreparedIndependentApply, TxPoolAuthority,
};
use crate::authority::{
    dependency::DependencyBatchDelta,
    exchange::{AuthorityComputeExecutionPermit, ComputeWorkerGrant, ComputeWorkerSlot},
    resources::{
        ActiveWorkAvailability, ActiveWorkOperation, ActiveWorkRevision, ChargeRecord,
        OrderedResourceProjection, ResourceBatchPlan, ResourceCapacityObservation, ResourceError,
    },
    runtime::AuthorityFinishedCompute,
    scheduler::{CheckoutTicket, SchedulerBatchDelta, SchedulerExchangeWave},
    state::{EntryVersion, OwnedTx, PreAcceptedPhase, RawTxHash, WorkPermit},
    work::CheckedOutWork,
};
use ckb_network::PeerIndex;
use std::{collections::HashMap, num::NonZeroUsize, sync::Arc};

/// A stable worker slot paired with its only finished settlement capability.
#[derive(Debug)]
#[must_use = "a finished compute slot must be settled or discharged"]
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

    pub(in crate::authority) fn into_parts(self) -> (ComputeWorkerSlot, AuthorityFinishedCompute) {
        (self.slot, self.finished)
    }
}

/// An effect-blocked malformed completion. The planner revalidates it against
/// the current owner cut, so promotion, replacement, and obsolescence remove
/// the peer exclusion without a second policy cache.
#[derive(Debug)]
pub(in crate::authority) struct ComputePeerExclusion {
    hash: RawTxHash,
    version: EntryVersion,
    peer: PeerIndex,
}

impl ComputePeerExclusion {
    pub(in crate::authority) fn from_completion(
        completion: &ComputeExchangeCompletion,
        peer: PeerIndex,
    ) -> Self {
        Self {
            hash: completion.finished.settlement().token.hash.clone(),
            version: completion.version(),
            peer,
        }
    }

    fn current_peer(&self, authority: &TxPoolAuthority) -> Option<PeerIndex> {
        let owner = authority.entries.get(&self.hash)?;
        if owner.record().version != self.version {
            return None;
        }
        let OwnedTx::PreAccepted(preaccepted) = &*owner else {
            return None;
        };
        preaccepted
            .source
            .payload_blame_peer()
            .filter(|peer| *peer == self.peer)
    }
}

#[must_use = "validated worker grants must be checked out or returned"]
pub(in crate::authority) struct ValidatedComputeExchangeInputs {
    grants: Vec<ComputeWorkerGrant>,
}

impl ValidatedComputeExchangeInputs {
    pub(in crate::authority) fn grant_len(&self) -> usize {
        self.grants.len()
    }

    fn into_grants(self) -> Vec<ComputeWorkerGrant> {
        self.grants
    }
}

#[derive(Debug)]
#[must_use = "an assignment owns checked-out work and one execution permit"]
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

    fn into_grant_before_commit(self) -> ComputeWorkerGrant {
        let Self { grant, work } = self;
        drop(work);
        grant
    }
}

#[must_use = "a failed checkout plan still owns every worker grant"]
pub(in crate::authority) struct ComputeExchangePlanFailure {
    error: PlanError,
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

impl ComputeExchangePlanFailure {
    pub(in crate::authority) fn error(&self) -> &PlanError {
        &self.error
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (PlanError, std::vec::IntoIter<ComputeWorkerGrant>) {
        (self.error, self.grants)
    }
}

pub(super) struct ComputeExchangeDelta {
    owner_cuts: Vec<IndependentOwnerCut>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    scheduler: SchedulerBatchDelta,
    dependency: DependencyBatchDelta,
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
    fn new(maximum_changes: usize) -> Self {
        Self {
            positions: HashMap::with_capacity(maximum_changes),
            changes: Vec::with_capacity(maximum_changes),
        }
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

struct ComputeExchangeDraft {
    owners: OwnerOverlay,
    resources: OrderedResourceProjection,
    active_work_revision: ActiveWorkRevision,
    capacity_observation: ResourceCapacityObservation,
    clock_plan: ClockPlanReservation,
    wave: SchedulerExchangeWave,
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

#[must_use = "committed checkout consequences must leave the authority guard"]
pub(in crate::authority) struct CommittedComputeExchange {
    pub(in crate::authority) retirement: Option<super::CommittedDelta>,
    pub(in crate::authority) assignments: Vec<ComputeExchangeAssignment>,
    pub(in crate::authority) unused_grants: Vec<ComputeWorkerGrant>,
}

#[must_use = "a changed checkout cut must return every worker grant"]
pub(in crate::authority) struct RecoveredComputeExchange {
    pub(in crate::authority) unused_grants: Vec<ComputeWorkerGrant>,
}

#[must_use = "a shared checkout terminal must be consumed"]
#[expect(
    clippy::large_enum_variant,
    reason = "the committed arm owns preallocated retirement after irreversible owner mutation; boxing would add a fallible post-commit allocation"
)]
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

#[must_use = "a prepared checkout must Apply or return every grant"]
pub(in crate::authority) struct PreparedSharedComputeExchange<'authority> {
    apply: SharedComputeApply<'authority>,
    assignments: Vec<ComputeExchangeAssignment>,
    unused_grants: Vec<ComputeWorkerGrant>,
    recovery_grants: Vec<ComputeWorkerGrant>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the Apply arm owns one preallocated atomic delta and is consumed immediately; boxing would allocate on every compute checkout"
)]
enum SharedComputeApply<'authority> {
    CommitNoop,
    Apply(PreparedIndependentApply<'authority>),
    RetryProbe,
}

impl PreparedSharedComputeExchange<'_> {
    fn recover(
        assignments: Vec<ComputeExchangeAssignment>,
        unused_grants: Vec<ComputeWorkerGrant>,
        mut recovery_grants: Vec<ComputeWorkerGrant>,
    ) -> RecoveredComputeExchange {
        recovery_grants.extend(
            assignments
                .into_iter()
                .map(ComputeExchangeAssignment::into_grant_before_commit),
        );
        recovery_grants.extend(unused_grants);
        RecoveredComputeExchange {
            unused_grants: recovery_grants,
        }
    }

    pub(in crate::authority) fn apply(self) -> SharedComputeExchangeOutcome {
        let Self {
            apply,
            assignments,
            unused_grants,
            recovery_grants,
        } = self;
        let plan = match apply {
            SharedComputeApply::CommitNoop => {
                return SharedComputeExchangeOutcome::Committed {
                    exchange: CommittedComputeExchange {
                        retirement: None,
                        assignments,
                        unused_grants,
                    },
                    post_commit_fault: None,
                };
            }
            SharedComputeApply::RetryProbe => {
                return SharedComputeExchangeOutcome::RetryProbe(Self::recover(
                    assignments,
                    unused_grants,
                    recovery_grants,
                ));
            }
            SharedComputeApply::Apply(plan) => plan,
        };
        match plan.apply() {
            Ok(committed) => {
                let (retirement, post_commit_fault) = committed.into_parts();
                SharedComputeExchangeOutcome::Committed {
                    exchange: CommittedComputeExchange {
                        retirement: Some(retirement),
                        assignments,
                        unused_grants,
                    },
                    post_commit_fault,
                }
            }
            Err(ConcurrentIndependentError::ChangedCut(_)) => {
                SharedComputeExchangeOutcome::RetryProbe(Self::recover(
                    assignments,
                    unused_grants,
                    recovery_grants,
                ))
            }
            Err(ConcurrentIndependentError::Fault(fault)) => SharedComputeExchangeOutcome::Fault {
                fault,
                recovered: Self::recover(assignments, unused_grants, recovery_grants),
            },
        }
    }
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

    #[cfg(test)]
    pub(in crate::authority) fn apply_compute_exchange(
        &self,
        grants: Vec<ComputeWorkerGrant>,
        exclusions: &[ComputePeerExclusion],
    ) -> Result<CommittedComputeExchange, ComputeExchangePlanFailure> {
        let inputs = self.validate_compute_exchange_inputs(grants)?;
        let prepared = self.prepare_shared_compute_exchange(inputs, exclusions)?;
        Ok(Self::apply_compute_exchange_for_test(prepared))
    }

    #[cfg(test)]
    fn apply_compute_exchange_for_test(
        prepared: PreparedSharedComputeExchange<'_>,
    ) -> CommittedComputeExchange {
        match prepared.apply() {
            SharedComputeExchangeOutcome::Committed {
                exchange,
                post_commit_fault,
            } => {
                assert!(
                    post_commit_fault.is_none(),
                    "the no-interleave production-path oracle cannot hide a post-commit fault"
                );
                exchange
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

    #[cfg(test)]
    pub(in crate::authority) fn apply_compute_exchange_for_owner(
        &self,
        grant: ComputeWorkerGrant,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: WorkPermit,
    ) -> Result<CommittedComputeExchange, ComputeExchangePlanFailure> {
        let slot = grant.slot();
        if slot.primary_permit() != permit && slot.fallback_permit() != Some(permit) {
            return Err(exchange_failure(
                PlanError::Fault(AuthorityFault::SchedulerProjection),
                vec![grant],
            ));
        }
        let (delta, mut jobs) =
            match self.compile_compute_exchange_owner_state(key, expected, permit) {
                Ok(compiled) => compiled,
                Err(error) => return Err(exchange_failure(error, vec![grant])),
            };
        let work = jobs
            .pop()
            .flatten()
            .expect("an exact eligible checkout compiles one job");
        let compiled = CompiledComputeExchange {
            delta,
            assignments: vec![ComputeExchangeAssignment { grant, work }],
            unused_grants: Vec::new(),
        };
        let recovery_grants = Vec::with_capacity(1);
        let prepared = self.prepare_compiled_compute_exchange(compiled, recovery_grants);
        Ok(Self::apply_compute_exchange_for_test(prepared))
    }

    pub(in crate::authority) fn validate_compute_exchange_inputs(
        &self,
        grants: Vec<ComputeWorkerGrant>,
    ) -> Result<ValidatedComputeExchangeInputs, ComputeExchangePlanFailure> {
        if grants.len() > self.resources.limits().active_work_limit() {
            return Err(exchange_failure(
                PlanError::Fault(AuthorityFault::SchedulerProjection),
                grants,
            ));
        }
        let mut slots = grants
            .iter()
            .map(|grant| grant.slot().id())
            .collect::<Vec<_>>();
        slots.sort_unstable();
        if slots
            .array_windows::<2>()
            .any(|[left, right]| left == right)
        {
            return Err(exchange_failure(
                PlanError::Fault(AuthorityFault::SchedulerProjection),
                grants,
            ));
        }
        Ok(ValidatedComputeExchangeInputs { grants })
    }

    pub(in crate::authority) fn prepare_shared_compute_exchange(
        &self,
        inputs: ValidatedComputeExchangeInputs,
        exclusions: &[ComputePeerExclusion],
    ) -> Result<PreparedSharedComputeExchange<'_>, ComputeExchangePlanFailure> {
        let grants = inputs.into_grants();
        let recovery_grants = Vec::with_capacity(grants.len());
        let mut blocked_revocation_peers = Vec::with_capacity(exclusions.len());
        for peer in exclusions
            .iter()
            .filter_map(|exclusion| exclusion.current_peer(self))
        {
            if !blocked_revocation_peers.contains(&peer) {
                blocked_revocation_peers.push(peer);
            }
        }
        match self.compile_compute_exchange_inner(grants, &blocked_revocation_peers) {
            Ok(compiled) => Ok(self.prepare_compiled_compute_exchange(compiled, recovery_grants)),
            Err(ComputeExchangeCompileFailure {
                error: PlanError::Stale(_),
                grants,
            }) => Ok(PreparedSharedComputeExchange {
                apply: SharedComputeApply::RetryProbe,
                assignments: Vec::new(),
                unused_grants: grants,
                recovery_grants,
            }),
            Err(ComputeExchangeCompileFailure { error, grants }) => {
                Err(exchange_failure(error, grants))
            }
        }
    }

    fn prepare_compiled_compute_exchange(
        &self,
        compiled: CompiledComputeExchange,
        recovery_grants: Vec<ComputeWorkerGrant>,
    ) -> PreparedSharedComputeExchange<'_> {
        let CompiledComputeExchange {
            delta,
            assignments,
            unused_grants,
        } = compiled;
        let apply = delta.map_or(SharedComputeApply::CommitNoop, |delta| {
            let delta = delta.into_independent();
            let support = delta.physical_support(self);
            SharedComputeApply::Apply(PreparedIndependentApply::Shared {
                authority: self,
                delta,
                support,
                staged_effect: None,
            })
        });
        PreparedSharedComputeExchange {
            apply,
            assignments,
            unused_grants,
            recovery_grants,
        }
    }

    fn compile_compute_exchange_inner(
        &self,
        mut grants: Vec<ComputeWorkerGrant>,
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<CompiledComputeExchange, ComputeExchangeCompileFailure> {
        grants.sort_unstable_by_key(|grant| grant.slot().work_selection_key());
        let mut slots = Vec::with_capacity(grants.len());
        slots.extend(grants.iter().map(ComputeWorkerGrant::slot));
        let mut committed_assignments = Vec::with_capacity(grants.len());
        let mut unused_grants = Vec::with_capacity(grants.len());

        let (delta, work) =
            match self.compile_compute_exchange_state(&slots, blocked_revocation_peers) {
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
        grant_slots: &[ComputeWorkerSlot],
        blocked_revocation_peers: &[PeerIndex],
    ) -> Result<(Option<ComputeExchangeDelta>, Vec<Option<CheckedOutWork>>), PlanError> {
        let mut draft = self.begin_compute_exchange_state(grant_slots.len())?;
        let mut assignments = Vec::with_capacity(grant_slots.len());
        assignments.resize_with(grant_slots.len(), || None);

        // Give every verifier primary lane one complete resource-aware pass
        // before any verifier can consume Resolve fallback capacity. This
        // preserves the Small-only lane and prevents a fallback from blocking
        // an already-runnable Large Verify owned by another verifier.
        for (assignment, slot) in assignments.iter_mut().zip(grant_slots.iter().copied()) {
            if slot.fallback_permit().is_none() {
                continue;
            }
            *assignment = self
                .search_exchange_permit(
                    &draft.owners,
                    &mut draft.resources,
                    &mut draft.wave,
                    slot.primary_permit(),
                    blocked_revocation_peers,
                )
                .map_err(|error| self.compute_exchange_derived_error(&draft.owners, error))?;
        }
        for (assignment, slot) in assignments.iter_mut().zip(grant_slots.iter().copied()) {
            if assignment.is_some() {
                continue;
            }
            let Some(fallback) = slot.fallback_permit() else {
                continue;
            };
            *assignment = self
                .search_exchange_permit(
                    &draft.owners,
                    &mut draft.resources,
                    &mut draft.wave,
                    fallback,
                    blocked_revocation_peers,
                )
                .map_err(|error| self.compute_exchange_derived_error(&draft.owners, error))?;
        }
        for (assignment, slot) in assignments.iter_mut().zip(grant_slots.iter().copied()) {
            if slot.fallback_permit().is_some() {
                continue;
            }
            *assignment = self
                .search_exchange_permit(
                    &draft.owners,
                    &mut draft.resources,
                    &mut draft.wave,
                    slot.primary_permit(),
                    blocked_revocation_peers,
                )
                .map_err(|error| self.compute_exchange_derived_error(&draft.owners, error))?;
        }

        self.finish_compute_exchange_state(draft, assignments)
    }

    fn begin_compute_exchange_state(
        &self,
        transition_bound: usize,
    ) -> Result<ComputeExchangeDraft, PlanError> {
        let owners = OwnerOverlay::new(transition_bound);
        let resources = self
            .resources
            .ordered_projection(&self.entries, transition_bound)?;
        let active_work_revision = resources.active_work_revision();
        let capacity_observation = resources.capacity_observation();
        let clock_plan = ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks));
        let wave = SchedulerExchangeWave::after(
            Arc::clone(&self.scheduler),
            owners.changes.iter().map(|change| &change.after),
            transition_bound,
        )
        .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        Ok(ComputeExchangeDraft {
            owners,
            resources,
            active_work_revision,
            capacity_observation,
            clock_plan,
            wave,
        })
    }

    fn finish_compute_exchange_state(
        &self,
        draft: ComputeExchangeDraft,
        assignments: Vec<Option<PlannedAssignment>>,
    ) -> Result<(Option<ComputeExchangeDelta>, Vec<Option<CheckedOutWork>>), PlanError> {
        let grant_count = assignments.len();
        let ComputeExchangeDraft {
            mut owners,
            resources: _,
            active_work_revision,
            capacity_observation,
            clock_plan,
            wave,
        } = draft;

        #[cfg(test)]
        self.entries.enter_compute_exchange_probe(
            crate::authority::shard::ComputeExchangeProbePhase::AfterSchedulerWave,
        );

        let transition_count = assignments
            .iter()
            .filter(|assignment| assignment.is_some())
            .count();
        if transition_count == 0 {
            let mut jobs = Vec::with_capacity(grant_count);
            jobs.resize_with(grant_count, || None);
            return Ok((None, jobs));
        }
        let mut jobs = Vec::with_capacity(grant_count);
        let assignment_count = assignments
            .iter()
            .filter(|assignment| assignment.is_some())
            .count();
        let mut clock_plan = clock_plan;
        let mut assignment_versions =
            if let Some(assignment_count) = NonZeroUsize::new(assignment_count) {
                Some(clock_plan.replacements(assignment_count)?)
            } else {
                None
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

        let mut resource_changes = Vec::with_capacity(owners.changes.len());
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
                Vec::new(),
            )
            .map_err(|error| self.compute_exchange_derived_error(&owners, error))?;
        let retired = super::retired_buffer(owners.changes.len());
        let mut owner_cuts = Vec::with_capacity(owners.changes.len());
        for change in owners.changes {
            owner_cuts.push(IndependentOwnerCut {
                key: change.key,
                expected: OwnerPrestate::from_owner(&change.before),
                removal_revision: None,
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
                retired,
            }),
            jobs,
        ))
    }

    #[cfg(test)]
    fn compile_compute_exchange_owner_state(
        &self,
        key: &RawTxHash,
        expected: EntryVersion,
        permit: WorkPermit,
    ) -> Result<(Option<ComputeExchangeDelta>, Vec<Option<CheckedOutWork>>), PlanError> {
        let mut draft = self.begin_compute_exchange_state(1)?;
        let ticket = self
            .scheduler
            .lock()
            .ticket_for_foundation(key, expected, permit)
            .ok_or(PlanError::Stale(super::StalePlan::Phase))?;
        let reservation = match self.exchange_checkout_resource(
            &draft.owners,
            &mut draft.resources,
            &ticket,
            permit,
            &[],
        )? {
            Ok(reservation) => reservation,
            Err(CandidateUnavailable::SkipOwner) => {
                return Err(PlanError::Stale(super::StalePlan::Dependency));
            }
            Err(CandidateUnavailable::Stop) => {
                return Err(PlanError::Backpressure(
                    super::Backpressure::ComputeResources,
                ));
            }
        };
        draft.wave.select(&ticket)?;
        let assignment = PlannedAssignment {
            permit,
            ticket,
            reservation,
        };
        self.finish_compute_exchange_state(draft, vec![Some(assignment)])
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
    grants: Vec<ComputeWorkerGrant>,
) -> ComputeExchangePlanFailure {
    ComputeExchangePlanFailure {
        error,
        grants: grants.into_iter(),
    }
}
