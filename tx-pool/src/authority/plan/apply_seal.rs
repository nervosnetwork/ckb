use super::super::dependency::DependencyApplyOutcome;
use super::super::resources::{
    OrderedResourceEnvelope, ResourceCapacityBeginError, ResourceCapacityCommit,
    ResourceCapacityWaitIdentity, ResourceCommitHealth, ResourceTotals,
};
use super::super::shard::{
    AuthorityShardRouter, DependencyGateCut, DependencyGateSupport, ShardOwnerSourceAdvance,
    ShardOwnerSourceCounts, ShardReadSupport, ShardResourcePlan, ShardWriteSupport,
    ShardedDependencyRelationWriteCut, ShardedOwnerMap, ShardedOwnerWriteCut,
};
use super::*;
use ckb_util::parking_lot::{Mutex, MutexGuard};
use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;

/// The authoritative state behind the planning facade.
///
/// Fields are readable throughout the parent planning module through
/// `TxPoolAuthority`'s immutable `Deref`.  Writable access requires the
/// unforgeable `ApplyToken`, so a planner cannot commit a partial transition
/// while it is constructing a delta.
#[derive(Debug)]
pub(in crate::authority) struct AuthorityState {
    pub(super) generation: PoolGeneration,
    pub(super) chain_view: ChainViewId,
    owner_resources: OwnerResourceAuthority,
    pub(super) indexes: AuthorityIndexes,
    pub(super) source_versions: AuthoritySourceVersions,
    pub(super) membership: MembershipProjection,
    pub(super) scheduler: Arc<Mutex<FairFrontier>>,
    pub(super) dependencies: DependencyFrontier,
    pub(super) effects: Arc<Mutex<EffectLog>>,
    pub(super) peer_bans: PeerBanSlotBank,
    pub(super) membership_config: MembershipConfig,
    pub(super) clocks: Arc<AuthorityClockBank>,
}

/// One physical owner for primary transactions and their bounded resource
/// projection.  Parent planners receive only this type's immutable `Deref`;
/// mutation remains private to this sealing module and requires `ApplyToken`.
#[derive(Debug)]
pub(in crate::authority) struct OwnerResourceAuthority {
    pub(super) entries: ShardedOwnerMap,
    pub(super) resources: ResourceLedger,
}

impl Deref for AuthorityState {
    type Target = OwnerResourceAuthority;

    fn deref(&self) -> &Self::Target {
        &self.owner_resources
    }
}

/// Read-mostly planning facade for one authoritative tx-pool generation.
///
/// There is deliberately no `DerefMut`: all committed writes must hold the
/// token created by the real `PreparedApply::apply` boundary.
#[derive(Debug)]
pub(in crate::authority) struct TxPoolAuthority {
    state: AuthorityState,
}

/// Fresh-generation compiler with no conversion back into a live authority.
///
/// It exposes only the ordinary admission compiler and a consuming projection
/// extraction, never an `&mut TxPoolAuthority` that a planner could swap with
/// the live authority.
pub(super) struct ScratchAuthority {
    authority: TxPoolAuthority,
    dependency_maintenance_activated: bool,
}

pub(super) struct ScratchAuthoritySeed {
    chain_view: ChainViewId,
    generation: PoolGeneration,
    clocks: AuthorityClocks,
    router: AuthorityShardRouter,
}

impl ScratchAuthoritySeed {
    pub(super) fn new(
        chain_view: ChainViewId,
        generation: PoolGeneration,
        clocks: AuthorityClocks,
        router: AuthorityShardRouter,
    ) -> Self {
        Self {
            chain_view,
            generation,
            clocks,
            router,
        }
    }
}

impl Deref for TxPoolAuthority {
    type Target = AuthorityState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// Capability whose constructor is private to this sealing module.
pub(in crate::authority) struct ApplyToken(());

/// Cause-specific final-cut evidence around the one shared OwnerRemovalBatch
/// engine. The control may add read/write support and one allocation-free
/// activation, but it cannot alter owner/resource/index/membership semantics.
pub(in crate::authority) trait SharedOwnerRemovalControl {
    type Begun;

    fn extend_final_support(
        &self,
        _entries: &ShardedOwnerMap,
        reads: &mut ShardReadSupport,
        writes: &mut ShardWriteSupport,
    );

