use super::apply_seal::ApplyToken;
use super::{
    ApplyClockReservation, AuthorityFault, ClockPlanReservation, CompiledSharedOwnerRemoval,
    ConcurrentIndependentError, DerivedOwnerDelta, IndependentDelta, IndependentOwnerAction,
    IndependentOwnerCut, OwnerPrestate, OwnerRemovalKeys, PlanError, PreparedIndependentApply,
    PreparedSharedOwnerRemoval, TxPoolAuthority,
};
use crate::authority::{
    dependency::{DependencyApplyOutcome, DependencyBatchDelta, PreparedDependencyBatch},
    effect::{
        CommittedAcceptance, CommittedEffect, CommittedPeerCohortRevocation, CommittedRejection,
        CommittedRemoteIngressRelease, EffectBuildError, EffectDelta, EffectLog, EffectPolicy,
        OrderedEffectAppendError, OrderedEffectPublication, RejectionAudience, StagedEffect,
    },
    indexes::RetainedIndexPremise,
    ingress::{
        RemoteIngressPressure, RetainedAdmissionBatch, RetainedIngressAttempt, RetainedIngressKind,
    },
    rejection::CommittedPublicReject,
    resources::{ChargeRecord, OrderedResourceEnvelope, OrderedResourceProjection, ResourceError},
    scheduler::{SchedulerBatchDelta, StagedSchedulerBatch},
    shard::{
        DependencyGateCut, DependencyGateSupport, OwnerShardRemovalRevision,
        ShardedDependencyRelationWriteCut, ShardedOwnerMap, ShardedOwnerWriteCut,
        StagedPeerIngressFence,
    },
    state::{
        AdmissionBasis, ApplySequence, ChainViewId, OwnedTx, PoolGeneration, PreAcceptedEntry,
        PreAcceptedPhase, PreAcceptedSource, ProposalBase, ProposalId, QueuedWork, RawTxHash,
        TxRecord, ValidatedAdmission,
    },
    work::ComputeSettlement,
};
use ckb_network::PeerIndex;
use std::collections::HashMap;
use std::time::Instant;

/// Linear owner for the prepared dependency delta, scheduler batch and gates in
/// one shared retained-ingress commit.
pub(super) struct StagedRetainedIngress<'authority> {
    dependency: PreparedDependencyBatch,
    scheduler: StagedSchedulerBatch<'authority>,
    gates: DependencyGateCut<'authority>,
}

impl<'authority> StagedRetainedIngress<'authority> {
    pub(super) fn stage(
        authority: &'authority TxPoolAuthority,
        scheduler: SchedulerBatchDelta,
        dependency: DependencyBatchDelta,
        gate_support: DependencyGateSupport,
    ) -> Result<Self, ConcurrentRetainedIngressError> {
        // Stage the scheduler before acquiring dependency gates so scheduler
        // allocation and lock work cannot lengthen the gate-held interval.
        let scheduler =
            StagedSchedulerBatch::stage_primary_replacements(&authority.scheduler, scheduler)
                .map_err(|error| match error {
                    crate::authority::scheduler::SchedulerError::Stale => {
                        ConcurrentRetainedIngressError::Stale
                    }
                    crate::authority::scheduler::SchedulerError::Projection
                    | crate::authority::scheduler::SchedulerError::Arithmetic => {
                        ConcurrentRetainedIngressError::Fault(AuthorityFault::SchedulerProjection)
                    }
                })?;
        let gates = authority.entries.dependency_gate_cut(gate_support);
        let dependency = PreparedDependencyBatch::prepare_with_gates(
            &authority.dependencies,
            dependency,
            &gates,
        )
        .map_err(|error| match error {
            crate::authority::dependency::DependencyPrepareError::Stale => {
                ConcurrentRetainedIngressError::Stale
            }
            crate::authority::dependency::DependencyPrepareError::Projection => {
                ConcurrentRetainedIngressError::Fault(AuthorityFault::DependencyProjection)
            }
        })?;
        Ok(Self {
            dependency,
            scheduler,
            gates,
        })
    }

    #[cfg(test)]
    pub(super) fn extend_final_write_support(
        &self,
        support: &mut crate::authority::shard::ShardWriteSupport,
    ) {
        self.dependency.extend_final_write_support(support);
    }

    pub(super) fn extend_final_support(
        &self,
        reads: &mut crate::authority::shard::ShardReadSupport,
        writes: &mut crate::authority::shard::ShardWriteSupport,
    ) {
        self.dependency.extend_final_read_support(reads);
        self.dependency.extend_final_write_support(writes);
    }

    pub(super) fn extend_final_relation_support(
        &self,
        reads: &mut crate::authority::shard::ShardReadSupport,
        writes: &mut crate::authority::shard::ShardWriteSupport,
    ) {
        self.dependency.extend_final_relation_read_support(reads);
        self.dependency.extend_final_relation_write_support(writes);
    }

    pub(super) fn prestate_is_fresh(
        &self,
        relations: &ShardedDependencyRelationWriteCut<'_>,
        owners: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.dependency.prestate_is_fresh(relations, owners)
    }

    pub(super) fn dependency_gates(&self) -> &DependencyGateCut<'_> {
        &self.gates
    }

    pub(super) fn activate(
        self,
        _entries: &ShardedOwnerMap,
        token: &ApplyToken,
        mut relations: ShardedDependencyRelationWriteCut<'_>,
        mut owners: ShardedOwnerWriteCut<'_>,
    ) -> DependencyApplyOutcome {
        let Self {
            dependency,
            scheduler,
            gates: _gates,
        } = self;
        let outcome = dependency.apply_in_cut(&mut relations, &mut owners);
        drop(relations);
        #[cfg(test)]
        _entries.enter_shared_owner_commit_probe();
        scheduler.activate(token, owners);
        outcome
    }

    pub(super) fn scheduler_wake_before(
        &self,
    ) -> Result<crate::authority::scheduler::SchedulerWakeProjection, ConcurrentRetainedIngressError>
    {
        self.scheduler
            .wake_projection_before()
            .ok_or(ConcurrentRetainedIngressError::Fault(
                AuthorityFault::SchedulerProjection,
            ))
    }
}

#[must_use = "compiled shared retained ingress must bind to the live generation or be discarded"]
pub(in crate::authority) struct CompiledSharedRetainedAdmissionBatch {
    generation: PoolGeneration,
    chain_view: ChainViewId,
    delta: IndependentDelta,
    consumed: usize,
}

#[must_use = "a bound shared retained-ingress batch must be applied exactly once"]
pub(in crate::authority) struct PreparedSharedRetainedAdmissionBatch<'authority> {
    authority: &'authority TxPoolAuthority,
    delta: IndependentDelta,
    consumed: usize,
}