    fn index_prestate_is_fresh(
        &self,
        indexes: &IndexDelta,
        _entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool;

    fn prestate_is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool;

    fn begin(
        self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> Result<Self::Begun, super::ingress::ConcurrentRetainedIngressError>;

    fn activate(begun: Self::Begun, cut: &mut ShardedOwnerWriteCut<'_>);
}

impl SharedOwnerRemovalControl for RemoteExpiryWitness {
    type Begun = ();

    fn extend_final_support(
        &self,
        _entries: &ShardedOwnerMap,
        reads: &mut ShardReadSupport,
        _writes: &mut ShardWriteSupport,
    ) {
        self.extend_final_read_support(reads);
    }

    fn index_prestate_is_fresh(
        &self,
        indexes: &IndexDelta,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        indexes.prestate_is_fresh(entries, cut)
    }

    fn prestate_is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        RemoteExpiryWitness::prestate_is_fresh(self, entries, cut)
    }

    fn begin(
        self,
        _cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> Result<Self::Begun, super::ingress::ConcurrentRetainedIngressError> {
        Ok(())
    }

    fn activate(_begun: Self::Begun, _cut: &mut ShardedOwnerWriteCut<'_>) {}
}

impl SharedOwnerRemovalControl for AdministrativeRemovalControl {
    type Begun = ();

    fn extend_final_support(
        &self,
        entries: &ShardedOwnerMap,
        reads: &mut ShardReadSupport,
        _writes: &mut ShardWriteSupport,
    ) {
        for parent in &self.parents {
            reads.insert(entries.owner_shard(&parent.hash));
        }
    }

    fn index_prestate_is_fresh(
        &self,
        indexes: &IndexDelta,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        indexes.prestate_is_fresh(entries, cut)
    }

    fn prestate_is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        self.parents.iter().all(|parent| {
            cut.owner_version_and_accepted(entries.owner_shard(&parent.hash), &parent.hash)
                == Some((parent.version, true))
        })
    }

    fn begin(
        self,
        _cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> Result<Self::Begun, super::ingress::ConcurrentRetainedIngressError> {
        Ok(())
    }

    fn activate(_begun: Self::Begun, _cut: &mut ShardedOwnerWriteCut<'_>) {}
}

impl SharedOwnerRemovalControl for AcceptedExpiryControl {
    type Begun = ();

    fn extend_final_support(
        &self,
        entries: &ShardedOwnerMap,
        reads: &mut ShardReadSupport,
        writes: &mut ShardWriteSupport,
    ) {
        self.administrative
            .extend_final_support(entries, reads, writes);
    }

    fn index_prestate_is_fresh(
        &self,
        indexes: &IndexDelta,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.administrative
            .index_prestate_is_fresh(indexes, entries, cut)
            && self.head.prestate_is_fresh(entries, cut)
    }

    fn prestate_is_fresh(&self, entries: &ShardedOwnerMap, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        self.administrative.prestate_is_fresh(entries, cut)
    }

    fn begin(
        self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> Result<Self::Begun, super::ingress::ConcurrentRetainedIngressError> {
        self.administrative.begin(cut)
    }

    fn activate(begun: Self::Begun, cut: &mut ShardedOwnerWriteCut<'_>) {
        AdministrativeRemovalControl::activate(begun, cut);
    }
}

impl<'authority> SharedOwnerRemovalControl
    for super::super::shard::StagedPeerIngressFence<'authority>
{
    type Begun = super::super::shard::BegunPeerIngressFence<'authority>;

    fn extend_final_support(
        &self,
        _entries: &ShardedOwnerMap,
        _reads: &mut ShardReadSupport,
        writes: &mut ShardWriteSupport,
    ) {
        self.extend_final_write_support(writes);
    }

    fn index_prestate_is_fresh(
        &self,
        indexes: &IndexDelta,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        self.peer()
            .zip(self.stage_id())
            .is_some_and(|(peer, stage_id)| {
                indexes.prestate_is_fresh_for_peer_revocation(entries, cut, peer, stage_id)
            })
    }

    fn prestate_is_fresh(
        &self,
        _entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        super::super::shard::StagedPeerIngressFence::prestate_is_fresh(self, cut)
    }

    fn begin(
        self,
        cut: &mut ShardedOwnerWriteCut<'_>,
    ) -> Result<Self::Begun, super::ingress::ConcurrentRetainedIngressError> {
        self.begin_bank_commit(cut).map_err(|error| match error {
            // The exact reserved slot and routed Hidden row were both
            // revalidated under this cut. A mismatch is structural; the
            // already-begun resource permit faults on Drop by design.
            PeerBanError::Contention => super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::MembershipProjection,
            ),
            PeerBanError::Faulted => super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::MembershipProjection,
            ),
            PeerBanError::CounterExhausted => {
                super::ingress::ConcurrentRetainedIngressError::Fault(
                    AuthorityFault::CounterExhausted,
                )
            }
        })
    }

    fn activate(begun: Self::Begun, cut: &mut ShardedOwnerWriteCut<'_>) {
        begun.activate(cut);
    }
}

enum SchedulerApplyPermit<'reservation, 'frontier> {
    Noop,
    Reserved {
        reservation: &'reservation mut super::super::scheduler::ReadySlotReservation,
        delta: SchedulerBatchDelta,
    },
    ReadyReresolution(super::super::scheduler::StagedReadyReresolution<'reservation, 'frontier>),
    Staged(super::super::scheduler::StagedSchedulerBatch<'frontier>),
}

fn independent_scheduler_stage_error(error: SchedulerError) -> ConcurrentIndependentError {
    match error {
        SchedulerError::Stale => {
            ConcurrentIndependentError::ChangedCut(SettlementChangedCut::scheduler())
        }
        SchedulerError::Projection | SchedulerError::Arithmetic => {
            ConcurrentIndependentError::Fault(AuthorityFault::SchedulerProjection)
        }
    }
}

fn independent_dependency_prepare_error(
    error: DependencyPrepareError,
) -> ConcurrentIndependentError {
    match error {
        DependencyPrepareError::Stale => {
            ConcurrentIndependentError::ChangedCut(SettlementChangedCut::owner_or_projection())
        }
        DependencyPrepareError::Projection => {
            ConcurrentIndependentError::Fault(AuthorityFault::DependencyProjection)
        }
    }
}

impl<'reservation, 'frontier> SchedulerApplyPermit<'reservation, 'frontier> {
    fn stage(
        frontier: &'frontier Arc<Mutex<FairFrontier>>,
        scheduler: SchedulerBatchDelta,
        reservation: Option<&'reservation mut ReadySlotReservation>,
    ) -> Result<Self, ConcurrentIndependentError> {
        match reservation {
            None if scheduler.is_empty() => Ok(Self::Noop),
            Some(reservation) if scheduler.is_shared_acceptance_removal_only() => {
                Ok(Self::Reserved {
                    reservation,
                    delta: scheduler,
                })
            }
            Some(reservation) => super::super::scheduler::StagedReadyReresolution::stage(
                frontier,
                scheduler,
                reservation,
            )
            .map(Self::ReadyReresolution)
            .map_err(independent_scheduler_stage_error),
            None => super::super::scheduler::StagedSchedulerBatch::stage_primary_replacements(
                frontier, scheduler,
            )
            .map(Self::Staged)
            .map_err(independent_scheduler_stage_error),
        }
    }

    /// Win any time-sensitive Ready claim immediately before the owner cut
    /// mutates. A staged batch already owns its scheduler premises until
    /// publication, so it has no second freshness read to perform here.
    fn begin_commit(&self, frontier: &Arc<Mutex<FairFrontier>>) -> bool {
        match self {
            Self::Noop => true,
            Self::Reserved { reservation, delta } => reservation.prestate_is_fresh(frontier, delta),
            Self::ReadyReresolution(reresolution) => reresolution.begin_commit(frontier),
            Self::Staged(_) => true,
        }
    }

    fn apply(
        self,
        frontier: &Arc<Mutex<FairFrontier>>,
        token: &ApplyToken,
        owners: ShardedOwnerWriteCut<'_>,
    ) {
        match self {
            Self::Noop => drop(owners),
            Self::Reserved { reservation, delta } => {
                reservation.activate(frontier, delta);
                drop(owners);
            }
            Self::ReadyReresolution(reresolution) => {
                reresolution.activate(token, owners);
            }
            Self::Staged(staged) => {
                staged.activate(token, owners);
            }
        }
    }
}

/// The two scheduler paths that share one dependency gate and owner cut.
enum IndependentProjectionPermit<'reservation, 'authority> {
    Ordinary {
        dependency: PreparedDependencyBatch,
        scheduler: SchedulerApplyPermit<'reservation, 'authority>,
        gates: DependencyGateCut<'authority>,
    },
    SchedulerSealedRetained {
        dependency: PreparedDependencyBatch,
        scheduler: super::super::scheduler::StagedSchedulerBatch<'authority>,
        gates: DependencyGateCut<'authority>,
    },
}

impl<'reservation, 'authority> IndependentProjectionPermit<'reservation, 'authority> {
    fn stage(
        authority: &'authority TxPoolAuthority,
        retained: bool,
        scheduler: SchedulerBatchDelta,
        dependency: DependencyBatchDelta,
        gate_support: DependencyGateSupport,
        reservation: Option<&'reservation mut ReadySlotReservation>,
    ) -> Result<Self, ConcurrentIndependentError> {
        if retained {
            if reservation.is_some() || !dependency.is_retained_insertion_shape() {
                return Err(ConcurrentIndependentError::Fault(
                    AuthorityFault::MembershipProjection,
                ));
            }
            let scheduler =
                super::super::scheduler::StagedSchedulerBatch::stage_primary_replacements(
                    &authority.scheduler,
                    scheduler,
                )
                .map_err(independent_scheduler_stage_error)?;
            let gates = authority.entries.dependency_gate_cut(gate_support);
            let dependency = PreparedDependencyBatch::prepare_with_gates(
                &authority.dependencies,
                dependency,
                &gates,
            )
            .map_err(independent_dependency_prepare_error)?
            .require_retained_insertion_shape()
            .map_err(|_| ConcurrentIndependentError::Fault(AuthorityFault::DependencyProjection))?;
            return Ok(Self::SchedulerSealedRetained {
                dependency,
                scheduler,
                gates,
            });
        }

        let scheduler = SchedulerApplyPermit::stage(&authority.scheduler, scheduler, reservation)?;
        let gates = authority.entries.dependency_gate_cut(gate_support);
        let dependency = PreparedDependencyBatch::prepare_shared_independent(
            &authority.dependencies,
            dependency,
            &gates,
        )
        .map_err(independent_dependency_prepare_error)?;
        Ok(Self::Ordinary {
            dependency,
            scheduler,
            gates,
        })
    }

    fn extend_final_support(&self, reads: &mut ShardReadSupport, writes: &mut ShardWriteSupport) {
        let dependency = match self {
            Self::Ordinary { dependency, .. }
            | Self::SchedulerSealedRetained { dependency, .. } => dependency,
        };
        dependency.extend_final_read_support(reads);
        dependency.extend_final_write_support(writes);
    }

    fn extend_final_relation_support(
        &self,
        reads: &mut ShardReadSupport,
        writes: &mut ShardWriteSupport,
    ) {
        let dependency = match self {
            Self::Ordinary { dependency, .. }
            | Self::SchedulerSealedRetained { dependency, .. } => dependency,
        };
        dependency.extend_final_relation_read_support(reads);
        dependency.extend_final_relation_write_support(writes);
    }

    fn gates(&self) -> &DependencyGateCut<'_> {
        match self {
            Self::Ordinary { gates, .. } | Self::SchedulerSealedRetained { gates, .. } => gates,
        }
    }

    fn prestate_is_fresh(
        &self,
        relations: &ShardedDependencyRelationWriteCut<'_>,
        owners: &ShardedOwnerWriteCut<'_>,
    ) -> bool {
        match self {
            Self::Ordinary { dependency, .. }
            | Self::SchedulerSealedRetained { dependency, .. } => {
                dependency.prestate_is_fresh(relations, owners)
            }
        }
    }

    fn wake_projection_before(
        &self,
        authority: &TxPoolAuthority,
    ) -> Result<Option<AuthorityWakeProjection>, ConcurrentIndependentError> {
        match self {
            Self::Ordinary { .. } => Ok(None),
            Self::SchedulerSealedRetained { scheduler, .. } => scheduler
                .wake_projection_before()
                .map(|wake| authority.wake_projection_with_scheduler_without_effect(wake))
                .map(Some)
                .ok_or(ConcurrentIndependentError::Fault(
                    AuthorityFault::SchedulerProjection,
                )),
        }
    }

    fn ready_reserved(&self) -> bool {
        matches!(
            self,
            Self::Ordinary {
                scheduler: SchedulerApplyPermit::Reserved { .. }
                    | SchedulerApplyPermit::ReadyReresolution(_),
                ..
            }
        )
    }

    fn begin_commit(&self, frontier: &Arc<Mutex<FairFrontier>>) -> bool {
        match self {
            Self::Ordinary { scheduler, .. } => scheduler.begin_commit(frontier),
            Self::SchedulerSealedRetained { .. } => true,
        }
    }

    fn activate(
        self,
        _entries: &ShardedOwnerMap,
        frontier: &Arc<Mutex<FairFrontier>>,
        token: &ApplyToken,
        mut relations: ShardedDependencyRelationWriteCut<'_>,
        mut owners: ShardedOwnerWriteCut<'_>,
    ) -> DependencyApplyOutcome {
        match self {
            Self::Ordinary {
                dependency,
                scheduler,
                gates: _gates,
            } => {
                let outcome = dependency.apply_in_cut(&mut relations, &mut owners);
                drop(relations);
                #[cfg(test)]
                {
                    _entries.enter_concurrent_removal_probe();
                    _entries.enter_shared_owner_commit_probe();
                }
                scheduler.apply(frontier, token, owners);
                outcome
            }
            Self::SchedulerSealedRetained {
                dependency,
                scheduler,
                gates: _gates,
            } => {
                let outcome = dependency.apply_in_cut(&mut relations, &mut owners);
                drop(relations);
                #[cfg(test)]
                {
                    _entries.enter_concurrent_removal_probe();
                    _entries.enter_shared_owner_commit_probe();
                    _entries.enter_shared_ingress_probe(
                        super::super::shard::SharedIngressProbePhase::FinalCutBeforeActivation,
                    );
                }
                scheduler.activate(token, owners);
                outcome
            }
        }
    }
}

pub(super) fn commit_independent(
    plan: PreparedIndependentApply<'_>,
) -> Result<CommittedDelta, ConcurrentIndependentError> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_ready_job_rows(
    authority: &TxPoolAuthority,
    delta: IndependentDelta,
    support: super::super::shard::ShardApplySupport,
    reservation: &mut super::super::scheduler::ReadySlotReservation,
) -> Result<super::ReadyCommittedRows, ConcurrentIndependentError> {
    PreparedIndependentApply::apply_shared_rows(
        authority,
        &ApplyToken(()),
        delta,
        support,
        Some(reservation),
    )
}

pub(super) fn commit_unreserved_shared_rows(
    authority: &TxPoolAuthority,
    delta: IndependentDelta,
    support: super::super::shard::ShardApplySupport,
) -> Result<super::ReadyCommittedRows, ConcurrentIndependentError> {
    PreparedIndependentApply::apply_shared_rows(authority, &ApplyToken(()), delta, support, None)
}

pub(super) fn commit_shared_peer_revocation(
    plan: super::ingress::PreparedSharedPeerRevocation<'_>,
) -> Result<
    super::ingress::CommittedRetainedAdmissionBatch,
    super::ingress::ConcurrentPeerRevocationFailure,
> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_shared_owner_removal<C>(
    plan: PreparedSharedOwnerRemoval<'_, C>,
) -> Result<super::CommittedDelta, super::ingress::ConcurrentOwnerRemovalFailure>
where
    C: SharedOwnerRemovalControl,
{
    plan.apply_with(&ApplyToken(()))
}

pub(super) struct OwnerResourceUpdate {
    key: RawTxHash,
    after: Option<OwnedTx>,
}

impl OwnerResourceUpdate {
    pub(super) fn new(key: RawTxHash, after: Option<OwnedTx>) -> Self {
        Self { key, after }
    }
}

pub(super) struct PreparedOwnerResourceDelta<I> {
    updates: I,
    resources: ResourceBatchPlan,
    proposed_counts: super::super::shard::ShardProposedCountPlan,
    support: ShardWriteSupport,
    owner_source_advance: Option<ShardOwnerSourceAdvance>,
}

impl<I> PreparedOwnerResourceDelta<I> {
    pub(super) fn batch(
        updates: I,
        resources: ResourceBatchPlan,
        proposed_counts: super::super::shard::ShardProposedCountPlan,
        support: ShardWriteSupport,
    ) -> Self {
        Self {
            updates,
            resources,
            proposed_counts,
            support,
            owner_source_advance: None,
        }
    }

    pub(super) fn with_owner_source_advance(mut self, advance: ShardOwnerSourceAdvance) -> Self {
        self.owner_source_advance = Some(advance);
        self
    }
}

impl PreparedOwnerResourceDelta<std::iter::Once<OwnerResourceUpdate>> {
    pub(super) fn single(
        update: OwnerResourceUpdate,
        resources: ResourceBatchPlan,
        support: ShardWriteSupport,
    ) -> Self {
        Self {
            updates: std::iter::once(update),
            resources,
            proposed_counts: Default::default(),
            support,
            owner_source_advance: None,
        }
    }
}

/// Resource transition sealed by the only owner-to-Nowhere compiler. Shared
/// Apply may rebase its per-shard subtraction on the exact current owners;
/// exclusive Apply may consume the already-compiled absolute shard targets.
/// Its private constructor prevents insertion/replacement plans from creating
/// this carrier.
pub(in crate::authority) struct OwnerRemovalResourcePlan {
    plan: ResourceBatchPlan,
    owners: Vec<(RawTxHash, ChargeRecord)>,
}

impl OwnerRemovalResourcePlan {
    fn new(plan: ResourceBatchPlan, owners: Vec<(RawTxHash, ChargeRecord)>) -> Self {
        Self { plan, owners }
    }

    pub(super) fn releases_preaccepted_active_work(&self) -> bool {
        self.plan.releases_preaccepted_active_work()
    }

    pub(super) fn shard_plan(&self) -> &ShardResourcePlan {
        self.plan.shard_plan()
    }

    pub(super) fn into_exclusive_plan(self) -> ResourceBatchPlan {
        self.plan
    }

    pub(super) fn rebase_shared_removal(
        self,
        entries: &ShardedOwnerMap,
        cut: &ShardedOwnerWriteCut<'_>,
        hashes: &[RawTxHash],
    ) -> Result<(ShardResourcePlan, ResourceCapacityCommit), ResourceError> {
        if hashes.len() != self.owners.len()
            || hashes
                .iter()
                .zip(&self.owners)
                .any(|(hash, (expected_hash, expected_charge))| {
                    hash != expected_hash
                        || cut.owner(entries, hash).map(OwnedTx::charge_record)
                            != Some(*expected_charge)
                })
        {
            return Err(ResourceError::ExistingChargeMismatch);
        }
        let (mut shards, capacity) = self.plan.into_shared_commit_parts();
        cut.rebase_owner_removal_resource_plan(&mut shards)?;
        Ok((shards, capacity))
    }
}

/// Physical-allocation capability for resource planning.
///
/// This deliberately does not implement `DerefMut`: callers can compile
/// resource deltas, but cannot replace or otherwise mutate the authoritative
/// ledger directly.
pub(in crate::authority) struct ResourcePlanner<'state> {
    entries: &'state ShardedOwnerMap,
    ledger: &'state ResourceLedger,
}

impl ResourcePlanner<'_> {
    fn compile_selected_transition<'change>(
        &self,
        changes: impl ExactSizeIterator<
            Item = (
                &'change RawTxHash,
                Option<ChargeRecord>,
                Option<ChargeRecord>,
            ),
        >,
    ) -> Result<(ShardResourcePlan, ResourceTotals, ResourceTotals), ResourceError> {
        let mut keys = HashSet::with_capacity(changes.len());
        let mut projections = Vec::with_capacity(changes.len());
        let mut before_totals = ResourceTotals::default();
        let mut after_totals = ResourceTotals::default();

        for (key, expected, after) in changes {
            expected.map(ChargeRecord::validate).transpose()?;
            after.map(ChargeRecord::validate).transpose()?;
            if !keys.insert(key) {
                return Err(ResourceError::DuplicateChange);
            }
            if self.entries.get(key).as_deref().map(OwnedTx::charge_record) != expected {
                return Err(ResourceError::ExistingChargeMismatch);
            }
            let before = ChargeProjection::from_validated(expected)?;
            let after = ChargeProjection::from_validated(after)?;
            before_totals = before_totals.checked_add(before)?;
            after_totals = after_totals.checked_add(after)?;
            projections.push((key, before, after));
        }

        let shards = self.entries.plan_resource_transitions(projections)?;
        Ok((shards, before_totals, after_totals))
    }

    pub(in crate::authority) fn membership_accepted_transition_fits(
        &self,
        released: super::super::resources::AcceptedResources,
        added: super::super::resources::AcceptedResources,
    ) -> Result<bool, ResourceError> {
        self.ledger
            .membership_accepted_transition_fits(released, added)
    }

    pub(in crate::authority) fn limits(&self) -> ResourceLimits {
        self.ledger.limits()
    }

    pub(in crate::authority) fn capacity_observation(
        &self,
    ) -> super::super::resources::ResourceCapacityObservation {
        self.ledger.capacity_observation()
    }

    pub(in crate::authority) fn capacity_wait_identity(&self) -> ResourceCapacityWaitIdentity {
        self.ledger.capacity_wait_identity()
    }

    pub(in crate::authority) fn plan_removal_batch(
        &self,
        changes: Vec<(RawTxHash, ChargeRecord)>,
    ) -> Result<OwnerRemovalResourcePlan, ResourceError> {
        let (shards, before, after) = self.compile_selected_transition(
            changes
                .iter()
                .map(|(key, before)| (key, Some(*before), None)),
        )?;
        let plan = self.ledger.reserve_plan(shards, before, after)?;
        Ok(OwnerRemovalResourcePlan::new(plan, changes))
    }

    pub(in crate::authority) fn plan_replace(
        &self,
        key: RawTxHash,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let (shards, before, after) =
            self.compile_selected_transition(std::iter::once((&key, expected, after)))?;
        self.ledger.reserve_plan(shards, before, after)
    }

    pub(in crate::authority) fn plan_batch(
        &self,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let (shards, before, after) = self.compile_selected_transition(
            changes
                .iter()
                .map(|(key, before, after)| (key, *before, *after)),
        )?;
        self.ledger.reserve_plan(shards, before, after)
    }

    pub(in crate::authority) fn plan_ordered_batch(
        &self,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
        envelope: OrderedResourceEnvelope,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let (shards, before, after) = self.compile_selected_transition(
            changes
                .iter()
                .map(|(key, before, after)| (key, *before, *after)),
        )?;
        self.ledger
            .reserve_ordered_plan(shards, before, after, envelope)
    }

    pub(in crate::authority) fn plan_direct_accepted_insertion_batch(
        &self,
        changes: Vec<(RawTxHash, ChargeRecord)>,
    ) -> Result<ResourceBatchPlan, DirectAcceptedInsertionError> {
        let (shards, before, after) = self.compile_selected_transition(
            changes.iter().map(|(key, after)| (key, None, Some(*after))),
        )?;
        self.ledger
            .reserve_direct_accepted_plan(shards, before, after)
    }
}

/// Physical-allocation capability for index planning.
pub(in crate::authority) struct IndexPlanner<'state> {
    indexes: &'state AuthorityIndexes,
}

impl IndexPlanner<'_> {
    pub(in crate::authority) fn capture_retained_premise<'entry>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
        cut: &ShardedOwnerWriteCut<'_>,
    ) -> Result<super::super::indexes::RetainedIndexPremise, IndexError> {
        self.indexes.capture_retained_premise(changes, cut)
    }

    pub(in crate::authority) fn plan_retained_replacements<'entry>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
        premise: super::super::indexes::RetainedIndexPremise,
    ) -> Result<IndexDelta, IndexError> {
        self.indexes.plan_retained_replacements(changes, premise)
    }

    pub(in crate::authority) fn plan_replace(
        &self,
        key: &RawTxHash,
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
    ) -> Result<IndexDelta, IndexError> {
        self.indexes.plan_replace(key, before, after)
    }

    pub(in crate::authority) fn plan_replacements<'entry>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'entry RawTxHash,
                Option<&'entry OwnedTx>,
                Option<&'entry OwnedTx>,
            ),
        >,
    ) -> Result<IndexDelta, IndexError> {
        self.indexes.plan_replacements(changes)
    }
}

/// Physical-allocation capability for effect-journal planning.
pub(in crate::authority) struct EffectPlanner<'state> {
    effects: MutexGuard<'state, EffectLog>,
    /// Borrowed only so the uncommon staged-suffix arm can clone its rollback
    /// owner. The direct no-prefix path performs no Arc refcount operation.
    /// Planning continues under `effects`; no second lock is acquired here.
    log: &'state Arc<Mutex<EffectLog>>,
}

impl EffectPlanner<'_> {
    pub(in crate::authority) fn plan_publication(
        &mut self,
        publication: &EffectPublication,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.effects
            .plan_publication_with_log(self.log, publication, sequence)
    }

    pub(in crate::authority) fn plan_chain_rebuildable(
        &mut self,
        effects: Vec<CommittedEffect>,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.effects
            .plan_chain_rebuildable(self.log, effects, sequence)
    }
}

/// Physical-allocation capability for peer-ban planning.
#[cfg(test)]
pub(in crate::authority) struct PeerBanPlanner<'state> {
    peer_bans: &'state PeerBanSlotBank,
}

#[cfg(test)]
impl PeerBanPlanner<'_> {
    pub(in crate::authority) fn plan_record(
        &mut self,
        peer: ckb_network::PeerIndex,
        observed_at: Instant,
    ) -> Result<PeerBanDelta, PeerBanError> {
        self.peer_bans.plan_exclusive_record(peer, observed_at)
    }
}