enum SharedRetainedEffectPlan {
    Unchanged,
    Publication {
        staged: super::super::effect::StagedEffect,
    },
}

#[must_use = "a prepared shared retained effect prefix must be applied exactly once"]
pub(in crate::authority) struct PreparedSharedRetainedEffectPrefix<'authority> {
    authority: &'authority TxPoolAuthority,
    plan: SharedRetainedEffectPlan,
    consumed: usize,
    read_cut: ShardedOwnerWriteCut<'authority>,
}

#[must_use = "a staged peer-cohort revocation must commit its exact cohort or roll back every hidden capability"]
pub(in crate::authority) struct PreparedSharedPeerRevocation<'authority> {
    core: PreparedSharedPeerRevocationCore<'authority>,
    consumed: usize,
}

pub(in crate::authority) type PreparedSharedPeerRevocationCore<'authority> =
    PreparedSharedOwnerRemoval<'authority, StagedPeerIngressFence<'authority>>;

#[derive(Debug)]
pub(in crate::authority) enum ConcurrentRetainedIngressError {
    Stale,
    Fault(AuthorityFault),
}

impl ConcurrentRetainedIngressError {
    fn from_independent(error: ConcurrentIndependentError) -> Self {
        match error {
            ConcurrentIndependentError::ChangedCut(_) => Self::Stale,
            ConcurrentIndependentError::Fault(fault) => Self::Fault(fault),
        }
    }
}

#[must_use = "a failed peer revocation returns the exact staged-effect capacity wake"]
#[derive(Debug)]
pub(in crate::authority) struct ConcurrentOwnerRemovalFailure {
    error: ConcurrentRetainedIngressError,
    effect_wake: Option<super::super::effect::EffectWakeTransition>,
}

pub(in crate::authority) type ConcurrentPeerRevocationFailure = ConcurrentOwnerRemovalFailure;

impl ConcurrentOwnerRemovalFailure {
    pub(in crate::authority) fn into_parts(
        self,
    ) -> (
        ConcurrentRetainedIngressError,
        Option<super::super::effect::EffectWakeTransition>,
    ) {
        (self.error, self.effect_wake)
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "the transient Applied variant carries the move-only post-commit capability without adding one heap allocation to every successful ingress batch"
)]
pub(in crate::authority) enum CommittedRetainedAdmissionBatch {
    Unchanged {
        consumed: usize,
    },
    Applied {
        retirement: super::CommittedDelta,
        consumed: usize,
    },
}

impl CompiledSharedRetainedAdmissionBatch {
    pub(in crate::authority) fn bind(
        self,
        authority: &TxPoolAuthority,
    ) -> Result<PreparedSharedRetainedAdmissionBatch<'_>, ConcurrentRetainedIngressError> {
        if authority.generation != self.generation || authority.chain_view != self.chain_view {
            return Err(ConcurrentRetainedIngressError::Stale);
        }
        Ok(PreparedSharedRetainedAdmissionBatch {
            authority,
            delta: self.delta,
            consumed: self.consumed,
        })
    }

    #[cfg(test)]
    pub(in crate::authority) fn is_compatible_with_for_foundation(
        &self,
        authority: &TxPoolAuthority,
        other: &Self,
    ) -> bool {
        self.delta
            .physical_support(authority)
            .is_compatible(other.delta.physical_support(authority))
            && self
                .delta
                .dependency_gate_support_for_foundation(authority)
                .is_compatible(
                    other
                        .delta
                        .dependency_gate_support_for_foundation(authority),
                )
    }
}

impl PreparedSharedRetainedAdmissionBatch<'_> {
    pub(in crate::authority) fn apply(
        self,
    ) -> Result<CommittedRetainedAdmissionBatch, ConcurrentRetainedIngressError> {
        let Self {
            authority,
            delta,
            consumed,
        } = self;
        if !delta.is_shared_retained_owner_only_shape(consumed) {
            return Err(ConcurrentRetainedIngressError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let support = delta.physical_support(authority);
        let retirement = PreparedIndependentApply::Shared {
            authority,
            delta,
            support,
            staged_effect: None,
        }
        .apply()
        .map_err(ConcurrentRetainedIngressError::from_independent)?;
        Ok(CommittedRetainedAdmissionBatch::Applied {
            retirement,
            consumed,
        })
    }
}

impl<C> CompiledSharedOwnerRemoval<C> {
    /// Bind every fallible live projection before the irreversible mixed owner
    /// cut. Scheduler staging and dependency preparation precede optional
    /// effect staging, so a Bind failure never abandons a charged effect record
    /// without its explicit owner.
    pub(in crate::authority) fn bind(
        mut self,
        authority: &TxPoolAuthority,
    ) -> Result<PreparedSharedOwnerRemoval<'_, C>, PlanError> {
        if authority.generation != self.generation || authority.chain_view != self.chain_view {
            return Err(PlanError::Stale(super::StalePlan::Version));
        }
        if self.publication.is_none() {
            authority.effects.lock().ensure_open()?;
        }
        let scheduler = std::mem::take(&mut self.removal.scheduler);
        let dependency = std::mem::take(&mut self.removal.dependency);
        let mut gate_support = dependency.dependency_gate_support(&authority.entries);
        gate_support.include(
            self.removal
                .membership
                .dependency_gate_support(&authority.entries),
        );
        let projections =
            StagedRetainedIngress::stage(authority, scheduler, dependency, gate_support).map_err(
                |error| match error {
                    ConcurrentRetainedIngressError::Stale => {
                        PlanError::Stale(super::StalePlan::Version)
                    }
                    ConcurrentRetainedIngressError::Fault(fault) => PlanError::Fault(fault),
                },
            )?;
        let staged_effect = self
            .publication
            .as_ref()
            .map(|publication| {
                let effect = authority
                    .effects_for_plan()
                    .plan_publication(publication, self.sequence)?;
                EffectLog::stage_publication(&authority.effects, effect).map_err(PlanError::from)
            })
            .transpose()?;
        Ok(PreparedSharedOwnerRemoval {
            authority,
            removal: self.removal,
            projections,
            staged_effect,
            control: self.control,
        })
    }
}