impl TxPoolAuthority {
    pub(in crate::authority) fn from_runtime(
        _init: crate::authority::runtime::AuthorityInitToken,
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
            AuthorityShardRouter::new(),
        ))
    }

    fn assemble(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effects: EffectLog,
        membership_config: MembershipConfig,
        chain_view: ChainViewId,
        router: AuthorityShardRouter,
    ) -> Self {
        let entries = ShardedOwnerMap::new(router);
        let dependencies = DependencyFrontier::for_entries(&entries);
        Self {
            state: AuthorityState {
                generation: PoolGeneration(0),
                chain_view,
                owner_resources: OwnerResourceAuthority {
                    entries: entries.clone(),
                    resources: ResourceLedger::new(limits),
                },
                indexes: AuthorityIndexes::for_entries(&entries),
                source_versions: AuthoritySourceVersions::initial(),
                membership: MembershipProjection::for_entries(&entries),
                scheduler: Arc::new(Mutex::new(FairFrontier::new(verify_order))),
                dependencies,
                effects: Arc::new(Mutex::new(effects)),
                peer_bans: PeerBanSlotBank::default(),
                membership_config,
                clocks: Arc::new(AuthorityClockBank::first()),
            },
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn from_test(
        _init: &super::test_support::AuthorityTestToken,
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effects: EffectLog,
        membership_config: MembershipConfig,
        chain_view: ChainViewId,
    ) -> Self {
        Self::assemble(
            limits,
            verify_order,
            effects,
            membership_config,
            chain_view,
            AuthorityShardRouter::new(),
        )
    }

    pub(super) fn write<'authority>(
        &'authority mut self,
        _token: &ApplyToken,
    ) -> &'authority mut AuthorityState {
        &mut self.state
    }

    pub(super) fn commit_owner_resources<I>(
        &self,
        token: &ApplyToken,
        delta: PreparedOwnerResourceDelta<I>,
        retired: &mut RetiredOwners,
    ) where
        I: IntoIterator<Item = OwnerResourceUpdate>,
    {
        let owner_resources = &self.state.owner_resources;
        let mut owners = owner_resources.entries.write_cut(delta.support);
        for update in delta.updates {
            let shard = owner_resources.entries.owner_shard(&update.key);
            let previous = owners.replace(shard, update.key, update.after);
            if let Some(owner) = previous {
                retired.push(owner);
            }
        }
        owners.apply_proposed_counts(delta.proposed_counts);
        let capacity = delta.resources.apply_shards(&mut owners);
        if let Some(advance) = delta.owner_source_advance {
            owners.apply_owner_source_advance(advance);
        }
        drop(owners);
        let _health = capacity.commit();
        let _ = token;
    }

    /// Commit the owner/resource fact and the two owner-derived projections
    /// through one physical shard cut. The logical Apply order is unchanged;
    /// this only removes repeated acquisition of overlapping fixed shards.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn commit_owner_resources_indexes_membership<I>(
        &self,
        token: &ApplyToken,
        delta: PreparedOwnerResourceDelta<I>,
        mut indexes: IndexDelta,
        membership: ProjectionDelta,
        owner_source_advance: ShardOwnerSourceAdvance,
        retired: &mut RetiredOwners,
    ) where
        I: IntoIterator<Item = OwnerResourceUpdate>,
    {
        let owner_resources = &self.state.owner_resources;
        let entries = &owner_resources.entries;
        let mut support = delta.support;
        support.include(indexes.sharded_write_support(entries));
        support.include(membership.sharded_write_support(entries));
        let mut owners = entries.write_cut(support);
        for update in delta.updates {
            let shard = entries.owner_shard(&update.key);
            let previous = owners.replace(shard, update.key, update.after);
            if let Some(owner) = previous {
                retired.push(owner);
            }
        }
        owners.apply_proposed_counts(delta.proposed_counts);
        let capacity = delta.resources.apply_shards(&mut owners);
        let _health = capacity.commit();
        indexes.apply_sharded(entries, &mut owners);
        membership.apply_sharded(entries, &mut owners);
        owners.apply_owner_source_advance(owner_source_advance);
        drop(owners);
        let _ = token;
    }

    /// Commit one canonical independent owner batch through its complete
    /// physical shard cut. The final mixed-cut OCC revalidates exact owner
    /// versions and every captured projection prestate before changing a row.
    /// Planning has already completed the batch's reportable fallible
    /// reservations; Apply advances the sealed owner and derived facts before
    /// opening the cut.
    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments are the single production IndependentDelta split into its physical authorities; a wrapper would duplicate that semantic carrier"
    )]
    pub(super) fn commit_shared_independent_rows(
        &self,
        token: &ApplyToken,
        owner_cuts: Vec<IndependentOwnerCut>,
        resources: Option<ResourceBatchPlan>,
        proposed_counts: super::super::shard::ShardProposedCountPlan,
        support: super::super::shard::ShardApplySupport,
        mut indexes: IndexDelta,
        membership: ProjectionDelta,
        dependency: DependencyBatchDelta,
        mut sources: SourceVersionDelta,
        owner_source_counts: ShardOwnerSourceCounts,
        scheduler: SchedulerBatchDelta,
        reservation: Option<&mut super::super::scheduler::ReadySlotReservation>,
        retired: &mut RetiredOwners,
    ) -> Result<
        (
            DependencyApplyOutcome,
            ResourceCommitHealth,
            Option<AuthorityWakeProjection>,
        ),
        ConcurrentIndependentError,
    > {
        let entries = &self.state.owner_resources.entries;
        let retained = !owner_cuts.is_empty()
            && owner_cuts
                .iter()
                .all(|owner| owner.removal_revision.is_some())
            && dependency.is_retained_insertion_shape();
        let (proposal_source_changed, transaction_source_changed) =
            sources.take_template_selection();
        if !sources.is_empty() {
            return Err(ConcurrentIndependentError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let mut gate_support = dependency.dependency_gate_support(entries);
        gate_support.include(membership.dependency_gate_support(entries));
        let projection = IndependentProjectionPermit::stage(
            self,
            retained,
            scheduler,
            dependency,
            gate_support,
            reservation,
        )?;
        #[cfg(test)]
        if retained {
            entries.enter_shared_ingress_probe(
                super::super::shard::SharedIngressProbePhase::ProjectionPreparedBeforeOwnerCut,
            );
        }
        let retained_before = projection.wake_projection_before(self)?;
        if !membership.dependency_aggregate_is_fresh(entries, projection.gates()) {
            return Err(ConcurrentIndependentError::ChangedCut(
                SettlementChangedCut::owner_or_projection(),
            ));
        }
        let mut reads = support.reads();
        let mut writes = support.writes();
        let mut relation_reads = ShardReadSupport::default();
        let mut relation_writes = ShardWriteSupport::default();
        projection.extend_final_relation_support(&mut relation_reads, &mut relation_writes);
        projection.extend_final_support(&mut reads, &mut writes);
        let relations = entries.dependency_relation_mixed_cut(relation_reads, relation_writes);
        let mut owners = entries.mixed_cut(reads, writes);
        let owners_fresh = owner_cuts
            .iter()
            .all(|owner| owner.is_fresh(entries, &owners));
        let proposed_fresh = owners.proposed_count_plan_is_fresh(&proposed_counts);
        let resources_fresh = resources
            .as_ref()
            .is_none_or(|resources| owners.resource_plan_is_fresh(resources.shard_plan()));
        let indexes_fresh = indexes.prestate_is_fresh(entries, &owners);
        let membership_fresh = membership.point_prestate_is_fresh(entries, &owners);
        let dependency_fresh = projection.prestate_is_fresh(&relations, &owners);
        if !(owners_fresh
            && proposed_fresh
            && resources_fresh
            && indexes_fresh
            && membership_fresh
            && dependency_fresh)
        {
            return Err(ConcurrentIndependentError::ChangedCut(
                SettlementChangedCut::owner_or_projection(),
            ));
        }
        if owner_source_counts.changed() != (proposal_source_changed, transaction_source_changed) {
            return Err(ConcurrentIndependentError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let source_advance = owners
            .prepare_owner_source_advance(owner_source_counts)
            .ok_or(ConcurrentIndependentError::Fault(
                AuthorityFault::CounterExhausted,
            ))?;
        let (resource_shards, capacity) = match resources {
            Some(resources) => {
                let (resource_shards, capacity) = resources.into_shared_commit_parts();
                (Some(resource_shards), Some(capacity))
            }
            None => (None, None),
        };
        let ready_reserved = projection.ready_reserved();
        if ready_reserved
            && capacity
                .as_ref()
                .is_some_and(ResourceCapacityCommit::seals_active_work_revision)
        {
            drop(owners);
            drop(relations);
            drop(projection);
            return Err(ConcurrentIndependentError::Fault(
                AuthorityFault::ResourceProjection,
            ));
        }
        // Revalidate the scheduler before beginning the capacity transition.
        // A stale Ready claim is still safely returnable here. Capacity begin
        // is the final fallible pre-owner operation; a Ready acceptance owns
        // no active-work revision, so every ordinary contention was already
        // resolved before its Fresh -> Committing priority linearization.
        if !projection.begin_commit(&self.state.scheduler) {
            drop(owners);
            drop(relations);
            drop(projection);
            return Err(ConcurrentIndependentError::ChangedCut(
                SettlementChangedCut::scheduler(),
            ));
        }
        let capacity = match capacity {
            Some(capacity) => match capacity.begin() {
                Ok(capacity) => Some(capacity),
                Err(ResourceCapacityBeginError::StaleActiveWorkRevision) if !ready_reserved => {
                    drop(owners);
                    drop(relations);
                    drop(projection);
                    return Err(ConcurrentIndependentError::ChangedCut(
                        SettlementChangedCut::resource_capacity(),
                    ));
                }
                Err(
                    ResourceCapacityBeginError::StaleActiveWorkRevision
                    | ResourceCapacityBeginError::Capacity(_),
                ) => {
                    drop(owners);
                    drop(relations);
                    drop(projection);
                    return Err(ConcurrentIndependentError::Fault(
                        AuthorityFault::ResourceProjection,
                    ));
                }
            },
            None => None,
        };
        for owner in owner_cuts {
            let IndependentOwnerAction::Replace(after) = owner.action else {
                continue;
            };
            let shard = entries.owner_shard(&owner.key);
            let previous = owners.replace(shard, owner.key, after);
            if let Some(owner) = previous {
                retired.push(owner);
            }
        }
        owners.apply_proposed_counts(proposed_counts);
        if let Some(resource_shards) = resource_shards {
            owners.apply_resource_plan(resource_shards);
        }
        indexes.apply_sharded(entries, &mut owners);
        membership.apply_sharded(entries, &mut owners);
        owners.apply_owner_source_advance(source_advance);
        let dependency =
            projection.activate(entries, &self.state.scheduler, token, relations, owners);
        let resource_health =
            capacity.map_or(ResourceCommitHealth::Healthy, |capacity| capacity.finish());
        let _ = token;
        Ok((dependency, resource_health, retained_before))
    }

    pub(super) fn commit_shared_owner_removal_rows<C>(
        &self,
        token: &ApplyToken,
        removal: OwnerRemovalBatch,
        staged: super::ingress::StagedRetainedIngress<'_>,
        control: C,
    ) -> Result<
        (DependencyApplyOutcome, ResourceCommitHealth, RetiredOwners),
        super::ingress::ConcurrentRetainedIngressError,
    >
    where
        C: SharedOwnerRemovalControl,
    {
        let OwnerRemovalBatch {
            hashes,
            expected_versions,
            owners,
            resources,
            mut membership,
            scheduler: _,
            dependency: _,
            mut retired,
        } = removal;
        let DerivedOwnerDelta {
            mut indexes,
            mut sources,
            template_sources,
        } = owners;
        let source_counts = template_sources.counts();
        let source_changes = sources.take_template_selection();
        if source_counts.changed() != source_changes || !sources.is_empty() {
            return Err(super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let entries = &self.state.owner_resources.entries;
        let mut proposed_counts = membership.take_proposed_counts();
        let mut support = entries.owner_resource_write_support(
            hashes.iter(),
            &proposed_counts,
            resources.shard_plan(),
        );
        support.include(indexes.sharded_write_support(entries));
        support.include(membership.sharded_write_support(entries));
        let mut reads = ShardReadSupport::default();
        let mut relation_reads = ShardReadSupport::default();
        let mut relation_writes = ShardWriteSupport::default();
        staged.extend_final_relation_support(&mut relation_reads, &mut relation_writes);
        staged.extend_final_support(&mut reads, &mut support);
        control.extend_final_support(entries, &mut reads, &mut support);
        if !membership.dependency_aggregate_is_fresh(entries, staged.dependency_gates()) {
            return Err(super::ingress::ConcurrentRetainedIngressError::Stale);
        }
        // Canonical ascending acquisition is sparse-write/full-read for Remote
        // expiry and sparse-write for peer revocation. The fixed 64-shard walk
        // is the only global ordering cost; no global writer is acquired.
        let relations = entries.dependency_relation_mixed_cut(relation_reads, relation_writes);
        let mut cut = entries.mixed_cut(reads, support);
        let source_advance = cut.prepare_owner_source_advance(source_counts).ok_or(
            super::ingress::ConcurrentRetainedIngressError::Fault(AuthorityFault::CounterExhausted),
        )?;
        let owners_fresh = hashes
            .iter()
            .zip(&expected_versions)
            .all(|(hash, version)| {
                cut.owner_version(entries.owner_shard(hash), hash) == Some(*version)
            });
        let indexes_fresh = control.index_prestate_is_fresh(&indexes, entries, &cut);
        let membership_fresh = membership.point_prestate_is_fresh(entries, &cut);
        let staged_fresh = staged.prestate_is_fresh(&relations, &cut);
        let control_fresh = control.prestate_is_fresh(entries, &cut);
        if !(owners_fresh && indexes_fresh && membership_fresh && staged_fresh && control_fresh) {
            return Err(super::ingress::ConcurrentRetainedIngressError::Stale);
        }
        if !cut.proposed_removal_plan_matches(entries, &hashes, &proposed_counts) {
            return Err(super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let (resource_shards, capacity) = resources
            .rebase_shared_removal(entries, &cut, &hashes)
            .map_err(|error| match error {
            ResourceError::ExistingChargeMismatch
            | ResourceError::Arithmetic
            | ResourceError::PreAcceptedLimit
            | ResourceError::RemoteLimit
            | ResourceError::PeerLimit(_)
            | ResourceError::ReplacementHistoryLimit
            | ResourceError::AcceptedLimit
            | ResourceError::DuplicateChange
            | ResourceError::ComputeEnvelope
            | ResourceError::AttributionMismatch
            | ResourceError::CapacityBankFault => {
                super::ingress::ConcurrentRetainedIngressError::Fault(
                    AuthorityFault::ResourceProjection,
                )
            }
        })?;
        cut.rebase_proposed_removal_plan(&mut proposed_counts)
            .map_err(|_| {
                super::ingress::ConcurrentRetainedIngressError::Fault(
                    AuthorityFault::MembershipProjection,
                )
            })?;
        let capacity = capacity.begin().map_err(|error| match error {
            ResourceCapacityBeginError::StaleActiveWorkRevision => {
                super::ingress::ConcurrentRetainedIngressError::Stale
            }
            ResourceCapacityBeginError::Capacity(_) => {
                super::ingress::ConcurrentRetainedIngressError::Fault(
                    AuthorityFault::ResourceProjection,
                )
            }
        })?;
        let control = control.begin(&mut cut)?;
        for hash in hashes {
            let shard = entries.owner_shard(&hash);
            let Some(owner) = cut.replace(shard, hash, None) else {
                // The exact version of every distinct cohort owner was checked
                // above while this same physical cut remained held. Reaching
                // this branch is therefore a structural Apply contradiction,
                // not a legal stale outcome. Do not activate any projection;
                // dropping the begun linear permits faults this generation.
                return Err(super::ingress::ConcurrentRetainedIngressError::Fault(
                    AuthorityFault::MembershipProjection,
                ));
            };
            retired.push(owner);
        }
        cut.apply_proposed_counts(proposed_counts);
        cut.apply_resource_plan(resource_shards);
        indexes.apply_sharded(entries, &mut cut);
        membership.apply_sharded(entries, &mut cut);
        cut.apply_owner_source_advance(source_advance);
        C::activate(control, &mut cut);
        let dependency = staged.activate(entries, token, relations, cut);
        let resource_health = capacity.finish();
        Ok((dependency, resource_health, retired))
    }

    pub(super) fn replace_owner_generation_resources(
        &mut self,
        token: &ApplyToken,
        retired_carrier: ShardedOwnerMap,
        resources: ResourceLedger,
    ) -> (ShardedOwnerMap, ResourceLedger) {
        let owner_resources = &mut self.state.owner_resources;
        owner_resources
            .entries
            .swap_generation_payload_with(&retired_carrier);
        let previous_resources = std::mem::replace(&mut owner_resources.resources, resources);
        let _ = token;
        (retired_carrier, previous_resources)
    }

    pub(super) fn reserve_primary_owner_insertions<'key>(
        &self,
        keys: impl IntoIterator<Item = &'key RawTxHash>,
    ) {
        self.state.owner_resources.entries.reserve_keys(keys);
    }

    pub(super) fn resources_for_plan(&self) -> ResourcePlanner<'_> {
        ResourcePlanner {
            entries: &self.state.owner_resources.entries,
            ledger: &self.state.owner_resources.resources,
        }
    }

    pub(in crate::authority) fn resource_capacity_wait_identity(
        &self,
    ) -> ResourceCapacityWaitIdentity {
        self.state
            .owner_resources
            .resources
            .capacity_wait_identity()
    }

    pub(super) fn indexes_for_plan(&self) -> IndexPlanner<'_> {
        IndexPlanner {
            indexes: &self.state.indexes,
        }
    }

    pub(super) fn effects_for_plan(&self) -> EffectPlanner<'_> {
        EffectPlanner {
            effects: self.state.effects.lock(),
            log: &self.state.effects,
        }
    }

    pub(super) fn reserve_membership_owner_insertions<'input, 'owner>(
        &self,
        inputs: impl IntoIterator<Item = &'input OutPoint>,
        owners: impl IntoIterator<Item = &'owner RawTxHash>,
    ) -> Result<(), PlanError> {
        self.state
            .membership
            .reserve_owner_insertion_capacity(inputs, owners)
    }

    pub(super) fn reserve_membership_child_row(
        &self,
        parent: &RawTxHash,
        additional: usize,
    ) -> Result<(), PlanError> {
        self.state.membership.reserve_child_row(parent, additional)
    }

    pub(super) fn reserve_membership_parent_row(
        &self,
        child: &RawTxHash,
        additional: usize,
    ) -> Result<(), PlanError> {
        self.state.membership.reserve_parent_row(child, additional)
    }

    pub(super) fn owner_derivation_parts(
        &self,
    ) -> (&ShardedOwnerMap, IndexPlanner<'_>, &AuthoritySourceVersions) {
        (
            &self.state.owner_resources.entries,
            IndexPlanner {
                indexes: &self.state.indexes,
            },
            &self.state.source_versions,
        )
    }

    pub(super) fn concurrent_owner_removal_plan_parts(
        &self,
    ) -> (
        &ShardedOwnerMap,
        ResourcePlanner<'_>,
        &DependencyFrontier,
        &AuthoritySourceVersions,
        IndexPlanner<'_>,
    ) {
        (
            &self.state.owner_resources.entries,
            ResourcePlanner {
                entries: &self.state.owner_resources.entries,
                ledger: &self.state.owner_resources.resources,
            },
            &self.state.dependencies,
            &self.state.source_versions,
            IndexPlanner {
                indexes: &self.state.indexes,
            },
        )
    }

    fn into_fresh_generation(self, dependency_maintenance_activated: bool) -> FreshGeneration {
        FreshGeneration {
            entries: self.state.owner_resources.entries,
            resources: self.state.owner_resources.resources,
            scheduler: self.state.scheduler,
            dependencies: self.state.dependencies,
            dependency_maintenance_activated,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn resources_for_test_plan(&self) -> ResourcePlanner<'_> {
        ResourcePlanner {
            entries: &self.state.owner_resources.entries,
            ledger: &self.state.owner_resources.resources,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn effects_for_test_plan(&mut self) -> EffectPlanner<'_> {
        EffectPlanner {
            effects: self.state.effects.lock(),
            log: &self.state.effects,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn peer_bans_for_test_plan(&self) -> PeerBanPlanner<'_> {
        PeerBanPlanner {
            peer_bans: &self.state.peer_bans,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn replace_membership_config_for_test(
        &mut self,
        _test: &super::test_support::AuthorityTestToken,
        membership_config: MembershipConfig,
    ) {
        self.state.membership_config = membership_config;
    }

    #[cfg(test)]
    pub(in crate::authority) fn replace_chain_view_for_test(
        &mut self,
        _test: &super::test_support::AuthorityTestToken,
        view: ChainViewId,
    ) {
        self.state.chain_view = view;
    }

    #[cfg(test)]
    pub(in crate::authority) fn replace_next_sequence_for_test(
        &mut self,
        _test: &super::test_support::AuthorityTestToken,
        sequence: ApplySequence,
    ) {
        let mut clocks = self.state.clocks.snapshot();
        clocks.next_sequence = sequence;
        self.state.clocks.replace_for_test(clocks);
    }

    #[cfg(test)]
    pub(in crate::authority) fn replace_peer_bans_for_test(
        &mut self,
        _test: &super::test_support::AuthorityTestToken,
        capacity: usize,
    ) {
        self.state.peer_bans = PeerBanSlotBank::with_limit_for_test(capacity);
    }

    #[cfg(test)]
    pub(in crate::authority) fn invalidate_peer_ban_stage_for_test(&self, stage_id: u64) -> bool {
        self.state
            .peer_bans
            .invalidate_reserved_stage_for_test(stage_id)
    }
}

impl ScratchAuthority {
    pub(super) fn assemble(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effects: EffectLog,
        membership_config: MembershipConfig,
        seed: ScratchAuthoritySeed,
    ) -> Self {
        let mut authority = TxPoolAuthority::assemble(
            limits,
            verify_order,
            effects,
            membership_config,
            seed.chain_view,
            seed.router,
        );
        authority.state.generation = seed.generation;
        authority.state.clocks = Arc::new(AuthorityClockBank::from_snapshot(seed.clocks));
        Self {
            authority,
            dependency_maintenance_activated: false,
        }
    }

    pub(super) fn resources(&self) -> &ResourceLedger {
        &self.authority.state.resources
    }

    pub(super) fn apply_charged_admission(
        &mut self,
        admission: ChargedAdmission,
    ) -> Result<(), PlanError> {
        let committed = self.authority.plan_charged_admission(admission)?.apply();
        self.dependency_maintenance_activated |= committed.into_scratch_dependency_wake();
        Ok(())
    }

    pub(super) fn clocks(&self) -> AuthorityClocks {
        self.authority.state.clocks.snapshot()
    }

    pub(super) fn into_fresh_generation(self) -> FreshGeneration {
        self.authority
            .into_fresh_generation(self.dependency_maintenance_activated)
    }
}

pub(super) fn commit(plan: PreparedApply<'_>) -> CommittedDelta {
    let token = ApplyToken(());
    plan.apply_with(&token)
}