impl<C> PreparedSharedOwnerRemoval<'_, C>
where
    C: super::apply_seal::SharedOwnerRemovalControl,
{
    #[cfg(test)]
    pub(in crate::authority) fn apply_for_foundation(self) -> super::CommittedDelta {
        let (committed, post_commit_fault) = self
            .apply()
            .expect("an exclusively held fixture local-removal cut is current")
            .into_parts();
        assert_eq!(post_commit_fault, None);
        committed
    }

    #[cfg(test)]
    pub(in crate::authority) fn physical_write_support_for_foundation(
        &self,
    ) -> crate::authority::shard::ShardWriteSupport {
        let authority = self.authority;
        let mut support = authority.entries.owner_resource_write_support(
            self.removal.hashes.iter(),
            self.removal.membership.proposed_count_plan(),
            self.removal.resources.shard_plan(),
        );
        support.include(
            self.removal
                .owners
                .indexes
                .sharded_write_support(&authority.entries),
        );
        support.include(
            self.removal
                .membership
                .sharded_write_support(&authority.entries),
        );
        self.projections.extend_final_write_support(&mut support);
        let mut reads = crate::authority::shard::ShardReadSupport::default();
        self.control
            .extend_final_support(&authority.entries, &mut reads, &mut support);
        support
    }

    pub(in crate::authority) fn apply(
        self,
    ) -> Result<super::CommittedDelta, ConcurrentOwnerRemovalFailure> {
        super::apply_seal::commit_shared_owner_removal(self)
    }

    pub(super) fn apply_with(
        self,
        token: &ApplyToken,
    ) -> Result<super::CommittedDelta, ConcurrentOwnerRemovalFailure> {
        let Self {
            authority,
            removal,
            projections,
            staged_effect,
            control,
        } = self;
        let compute_slot_released = removal.resources.releases_preaccepted_active_work();
        let before = match projections.scheduler_wake_before() {
            Ok(scheduler) => authority.wake_projection_with_scheduler_without_effect(scheduler),
            Err(error) => return Err(Self::rollback_failure(staged_effect, error)),
        };
        let template_source_changed = removal.owners.template_sources.counts().changed();
        let (dependency, resource_health, retired) = match authority
            .commit_shared_owner_removal_rows(token, removal, projections, control)
        {
            Ok(committed) => committed,
            Err(error) => return Err(Self::rollback_failure(staged_effect, error)),
        };
        let effect_wake = staged_effect.map(StagedEffect::activate_with_wake);
        let retirement = super::ApplyRetirement {
            async_process_observations: super::AsyncProcessObservations::None,
            removals: Vec::new(),
            retired,
            retired_effect: None,
            retired_generation: None,
            dependency: Some(dependency),
            template_source_changed: template_source_changed.0 || template_source_changed.1,
        };
        let after = authority.wake_projection_without_effect();
        let committed = super::finish_apply_between(
            authority,
            before,
            after,
            compute_slot_released,
            false,
            retirement,
        );
        let committed = match effect_wake {
            Some(effect_wake) => committed.with_effect_wake(effect_wake),
            None => committed,
        };
        Ok(committed.with_resource_health(resource_health))
    }

    fn rollback_failure(
        staged_effect: Option<StagedEffect>,
        error: ConcurrentRetainedIngressError,
    ) -> ConcurrentOwnerRemovalFailure {
        match staged_effect {
            Some(staged_effect) => match staged_effect.rollback_with_wake() {
                Ok(effect_wake) => ConcurrentOwnerRemovalFailure {
                    error,
                    effect_wake: Some(effect_wake),
                },
                Err(_) => ConcurrentOwnerRemovalFailure {
                    error: ConcurrentRetainedIngressError::Fault(AuthorityFault::EffectProjection),
                    effect_wake: None,
                },
            },
            None => ConcurrentOwnerRemovalFailure {
                error,
                effect_wake: None,
            },
        }
    }
}

impl PreparedSharedPeerRevocation<'_> {
    #[cfg(test)]
    pub(in crate::authority) fn peer_fence_stage_id_for_foundation(&self) -> u64 {
        self.core
            .control
            .stage_id()
            .expect("a prepared peer revocation owns its staged fence")
    }

    pub(in crate::authority) fn apply(
        self,
    ) -> Result<CommittedRetainedAdmissionBatch, ConcurrentPeerRevocationFailure> {
        super::apply_seal::commit_shared_peer_revocation(self)
    }

    pub(super) fn apply_with(
        self,
        token: &ApplyToken,
    ) -> Result<CommittedRetainedAdmissionBatch, ConcurrentPeerRevocationFailure> {
        let Self { core, consumed } = self;
        let committed = core.apply_with(token)?;
        Ok(CommittedRetainedAdmissionBatch::Applied {
            retirement: committed,
            consumed,
        })
    }
}

impl PreparedSharedOwnerRemoval<'_, StagedPeerIngressFence<'_>> {
    pub(in crate::authority) fn apply_compute(
        self,
        recovery: ComputeSettlement,
    ) -> super::SharedComputeSettlementOutcome {
        let authority = self.authority;
        match super::apply_seal::commit_shared_owner_removal(self) {
            Ok(committed) => super::SharedComputeSettlementOutcome::Committed(committed),
            Err(failure) => {
                let (error, effect_wake) = failure.into_parts();
                let failure = match error {
                    ConcurrentRetainedIngressError::Stale => authority
                        .compute_settlement_changed_cut_failure(
                            super::SettlementChangedCut::owner_or_projection(),
                            recovery,
                        ),
                    ConcurrentRetainedIngressError::Fault(fault) => {
                        authority.compute_settlement_failure(PlanError::Fault(fault), recovery)
                    }
                };
                super::SharedComputeSettlementOutcome::Failed {
                    failure,
                    effect_wake,
                }
            }
        }
    }
}

impl PreparedSharedRetainedEffectPrefix<'_> {
    pub(in crate::authority) fn apply(self) -> CommittedRetainedAdmissionBatch {
        let Self {
            authority: _authority,
            plan,
            consumed,
            read_cut: _read_cut,
        } = self;
        match plan {
            SharedRetainedEffectPlan::Unchanged => {
                CommittedRetainedAdmissionBatch::Unchanged { consumed }
            }
            SharedRetainedEffectPlan::Publication { staged } => {
                #[cfg(test)]
                _authority.entries.enter_shared_ingress_probe(
                    crate::authority::shard::SharedIngressProbePhase::EffectReadCutBeforeActivation,
                );
                let effect_wake = staged.activate_with_wake();
                let retirement = super::ApplyRetirement {
                    async_process_observations: super::AsyncProcessObservations::None,
                    removals: Vec::new(),
                    retired: super::RetiredOwners::default(),
                    retired_effect: None,
                    retired_generation: None,
                    dependency: None,
                    template_source_changed: false,
                };
                CommittedRetainedAdmissionBatch::Applied {
                    retirement: super::finish_effect_only_apply(effect_wake, retirement),
                    consumed,
                }
            }
        }
    }
}

struct OwnerChange {
    key: RawTxHash,
    before: Option<OwnedTx>,
    vacancy_revision: Option<OwnerShardRemovalRevision>,
    after: OwnedTx,
}

struct OwnerObservation {
    before: Option<OwnedTx>,
    vacancy_revision: Option<OwnerShardRemovalRevision>,
}

struct OwnerOverlay {
    positions: HashMap<RawTxHash, usize>,
    observations: HashMap<RawTxHash, OwnerObservation>,
    changes: Vec<OwnerChange>,
    proposals: HashMap<ProposalId, RawTxHash>,
}

impl OwnerOverlay {
    fn new(maximum_items: usize) -> Self {
        Self {
            positions: HashMap::with_capacity(maximum_items),
            observations: HashMap::with_capacity(maximum_items),
            changes: Vec::with_capacity(maximum_items),
            proposals: HashMap::with_capacity(maximum_items),
        }
    }

    fn observe<'authority>(
        &'authority mut self,
        authority: &TxPoolAuthority,
        read_cut: Option<&ShardedOwnerWriteCut<'_>>,
        key: &RawTxHash,
    ) -> &'authority OwnerObservation {
        self.observations.entry(key.clone()).or_insert_with(|| {
            let (before, vacancy_revision) = read_cut.map_or_else(
                || authority.entries.owner_and_vacancy_revision(key),
                |cut| cut.owner_and_vacancy_revision(&authority.entries, key),
            );
            OwnerObservation {
                before,
                vacancy_revision,
            }
        })
    }

    fn current(
        &mut self,
        authority: &TxPoolAuthority,
        read_cut: Option<&ShardedOwnerWriteCut<'_>>,
        key: &RawTxHash,
    ) -> Result<Option<OwnedTx>, PlanError> {
        match self.positions.get(key).copied() {
            Some(position) => self
                .changes
                .get(position)
                .map(|change| Some(change.after.clone()))
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection)),
            None => Ok(self.observe(authority, read_cut, key).before.clone()),
        }
    }

    fn proposal_owner(
        &self,
        authority: &TxPoolAuthority,
        read_cut: Option<&ShardedOwnerWriteCut<'_>>,
        proposal: &ProposalId,
    ) -> Option<RawTxHash> {
        self.proposals.get(proposal).cloned().or_else(|| {
            read_cut.map_or_else(
                || authority.indexes.proposal_owner(proposal),
                |cut| cut.proposal_owner(&authority.entries, proposal).cloned(),
            )
        })
    }

    fn replace(
        &mut self,
        authority: &TxPoolAuthority,
        read_cut: Option<&ShardedOwnerWriteCut<'_>>,
        key: RawTxHash,
        after: OwnedTx,
    ) -> Result<(), PlanError> {
        if let Some(position) = self.positions.get(&key).copied() {
            let Some(change) = self.changes.get_mut(position) else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            change.after = after;
            return Ok(());
        }
        let position = self.changes.len();
        let OwnerObservation {
            before,
            vacancy_revision,
        } = self.observe(authority, read_cut, &key);
        let before = before.clone();
        let vacancy_revision = *vacancy_revision;
        self.positions.insert(key.clone(), position);
        self.proposals
            .insert(after.record().identity.proposal.clone(), key.clone());
        self.changes.push(OwnerChange {
            key,
            before,
            vacancy_revision,
            after,
        });
        Ok(())
    }
}

struct BatchScratch<'cut> {
    owners: OwnerOverlay,
    resources: OrderedResourceProjection,
    clocks: ClockPlanReservation,
    read_cut: Option<ShardedOwnerWriteCut<'cut>>,
}

fn replace_scratch_resources(
    resources: &mut OrderedResourceProjection,
    read_cut: Option<&ShardedOwnerWriteCut<'_>>,
    authority: &TxPoolAuthority,
    expected: Option<ChargeRecord>,
    after: Option<ChargeRecord>,
) -> Result<(), ResourceError> {
    resources.replace_with_peer(expected, after, |peer| {
        read_cut.map_or_else(
            || authority.entries.peer_resource(peer),
            |cut| cut.peer_resource(&authority.entries, peer),
        )
    })
}

enum ItemDecision {
    Owner,
    NoOwner(Option<CommittedEffect>),
}

impl ItemDecision {
    fn no_owner(effect: Option<CommittedEffect>) -> Self {
        Self::NoOwner(effect)
    }

    fn owner() -> Self {
        Self::Owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum SharedRetainedIngressHead {
    Owner,
    EffectOrNoop,
}

fn shared_resource_race(error: &ResourceError) -> bool {
    matches!(
        error,
        ResourceError::PreAcceptedLimit
            | ResourceError::RemoteLimit
            | ResourceError::PeerLimit(_)
            | ResourceError::ReplacementHistoryLimit
            | ResourceError::AcceptedLimit
            | ResourceError::ExistingChargeMismatch
    )
}

impl TxPoolAuthority {
    fn retained_ingress_read_cut(
        &self,
        batch: &RetainedAdmissionBatch,
        maximum_items: usize,
    ) -> Result<ShardedOwnerWriteCut<'_>, PlanError> {
        let peer = match batch.kind() {
            RetainedIngressKind::Remote(peer)
                if batch
                    .attempts()
                    .take(maximum_items)
                    .any(|attempt| matches!(attempt, RetainedIngressAttempt::Validated(_))) =>
            {
                Some(peer)
            }
            RetainedIngressKind::Remote(_) => None,
            RetainedIngressKind::Proposal => None,
        };
        let mut owners = Vec::new();
        owners.reserve_exact(maximum_items);
        let mut proposals = Vec::new();
        proposals.reserve_exact(maximum_items);
        for attempt in batch.attempts().take(maximum_items) {
            let RetainedIngressAttempt::Validated(ingress) = attempt else {
                continue;
            };
            owners.push(ingress.admission().identity.raw.clone());
            proposals.push(ingress.admission().identity.proposal.clone());
        }
        Ok(self
            .entries
            .retained_ingress_read_cut(&owners, &proposals, peer))
    }

    fn compile_retained_independent_delta(
        &self,
        changes: Vec<OwnerChange>,
        sequence: ApplySequence,
        index_premise: RetainedIndexPremise,
        resource_envelope: OrderedResourceEnvelope,
    ) -> Result<IndependentDelta, PlanError> {
        self.reserve_primary_owner_insertions(
            changes
                .iter()
                .filter(|change| change.before.is_none())
                .map(|change| &change.key),
        );
        let mut resource_changes = Vec::new();
        resource_changes.reserve_exact(changes.len());
        resource_changes.extend(changes.iter().map(|change| {
            (
                change.key.clone(),
                change.before.as_ref().map(OwnedTx::charge_record),
                Some(change.after.charge_record()),
            )
        }));
        let resources = match self
            .resources_for_plan()
            .plan_ordered_batch(resource_changes, resource_envelope)
        {
            Ok(resources) => resources,
            Err(error) if shared_resource_race(&error) => {
                return Err(PlanError::Stale(super::StalePlan::Version));
            }
            Err(error) => return Err(error.into()),
        };
        let scheduler = self.scheduler.lock().compile_batch(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), Some(&change.after))),
        )?;
        let dependency = self.dependencies.compile_primary_replacements(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), Some(&change.after))),
        )?;
        let sources = super::AuthoritySourceVersions::plan_template_selection_replacements(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), Some(&change.after))),
            sequence,
        );
        let template_sources = self.plan_owner_sources(
            changes
                .iter()
                .map(|change| (&change.key, change.before.as_ref(), Some(&change.after))),
        )?;
        let indexes = self.indexes_for_plan().plan_retained_replacements(
            changes
                .iter()
                .map(|change| (&change.key, change.before.as_ref(), Some(&change.after))),
            index_premise,
        )?;
        let retired = super::retired_buffer(changes.len());
        let mut owner_cuts = Vec::new();
        owner_cuts.reserve_exact(changes.len());
        owner_cuts.extend(changes.into_iter().map(|change| {
            IndependentOwnerCut {
                key: change.key,
                expected: change
                    .before
                    .as_ref()
                    .map_or(OwnerPrestate::Vacant, OwnerPrestate::from_owner),
                removal_revision: change.vacancy_revision,
                action: IndependentOwnerAction::Replace(Some(change.after)),
            }
        }));
        Ok(IndependentDelta {
            owner_cuts,
            owners: DerivedOwnerDelta {
                indexes,
                sources,
                template_sources,
            },
            resource: Some(resources),
            projection: super::ProjectionDelta::empty(),
            scheduler,
            dependency,
            effect: EffectDelta::default(),
            async_process_starts: Vec::new(),
            removals: Vec::new(),
            retired,
        })
    }

    fn plan_retained_batch_prefix(
        &self,
        kind: RetainedIngressKind,
        batch: &RetainedAdmissionBatch,
        planned_at: Instant,
        scratch: &mut BatchScratch<'_>,
        effects: &mut OrderedEffectPublication,
        maximum_items: usize,
    ) -> Result<usize, PlanError> {
        let mut consumed = 0usize;
        for attempt in batch.attempts().take(maximum_items) {
            let decision =
                self.plan_retained_batch_item(kind, attempt, planned_at, scratch, false)?;
            let effect = match decision {
                ItemDecision::Owner => break,
                ItemDecision::NoOwner(effect) => effect,
            };
            if let Some(effect) = effect {
                match effects.push(effect) {
                    Ok(()) => {}
                    Err(OrderedEffectAppendError::Full) if consumed == 0 => {
                        return Err(PlanError::Backpressure(super::Backpressure::EffectCapacity));
                    }
                    Err(OrderedEffectAppendError::Full) => break,
                    Err(OrderedEffectAppendError::Projection) => {
                        return Err(PlanError::Fault(AuthorityFault::EffectProjection));
                    }
                }
            }
            consumed = consumed
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        }
        Ok(consumed)
    }

    /// Classify the first canonical retained-ingress outcome under one
    /// monotone routed read-support closure. This is the exhaustive routing
    /// authority for every non-malformed batch; a later state change is an OCC
    /// contention outcome, not a reason to probe a second semantic route.
    pub(in crate::authority) fn classify_shared_retained_ingress_head(
        &self,
        batch: &RetainedAdmissionBatch,
    ) -> Result<SharedRetainedIngressHead, PlanError> {
        if batch
            .attempts()
            .any(RetainedIngressAttempt::is_malformed_remote)
        {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        self.effects.lock().ensure_open()?;
        let kind = batch.kind();
        let mut scratch = BatchScratch {
            owners: OwnerOverlay::new(1),
            // One classified item can release at most one existing Remote
            // attribution. Reserve that map slot before the routed read cut.
            resources: self.resources.ordered_committed_projection(1)?,
            clocks: ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks)),
            read_cut: None,
        };
        scratch.read_cut = Some(self.retained_ingress_read_cut(batch, 1)?);
        let attempt = batch
            .attempts()
            .next()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let decision =
            self.plan_retained_batch_item(kind, attempt, Instant::now(), &mut scratch, false)?;
        Ok(match decision {
            ItemDecision::Owner => SharedRetainedIngressHead::Owner,
            ItemDecision::NoOwner(_) => SharedRetainedIngressHead::EffectOrNoop,
        })
    }

    /// Compile only the closed retained-ingress owner shape: all-new Remote or
    /// Proposal insertion, or an existing Remote/history owner promoted to
    /// Proposal, with no effect. The canonical item planner remains the sole
    /// semantic compiler. Exact old/new scheduler visibility, dependency
    /// rows, owner-removal ABA evidence and the final owner/resource/index cut
    /// make unrelated shared Apply independent without an outer write branch.
    pub(in crate::authority) fn compile_shared_retained_ingress_batch(
        &self,
        batch: &RetainedAdmissionBatch,
    ) -> Result<Option<CompiledSharedRetainedAdmissionBatch>, PlanError> {
        if batch
            .attempts()
            .any(RetainedIngressAttempt::is_malformed_remote)
        {
            return Ok(None);
        }

        self.effects.lock().ensure_open()?;
        let item_count = batch.len();
        let kind = batch.kind();
        let planned_at = Instant::now();
        let maximum_peers = match kind {
            RetainedIngressKind::Remote(_) => 1,
            RetainedIngressKind::Proposal => item_count,
        };
        let mut scratch = BatchScratch {
            owners: OwnerOverlay::new(item_count),
            resources: self.resources.ordered_committed_projection(maximum_peers)?,
            clocks: ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks)),
            read_cut: None,
        };
        scratch.read_cut = Some(self.retained_ingress_read_cut(batch, item_count)?);
        let mut consumed = 0usize;
        for attempt in batch.attempts() {
            let decision =
                self.plan_retained_batch_item(kind, attempt, planned_at, &mut scratch, true)?;
            match decision {
                ItemDecision::Owner => {}
                ItemDecision::NoOwner(_) => break,
            }
            consumed = consumed
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        }
        if consumed == 0 || scratch.owners.changes.is_empty() {
            return Ok(None);
        }

        let index_premise = self.indexes_for_plan().capture_retained_premise(
            scratch
                .owners
                .changes
                .iter()
                .map(|change| (&change.key, change.before.as_ref(), Some(&change.after))),
            scratch
                .read_cut
                .as_ref()
                .ok_or(PlanError::Fault(AuthorityFault::IndexProjection))?,
        )?;

        let BatchScratch {
            owners,
            resources,
            clocks,
            read_cut,
        } = scratch;
        drop(read_cut);
        #[cfg(test)]
        self.entries.enter_shared_ingress_probe(
            crate::authority::shard::SharedIngressProbePhase::AfterRetainedIngressSemanticCut,
        );
        let resource_envelope = resources.into_envelope();
        let clocks = clocks.commit()?;
        let sequence = clocks.sequence();
        let changes = owners.changes;
        let delta = self.compile_retained_independent_delta(
            changes,
            sequence,
            index_premise,
            resource_envelope,
        )?;
        if !delta.is_shared_retained_owner_only_shape(consumed) {
            return Ok(None);
        }
        Ok(Some(CompiledSharedRetainedAdmissionBatch {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            delta,
            consumed,
        }))
    }

    /// Plan the longest canonical retained-ingress prefix which owns no
    /// transaction row. No-op items commit unchanged; rejection, duplicate,
    /// release and pressure effects reserve one bounded hidden record in the
    /// sole EffectLog and activate it without the outer write guard. The first
    /// owner-producing item and every later item remain in the exact move-only
    /// suffix for the next canonical round.
    pub(in crate::authority) fn plan_shared_retained_effect_prefix(
        &self,
        batch: &RetainedAdmissionBatch,
    ) -> Result<Option<PreparedSharedRetainedEffectPrefix<'_>>, PlanError> {
        if batch
            .attempts()
            .any(RetainedIngressAttempt::is_malformed_remote)
        {
            return Ok(None);
        }

        self.effects.lock().ensure_open()?;
        let kind = batch.kind();
        let item_count = batch.len();
        let policy = match kind {
            RetainedIngressKind::Remote(_) => EffectPolicy::Remote,
            RetainedIngressKind::Proposal => EffectPolicy::Trusted,
        };
        let preview_maximum_peers = match kind {
            RetainedIngressKind::Remote(_) => 1,
            RetainedIngressKind::Proposal => item_count,
        };
        let mut preview_effects = self
            .effects
            .lock()
            .ordered_publication(policy, item_count)?;
        let mut preview = BatchScratch {
            owners: OwnerOverlay::new(item_count),
            resources: self
                .resources
                .ordered_committed_projection(preview_maximum_peers)?,
            clocks: ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks)),
            read_cut: None,
        };
        let tentative = self.plan_retained_batch_prefix(
            kind,
            batch,
            Instant::now(),
            &mut preview,
            &mut preview_effects,
            item_count,
        )?;
        drop(preview_effects);
        drop(preview);
        if tentative == 0 {
            return Ok(None);
        }

        let maximum_peers = match kind {
            RetainedIngressKind::Remote(_) => 1,
            RetainedIngressKind::Proposal => tentative,
        };
        let mut effects = self.effects.lock().ordered_publication(policy, tentative)?;
        let mut scratch = BatchScratch {
            owners: OwnerOverlay::new(tentative),
            resources: self.resources.ordered_committed_projection(maximum_peers)?,
            clocks: ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks)),
            read_cut: None,
        };
        scratch.read_cut = Some(self.retained_ingress_read_cut(batch, tentative)?);
        let consumed = self.plan_retained_batch_prefix(
            kind,
            batch,
            Instant::now(),
            &mut scratch,
            &mut effects,
            tentative,
        )?;
        if consumed == 0 {
            return Err(PlanError::Stale(super::StalePlan::Version));
        }
        if consumed != tentative {
            // The preview cut may race EffectLog capacity or owner
            // classification. Keeping its larger support would block a suffix
            // which this publication cannot consume. Drop every provisional
            // capability and let the bounded runtime reclassification acquire
            // support for the new exact prefix.
            return Err(PlanError::Stale(super::StalePlan::Version));
        }

        let publication = effects.finish()?;
        let read_cut = scratch
            .read_cut
            .take()
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        drop(scratch);
        let Some(publication) = publication else {
            return Ok(Some(PreparedSharedRetainedEffectPrefix {
                authority: self,
                plan: SharedRetainedEffectPlan::Unchanged,
                consumed,
                read_cut,
            }));
        };
        let clocks = ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks)).commit()?;
        let sequence = clocks.sequence();
        let effect = self
            .effects_for_plan()
            .plan_publication(&publication, sequence)?;
        let staged = super::super::effect::EffectLog::stage_publication(&self.effects, effect)?;
        Ok(Some(PreparedSharedRetainedEffectPrefix {
            authority: self,
            plan: SharedRetainedEffectPlan::Publication { staged },
            consumed,
            read_cut,
        }))
    }

    /// Stage the sole malformed-Remote policy without the outer authority
    /// write guard. The peer-row Hidden fence is installed before the cohort
    /// snapshot, so same-peer ingress can no longer grow the removal set; all
    /// allocations and fallible journal/scheduler/dependency work still occur
    /// before any owner mutation.
    pub(in crate::authority) fn plan_shared_peer_revocation(
        &self,
        batch: &RetainedAdmissionBatch,
    ) -> Result<Option<PreparedSharedPeerRevocation<'_>>, PlanError> {
        let Some(position) = batch
            .attempts()
            .position(RetainedIngressAttempt::is_malformed_remote)
        else {
            return Ok(None);
        };
        let RetainedIngressKind::Remote(peer) = batch.kind() else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        let culprit = batch
            .attempts()
            .nth(position)
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
        let RetainedIngressAttempt::Rejected(culprit) = culprit else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        if !culprit.reason().is_malformed() {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        let core = self.compile_shared_peer_revocation_core(
            peer,
            RawTxHash(culprit.transaction().hash()),
            culprit.reason().clone(),
        )?;
        Ok(Some(PreparedSharedPeerRevocation {
            core,
            consumed: batch.len(),
        }))
    }

    pub(in crate::authority) fn compile_shared_peer_revocation_core(
        &self,
        peer: PeerIndex,
        culprit_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Result<PreparedSharedPeerRevocationCore<'_>, PlanError> {
        if !reason.is_malformed() {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        }
        self.effects.lock().ensure_open()?;
        let slot = self.peer_bans.plan_record(peer, Instant::now())?;
        let revocation =
            CommittedPeerCohortRevocation::malformed(slot.lease(), culprit_hash, reason)
                .ok_or(PlanError::Fault(AuthorityFault::EffectProjection))?;
        let mut effects = Vec::new();
        effects.reserve_exact(1);
        effects.push(CommittedEffect::PeerCohortRevoked(revocation));
        let publication = self
            .effects
            .lock()
            .build_publication(EffectPolicy::CriticalDetail, effects)
            .map_err(|error| match error {
                EffectBuildError::Empty
                | EffectBuildError::TooMany
                | EffectBuildError::TooLarge
                | EffectBuildError::Arithmetic
                | EffectBuildError::ReservedReset => {
                    PlanError::Fault(AuthorityFault::EffectProjection)
                }
            })?;
        let clocks = ApplyClockReservation::begin(std::sync::Arc::clone(&self.clocks))?;
        let sequence = clocks.sequence();
        let peer_fence =
            self.entries
                .stage_peer_ingress_fence(slot)
                .map_err(|error| match error {
                    crate::authority::shard::PeerFenceStageError::Stale => {
                        PlanError::Stale(super::StalePlan::Version)
                    }
                })?;
        let indexed = self.indexes.preaccepted_for_peer(peer).unwrap_or_default();
        let mut hashes = Vec::new();
        hashes.reserve_exact(indexed.len());
        hashes.extend(indexed);
        hashes.sort_unstable();
        let removal = self.plan_preaccepted_peer_cohort_removal_batch(
            OwnerRemovalKeys::new(hashes)?,
            peer,
            sequence,
        )?;
        // Bind stages scheduler/effect rows and prepares dependency gates only
        // after every bounded cohort/delta allocation has succeeded.
        let removal = CompiledSharedOwnerRemoval {
            generation: self.generation,
            chain_view: self.chain_view.clone(),
            removal,
            publication: Some(publication),
            sequence,
            control: peer_fence,
        }
        .bind(self)?;
        Ok(removal)
    }

    fn plan_retained_batch_item(
        &self,
        kind: RetainedIngressKind,
        attempt: &RetainedIngressAttempt,
        planned_at: Instant,
        scratch: &mut BatchScratch<'_>,
        materialize_owner: bool,
    ) -> Result<ItemDecision, PlanError> {
        match attempt {
            RetainedIngressAttempt::Rejected(rejection) => {
                let audience = match rejection.kind() {
                    RetainedIngressKind::Remote(peer) => Some(peer),
                    RetainedIngressKind::Proposal => None,
                };
                Ok(ItemDecision::no_owner(Some(CommittedEffect::Rejected(
                    CommittedRejection::Validation {
                        tx: std::sync::Arc::clone(rejection.transaction()),
                        audience: RejectionAudience::from_ingress(audience),
                        reason: rejection.reason().clone(),
                    },
                ))))
            }
            RetainedIngressAttempt::ProposalUnavailable => Ok(ItemDecision::no_owner(None)),
            RetainedIngressAttempt::Validated(ingress) => {
                if ingress.kind() != kind {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                self.plan_validated_retained_batch_item(
                    kind,
                    ingress.admission(),
                    planned_at,
                    scratch,
                    materialize_owner,
                )
            }
        }
    }

    fn plan_validated_retained_batch_item(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        planned_at: Instant,
        scratch: &mut BatchScratch<'_>,
        materialize_owner: bool,
    ) -> Result<ItemDecision, PlanError> {
        let key = admission.identity.raw.clone();
        if let RetainedIngressKind::Remote(peer) = kind {
            let banned = match scratch.read_cut.as_ref() {
                Some(cut) => cut.peer_is_banned_at(&self.entries, peer, planned_at),
                None => self.entries.peer_is_banned_at(peer, planned_at),
            }
            .map_err(|error| match error {
                crate::authority::shard::PeerFenceStageError::Stale => {
                    PlanError::Stale(super::StalePlan::Version)
                }
            })?;
            if banned {
                return Ok(ItemDecision::no_owner(Some(
                    CommittedEffect::RemoteIngressReleased(
                        CommittedRemoteIngressRelease::unretained_remote_submission(key, peer),
                    ),
                )));
            }
            return match scratch
                .owners
                .current(self, scratch.read_cut.as_ref(), &key)?
            {
                Some(OwnedTx::Accepted(_)) => Ok(ItemDecision::no_owner(Some(
                    CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                        tx_hash: key,
                        requesting_peer: Some(peer),
                    }),
                ))),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) => Ok(
                    ItemDecision::no_owner(Some(CommittedEffect::RemoteIngressReleased(
                        CommittedRemoteIngressRelease::unretained_remote_submission(key, peer),
                    ))),
                ),
                None => self.plan_new_retained_owner(kind, admission, scratch, materialize_owner),
            };
        }

        let current = scratch
            .owners
            .current(self, scratch.read_cut.as_ref(), &key)?;
        match &current {
            Some(OwnedTx::Accepted(_)) => {
                return Ok(ItemDecision::no_owner(None));
            }
            Some(OwnedTx::PreAccepted(entry))
                if entry.record.identity.witness == admission.identity.witness
                    && !matches!(entry.source, PreAcceptedSource::Remote(_)) =>
            {
                return Ok(ItemDecision::no_owner(None));
            }
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.source, PreAcceptedSource::Recovery(_)) =>
            {
                return Ok(ItemDecision::no_owner(None));
            }
            Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {}
        }
        self.plan_proposal_owner(admission, current, scratch, materialize_owner)
    }

    fn plan_new_retained_owner(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        scratch: &mut BatchScratch<'_>,
        materialize_owner: bool,
    ) -> Result<ItemDecision, PlanError> {
        if let Some(owner) = scratch.owners.proposal_owner(
            self,
            scratch.read_cut.as_ref(),
            &admission.identity.proposal,
        ) && owner != admission.identity.raw
        {
            return self.retained_pressure(kind, admission, super::Backpressure::ProposalCollision);
        }
        let charge = self
            .resources
            .admission_charge(admission.payload_bytes, admission.encoded_edges)?;
        if let Err(error) = self.resources.validate_admission(charge) {
            return self.retained_resource_pressure(kind, admission, error);
        }
        let charge_record = ChargeRecord::PreAccepted {
            resources: charge,
            residency_peer: admission.source.ingress_peer(),
            compute_peer: None,
        };
        if let Err(error) = replace_scratch_resources(
            &mut scratch.resources,
            scratch.read_cut.as_ref(),
            self,
            None,
            Some(charge_record),
        ) {
            return self.retained_resource_pressure(kind, admission, error);
        }
        if !materialize_owner {
            return Ok(ItemDecision::owner());
        }

        // Identity allocation follows every fallible resource decision which
        // does not need that identity. A pressure-excluded item therefore
        // consumes neither a version nor an arrival, while a subsequently
        // dropped nonempty Plan still leaves its already-issued identities as
        // non-reusable gaps.
        let (version, arrival) = scratch.clocks.insertion()?;
        let after = OwnedTx::PreAccepted(PreAcceptedEntry {
            record: TxRecord {
                tx: std::sync::Arc::clone(&admission.tx),
                identity: admission.identity.clone(),
                version,
                arrival,
            },
            source: admission.source,
            basis: AdmissionBasis::new(
                admission.dependencies.clone(),
                admission.payload_bytes,
                admission.encoded_edges,
                charge,
            ),
            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
            charge,
        });
        scratch.owners.replace(
            self,
            scratch.read_cut.as_ref(),
            admission.identity.raw.clone(),
            after,
        )?;
        Ok(ItemDecision::owner())
    }

    fn plan_proposal_owner(
        &self,
        admission: &ValidatedAdmission,
        current: Option<OwnedTx>,
        scratch: &mut BatchScratch<'_>,
        materialize_owner: bool,
    ) -> Result<ItemDecision, PlanError> {
        let Some(current) = current else {
            return self.plan_new_retained_owner(
                RetainedIngressKind::Proposal,
                admission,
                scratch,
                materialize_owner,
            );
        };
        let charge = self
            .resources
            .admission_charge(admission.payload_bytes, admission.encoded_edges)?;
        if let Err(error) = self.resources.validate_admission(charge) {
            return self.retained_resource_pressure(
                RetainedIngressKind::Proposal,
                admission,
                error,
            );
        }

        let placeholder_version = current.record().version;
        let mut after = match &current {
            OwnedTx::PreAccepted(entry) => {
                let same_witness = entry.record.identity.witness == admission.identity.witness;
                let proposal_base = match entry.source {
                    PreAcceptedSource::Remote(remote) => ProposalBase::Remote(remote.residency),
                    PreAcceptedSource::Proposal { base } => base,
                    PreAcceptedSource::Recovery(_) => {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                };
                if same_witness {
                    let mut promoted = entry.clone();
                    promoted.source = PreAcceptedSource::Proposal {
                        base: proposal_base,
                    };
                    if matches!(promoted.phase, PreAcceptedPhase::Waiting(_)) {
                        promoted.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
                        promoted.charge = promoted.original_charge();
                    }
                    if promoted.source == entry.source && promoted.phase == entry.phase {
                        return Ok(ItemDecision::no_owner(None));
                    }
                    OwnedTx::PreAccepted(promoted)
                } else {
                    OwnedTx::PreAccepted(PreAcceptedEntry {
                        record: TxRecord {
                            tx: std::sync::Arc::clone(&admission.tx),
                            identity: admission.identity.clone(),
                            version: placeholder_version,
                            arrival: entry.record.arrival,
                        },
                        source: PreAcceptedSource::Proposal {
                            base: proposal_base,
                        },
                        basis: AdmissionBasis::new(
                            admission.dependencies.clone(),
                            admission.payload_bytes,
                            admission.encoded_edges,
                            charge,
                        ),
                        phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        charge,
                    })
                }
            }
            OwnedTx::ReplacementHistory(history) => {
                let same_witness = history.record().identity.witness == admission.identity.witness;
                let promoted = if same_witness {
                    let mut promoted = history
                        .clone()
                        .into_recovery(self.generation, placeholder_version);
                    promoted.source = PreAcceptedSource::Proposal {
                        base: ProposalBase::Trusted,
                    };
                    promoted
                } else {
                    PreAcceptedEntry {
                        record: TxRecord {
                            tx: std::sync::Arc::clone(&admission.tx),
                            identity: admission.identity.clone(),
                            version: placeholder_version,
                            arrival: history.record().arrival,
                        },
                        source: PreAcceptedSource::Proposal {
                            base: ProposalBase::Trusted,
                        },
                        basis: AdmissionBasis::new(
                            admission.dependencies.clone(),
                            admission.payload_bytes,
                            admission.encoded_edges,
                            charge,
                        ),
                        phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        charge,
                    }
                };
                OwnedTx::PreAccepted(promoted)
            }
            OwnedTx::Accepted(_) => {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        };
        if let Err(error) = replace_scratch_resources(
            &mut scratch.resources,
            scratch.read_cut.as_ref(),
            self,
            Some(current.charge_record()),
            Some(after.charge_record()),
        ) {
            return self.retained_resource_pressure(
                RetainedIngressKind::Proposal,
                admission,
                error,
            );
        }
        if !materialize_owner {
            return Ok(ItemDecision::owner());
        }
        let version = scratch.clocks.replacement()?;
        let OwnedTx::PreAccepted(entry) = &mut after else {
            return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
        };
        entry.record.version = version;
        scratch.owners.replace(
            self,
            scratch.read_cut.as_ref(),
            admission.identity.raw.clone(),
            after,
        )?;
        Ok(ItemDecision::owner())
    }

    fn retained_resource_pressure(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        error: ResourceError,
    ) -> Result<ItemDecision, PlanError> {
        let pressure = match error {
            ResourceError::PreAcceptedLimit => super::Backpressure::TotalResources,
            ResourceError::RemoteLimit => super::Backpressure::RemoteResources,
            ResourceError::PeerLimit(_) => super::Backpressure::PeerResources,
            ResourceError::ComputeEnvelope => super::Backpressure::ComputeResources,
            ResourceError::ReplacementHistoryLimit
            | ResourceError::AcceptedLimit
            | ResourceError::Arithmetic
            | ResourceError::ExistingChargeMismatch
            | ResourceError::DuplicateChange
            | ResourceError::AttributionMismatch
            | ResourceError::CapacityBankFault => return Err(error.into()),
        };
        self.retained_pressure(kind, admission, pressure)
    }

    fn retained_pressure(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        pressure: super::Backpressure,
    ) -> Result<ItemDecision, PlanError> {
        let RetainedIngressKind::Remote(peer) = kind else {
            return match pressure {
                super::Backpressure::TotalResources
                | super::Backpressure::RemoteResources
                | super::Backpressure::PeerResources
                | super::Backpressure::ComputeResources
                | super::Backpressure::ProposalCollision => Ok(ItemDecision::no_owner(None)),
                super::Backpressure::AcceptedResources
                | super::Backpressure::GenerationReplacement
                | super::Backpressure::EffectCapacity => {
                    Err(PlanError::Fault(AuthorityFault::ResourceProjection))
                }
            };
        };
        let pressure = match pressure {
            super::Backpressure::TotalResources => RemoteIngressPressure::TotalResources,
            super::Backpressure::RemoteResources => RemoteIngressPressure::RemoteResources,
            super::Backpressure::PeerResources => RemoteIngressPressure::PeerResources,
            super::Backpressure::ComputeResources => RemoteIngressPressure::ComputeResources,
            super::Backpressure::ProposalCollision => RemoteIngressPressure::ProposalCollision,
            super::Backpressure::AcceptedResources
            | super::Backpressure::GenerationReplacement
            | super::Backpressure::EffectCapacity => {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
        };
        let reason = crate::authority::rejection::CommittedPublicReject::new(
            crate::error::Reject::Full(pressure.reason().to_owned()),
        );
        Ok(ItemDecision::no_owner(Some(CommittedEffect::Rejected(
            CommittedRejection::Validation {
                tx: std::sync::Arc::clone(&admission.tx),
                audience: RejectionAudience::from_ingress(Some(peer)),
                reason,
            },
        ))))
    }
}
