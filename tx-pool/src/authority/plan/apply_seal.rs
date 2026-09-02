use super::super::dependency::DependencyFinalization;
use super::super::resources::{
    ResourceCapacityBeginError, ResourceCapacityCommit, ResourceCapacityWaitIdentity,
    ResourceCommitHealth,
};
use super::super::shard::{
    AuthorityShardRouter, ShardOwnerSourceAdvance, ShardOwnerSourceCounts, ShardReadSupport,
    ShardWriteSupport, ShardedOwnerMap, ShardedOwnerWriteCut,
};
use super::*;
use ckb_util::parking_lot::{Mutex, MutexGuard};
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
    dependency_publication: super::FreshDependencyPublication,
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
pub(super) trait SharedOwnerRemovalControl {
    type Begun;

    fn extend_final_support(
        &self,
        entries: &ShardedOwnerMap,
        reads: &mut ShardReadSupport,
        writes: &mut ShardWriteSupport,
    );

    fn index_prestate_is_fresh(
        &self,
        indexes: &IndexDelta,
        entries: &ShardedOwnerMap,
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

enum SchedulerApplyPermit<'frontier> {
    Noop,
    Reserved {
        reservation: super::super::scheduler::ReadySlotReservation,
        delta: SchedulerBatchDelta,
    },
    Staged(super::super::scheduler::StagedSchedulerBatch<'frontier>),
}

enum SharedIndependentDependencyStage {
    Exact(StagedDependencyBatch),
    ReadyPhase(SealedReadyPhaseDependency),
}

impl SharedIndependentDependencyStage {
    fn visibility(&self) -> &StagedIngressVisibility {
        match self {
            Self::Exact(dependency) => dependency.visibility(),
            Self::ReadyPhase(dependency) => dependency.visibility(),
        }
    }

    fn extend_final_support(&self, reads: &mut ShardReadSupport, writes: &mut ShardWriteSupport) {
        match self {
            Self::Exact(dependency) => {
                dependency.extend_final_read_support(reads);
                dependency.extend_final_write_support(writes);
            }
            Self::ReadyPhase(dependency) => dependency.extend_final_read_support(reads),
        }
    }

    fn prestate_is_fresh(&self, cut: &ShardedOwnerWriteCut<'_>) -> bool {
        match self {
            Self::Exact(dependency) => dependency.prestate_is_fresh(cut),
            Self::ReadyPhase(dependency) => dependency.prestate_is_fresh(cut),
        }
    }

    fn activate_in_cut(self, cut: &mut ShardedOwnerWriteCut<'_>) -> RowsActivatedDependencyBatch {
        match self {
            Self::Exact(dependency) => dependency.activate_in_cut(cut),
            Self::ReadyPhase(dependency) => dependency.activate_in_cut(cut),
        }
    }
}

impl SchedulerApplyPermit<'_> {
    fn prestate_is_fresh(&self, frontier: &Arc<Mutex<FairFrontier>>) -> bool {
        match self {
            Self::Noop => true,
            Self::Reserved { reservation, delta } => reservation.prestate_is_fresh(frontier, delta),
            Self::Staged(staged) => staged.prestate_is_fresh(),
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
            Self::Staged(staged) => {
                let _published = staged.activate(token, owners);
            }
        }
    }
}

pub(super) fn commit_independent(
    plan: PreparedIndependentApply<'_>,
) -> Result<CommittedSharedApply, ConcurrentIndependentError> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_reserved_independent(
    plan: PreparedIndependentApply<'_>,
    reservation: super::super::scheduler::ReadyReservation,
) -> Result<CommittedSharedApply, ConcurrentIndependentError> {
    match plan {
        PreparedIndependentApply::Shared {
            authority,
            delta,
            support,
            staged_effect,
        } => PreparedIndependentApply::apply_shared(
            authority,
            &ApplyToken(()),
            delta,
            support,
            staged_effect,
            Some(super::super::scheduler::ReadyApplyReservation::Batch(
                reservation,
            )),
        ),
        #[cfg(test)]
        PreparedIndependentApply::Exclusive { .. } => Err(ConcurrentIndependentError::Fault(
            AuthorityFault::SchedulerProjection,
        )),
    }
}

pub(super) fn commit_ready_job_rows(
    authority: &TxPoolAuthority,
    delta: IndependentDelta,
    support: super::super::shard::ShardApplySupport,
    reservation: super::super::scheduler::ReadySlotReservation,
) -> Result<super::ReadyCommittedRows, ConcurrentIndependentError> {
    PreparedIndependentApply::apply_shared_rows(
        authority,
        &ApplyToken(()),
        delta,
        support,
        Some(super::super::scheduler::ReadyApplyReservation::Slot(
            reservation,
        )),
    )
}

pub(super) fn commit_reserved_ready_head_rows(
    authority: &TxPoolAuthority,
    delta: IndependentDelta,
    support: super::super::shard::ShardApplySupport,
    reservation: super::super::scheduler::ReadyReservation,
) -> Result<super::ReadyCommittedRows, ConcurrentIndependentError> {
    PreparedIndependentApply::apply_shared_rows(
        authority,
        &ApplyToken(()),
        delta,
        support,
        Some(super::super::scheduler::ReadyApplyReservation::Batch(
            reservation,
        )),
    )
}

pub(super) fn commit_unreserved_shared_rows(
    authority: &TxPoolAuthority,
    delta: IndependentDelta,
    support: super::super::shard::ShardApplySupport,
) -> Result<super::ReadyCommittedRows, ConcurrentIndependentError> {
    PreparedIndependentApply::apply_shared_rows(authority, &ApplyToken(()), delta, support, None)
}

pub(super) fn commit_shared_retained_ingress(
    plan: super::ingress::PreparedSharedRetainedAdmissionBatch<'_>,
) -> Result<
    super::ingress::CommittedRetainedAdmissionBatch,
    super::ingress::ConcurrentRetainedIngressError,
> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_shared_peer_revocation(
    plan: super::ingress::PreparedSharedPeerRevocation<'_>,
) -> Result<
    super::ingress::CommittedRetainedAdmissionBatch,
    super::ingress::ConcurrentPeerRevocationFailure,
> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_shared_peer_revocation_core(
    plan: super::ingress::PreparedSharedPeerRevocationCore<'_>,
) -> Result<super::CommittedSharedApply, super::ingress::ConcurrentPeerRevocationFailure> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_shared_remote_expiry(
    plan: PreparedSharedRemoteExpiry<'_>,
) -> Result<super::CommittedSharedApply, super::ingress::ConcurrentOwnerRemovalFailure> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_shared_accepted_expiry(
    plan: PreparedSharedAcceptedExpiry<'_>,
) -> Result<super::CommittedSharedApply, super::ingress::ConcurrentOwnerRemovalFailure> {
    plan.apply_with(&ApplyToken(()))
}

pub(super) fn commit_shared_local_removal(
    plan: PreparedSharedLocalRemoval<'_>,
) -> Result<super::CommittedSharedApply, super::ingress::ConcurrentOwnerRemovalFailure> {
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

enum PreparedResourceApply {
    Single(ResourcePlan),
    Batch(ResourceBatchPlan),
}

impl PreparedResourceApply {
    fn apply_shards(self, owners: &mut ShardedOwnerWriteCut<'_>) -> ResourceCapacityCommit {
        match self {
            Self::Single(plan) => plan.apply_shards(owners),
            Self::Batch(plan) => plan.apply_shards(owners),
        }
    }
}

pub(super) struct PreparedOwnerResourceDelta<I> {
    updates: I,
    resources: PreparedResourceApply,
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
            resources: PreparedResourceApply::Batch(resources),
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
        resources: ResourcePlan,
        support: ShardWriteSupport,
    ) -> Self {
        Self {
            updates: std::iter::once(update),
            resources: PreparedResourceApply::Single(resources),
            proposed_counts: Default::default(),
            support,
            owner_source_advance: None,
        }
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
        let mut projections = Vec::new();
        projections
            .try_reserve(changes.len())
            .map_err(|_| ResourceError::Allocation)?;
        for (key, before) in &changes {
            projections.push((
                key,
                ChargeProjection::from_validated(Some(*before))?,
                ChargeProjection::from_validated(None)?,
            ));
        }
        let shards = self.entries.plan_resource_transitions(projections)?;
        self.ledger
            .plan_removal_batch(self.entries, changes, shards)
    }

    pub(in crate::authority) fn plan_replace(
        &self,
        key: RawTxHash,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<ResourcePlan, ResourceError> {
        let before_projection = ChargeProjection::from_validated(expected)?;
        let after_projection = ChargeProjection::from_validated(after)?;
        let shards = self.entries.plan_resource_transitions(std::iter::once((
            &key,
            before_projection,
            after_projection,
        )))?;
        let entries = self.entries;
        self.ledger
            .plan_replace(entries, expected, after, shards, || {
                entries.get(&key).as_deref().map(OwnedTx::charge_record)
            })
    }

    pub(in crate::authority) fn plan_batch(
        &self,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let mut projections = Vec::new();
        projections
            .try_reserve(changes.len())
            .map_err(|_| ResourceError::Allocation)?;
        for (key, before, after) in &changes {
            projections.push((
                key,
                ChargeProjection::from_validated(*before)?,
                ChargeProjection::from_validated(*after)?,
            ));
        }
        let shards = self.entries.plan_resource_transitions(projections)?;
        let entries = self.entries;
        self.ledger.plan_batch(entries, changes, shards, |key| {
            entries.get(key).as_deref().map(OwnedTx::charge_record)
        })
    }

    pub(in crate::authority) fn plan_shared_transition_batch(
        &self,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        let mut projections = Vec::new();
        projections
            .try_reserve(changes.len())
            .map_err(|_| ResourceError::Allocation)?;
        for (key, before, after) in &changes {
            projections.push((
                key,
                ChargeProjection::from_validated(*before)?,
                ChargeProjection::from_validated(*after)?,
            ));
        }
        let shards = self.entries.plan_resource_transitions(projections)?;
        self.ledger
            .plan_shared_transition_batch(self.entries, changes, shards)
    }

    pub(in crate::authority) fn plan_direct_accepted_insertion_batch(
        &self,
        changes: Vec<(RawTxHash, ChargeRecord)>,
    ) -> Result<ResourceBatchPlan, DirectAcceptedInsertionError> {
        let mut projections = Vec::new();
        projections
            .try_reserve(changes.len())
            .map_err(|_| DirectAcceptedInsertionError::Resource(ResourceError::Allocation))?;
        for (key, after) in &changes {
            projections.push((
                key,
                ChargeProjection::from_validated(None)?,
                ChargeProjection::from_validated(Some(*after))?,
            ));
        }
        let shards = self.entries.plan_resource_transitions(projections)?;
        self.ledger
            .plan_direct_accepted_insertion_batch(self.entries, changes, shards)
    }
}

/// Physical-allocation capability for index planning.
pub(in crate::authority) struct IndexPlanner<'state> {
    indexes: &'state AuthorityIndexes,
}

impl IndexPlanner<'_> {
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
    #[cfg(test)]
    pub(in crate::authority) fn enter_concurrent_removal_plan_probe(&self) {
        self.state
            .owner_resources
            .entries
            .enter_concurrent_removal_plan_probe();
    }

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
        let dependencies =
            DependencyFrontier::for_entries(&entries, limits.max_dependency_stage_units());
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
        ready_phase_only: bool,
        mut sources: SourceVersionDelta,
        owner_source_counts: ShardOwnerSourceCounts,
        scheduler: SchedulerBatchDelta,
        reservation: Option<super::super::scheduler::ReadyApplyReservation>,
        retired: &mut RetiredOwners,
    ) -> Result<(DependencyFinalization, ResourceCommitHealth), ConcurrentIndependentError> {
        let entries = &self.state.owner_resources.entries;
        let (proposal_source_changed, transaction_source_changed) =
            sources.take_template_selection();
        if !sources.is_empty() {
            return Err(ConcurrentIndependentError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let dependency = if ready_phase_only {
            SharedIndependentDependencyStage::ReadyPhase(
                StagedDependencyBatch::stage_ready_phase(&self.state.dependencies, dependency)
                    .map_err(|error| match error {
                        DependencyStageError::Stale => ConcurrentIndependentError::ChangedCut(
                            SettlementChangedCut::owner_or_projection(),
                        ),
                        DependencyStageError::Capacity => ConcurrentIndependentError::ChangedCut(
                            SettlementChangedCut::dependency_stage_capacity(),
                        ),
                        DependencyStageError::Projection | DependencyStageError::Allocation => {
                            ConcurrentIndependentError::Fault(AuthorityFault::DependencyProjection)
                        }
                    })?,
            )
        } else {
            SharedIndependentDependencyStage::Exact(
                StagedDependencyBatch::stage_primary_replacements(
                    &self.state.dependencies,
                    dependency,
                )
                .map_err(|error| match error {
                    DependencyStageError::Stale => ConcurrentIndependentError::ChangedCut(
                        SettlementChangedCut::owner_or_projection(),
                    ),
                    DependencyStageError::Capacity => ConcurrentIndependentError::ChangedCut(
                        SettlementChangedCut::dependency_stage_capacity(),
                    ),
                    DependencyStageError::Projection | DependencyStageError::Allocation => {
                        ConcurrentIndependentError::Fault(AuthorityFault::DependencyProjection)
                    }
                })?,
            )
        };
        // Scheduler rows are physically staged before any owner shard is
        // locked. The staged capability later publishes only the actual-order
        // revision/cursor/visibility cut while owners are live, then releases
        // owners before scheduler B-tree cleanup. A per-slot Ready claim keeps
        // its existing lock-free CAS linearization.
        let scheduler_permit = match reservation {
            None if scheduler.is_empty() => SchedulerApplyPermit::Noop,
            Some(super::super::scheduler::ReadyApplyReservation::Slot(reservation)) => {
                SchedulerApplyPermit::Reserved {
                    reservation,
                    delta: scheduler,
                }
            }
            Some(super::super::scheduler::ReadyApplyReservation::Batch(reservation)) => {
                match super::super::scheduler::StagedSchedulerBatch::stage_reserved_ready_batch(
                    &self.state.scheduler,
                    scheduler,
                    reservation,
                ) {
                    Ok(staged) => SchedulerApplyPermit::Staged(staged),
                    Err(super::super::scheduler::SchedulerError::Stale) => {
                        return Err(ConcurrentIndependentError::ChangedCut(
                            SettlementChangedCut::scheduler(),
                        ));
                    }
                    Err(
                        super::super::scheduler::SchedulerError::Projection
                        | super::super::scheduler::SchedulerError::Arithmetic
                        | super::super::scheduler::SchedulerError::Allocation,
                    ) => {
                        return Err(ConcurrentIndependentError::Fault(
                            AuthorityFault::SchedulerProjection,
                        ));
                    }
                }
            }
            None => {
                match super::super::scheduler::StagedSchedulerBatch::stage_primary_replacements(
                    &self.state.scheduler,
                    scheduler,
                ) {
                    Ok(staged) => SchedulerApplyPermit::Staged(staged),
                    Err(super::super::scheduler::SchedulerError::Stale) => {
                        return Err(ConcurrentIndependentError::ChangedCut(
                            SettlementChangedCut::scheduler(),
                        ));
                    }
                    Err(
                        super::super::scheduler::SchedulerError::Projection
                        | super::super::scheduler::SchedulerError::Arithmetic
                        | super::super::scheduler::SchedulerError::Allocation,
                    ) => {
                        return Err(ConcurrentIndependentError::Fault(
                            AuthorityFault::SchedulerProjection,
                        ));
                    }
                }
            }
        };
        let mut reads = support.reads();
        let mut writes = support.writes();
        dependency.extend_final_support(&mut reads, &mut writes);
        let mut owners = entries.mixed_cut(reads, writes);
        let owners_fresh = owner_cuts
            .iter()
            .all(|owner| owner.expected.is_fresh(owners.owner(entries, &owner.key)));
        let proposed_fresh = owners.proposed_count_plan_is_fresh(&proposed_counts);
        let resources_fresh = resources
            .as_ref()
            .is_none_or(|resources| owners.resource_plan_is_fresh(resources.shard_plan()));
        let indexes_fresh = indexes.prestate_is_fresh(entries, &owners);
        let membership_fresh = membership.prestate_is_fresh_before_dependency_stage(
            entries,
            &owners,
            dependency.visibility(),
        );
        let dependency_fresh = dependency.prestate_is_fresh(&owners);
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
        if !scheduler_permit.prestate_is_fresh(&self.state.scheduler) {
            drop(owners);
            return Err(ConcurrentIndependentError::ChangedCut(
                SettlementChangedCut::scheduler(),
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
                let capacity = match capacity.begin() {
                    Ok(capacity) => capacity,
                    Err(ResourceCapacityBeginError::StaleActiveWorkRevision) => {
                        drop(owners);
                        drop(scheduler_permit);
                        return Err(ConcurrentIndependentError::ChangedCut(
                            SettlementChangedCut::resource_capacity(),
                        ));
                    }
                    Err(ResourceCapacityBeginError::Capacity(_)) => {
                        drop(owners);
                        drop(scheduler_permit);
                        return Err(ConcurrentIndependentError::Fault(
                            AuthorityFault::ResourceProjection,
                        ));
                    }
                };
                (Some(resource_shards), Some(capacity))
            }
            None => (None, None),
        };
        #[cfg(test)]
        entries.enter_concurrent_removal_probe();
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
        let dependency = dependency.activate_in_cut(&mut owners).publish_owned();
        owners.apply_owner_source_advance(source_advance);
        #[cfg(test)]
        entries.enter_shared_owner_commit_probe();
        scheduler_permit.apply(&self.state.scheduler, token, owners);
        let dependency = dependency.finalize();
        let resource_health =
            capacity.map_or(ResourceCommitHealth::Healthy, |capacity| capacity.finish());
        let _ = token;
        Ok((dependency, resource_health))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the arguments are the one canonical retained-ingress delta split into its existing physical authorities; a wrapper would duplicate that semantic carrier"
    )]
    pub(super) fn commit_shared_retained_ingress_rows(
        &self,
        token: &ApplyToken,
        updates: Vec<super::ingress::RetainedIngressUpdate>,
        resources: ResourceBatchPlan,
        mut indexes: IndexDelta,
        owner_source_counts: ShardOwnerSourceCounts,
        staged: super::ingress::StagedRetainedAdmissionIngress<'_>,
        clocks: AuthorityClocks,
        mut retired: RetiredOwners,
    ) -> Result<
        (DependencyFinalization, ResourceCommitHealth, RetiredOwners),
        super::ingress::ConcurrentRetainedIngressError,
    > {
        if updates.iter().any(|update| {
            update.vacancy_revision.is_none()
                || match &update.before {
                    None => false,
                    Some(before) => {
                        let after = &update.after;
                        before.record().version == after.record().version
                    }
                }
        }) {
            return Err(super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::MembershipProjection,
            ));
        }
        let entries = &self.state.owner_resources.entries;
        let proposed_counts = super::super::shard::ShardProposedCountPlan::default();
        let mut support = entries.owner_resource_write_support(
            updates.iter().map(|update| &update.key),
            &proposed_counts,
            resources.shard_plan(),
        );
        support.include(indexes.sharded_write_support(entries));
        let mut reads = ShardReadSupport::default();
        staged.extend_final_support(&mut reads, &mut support);
        let mut owners = entries.mixed_cut(reads, support);
        if !updates.iter().all(|update| {
            let shard = entries.owner_shard(&update.key);
            let Some(expected_revision) = update.vacancy_revision else {
                return false;
            };
            if owners.owner_removal_revision(shard) != expected_revision {
                return false;
            }
            match &update.before {
                None => owners.owner_version(shard, &update.key).is_none(),
                Some(before) => {
                    owners.owner_version(shard, &update.key) == Some(before.record().version)
                }
            }
        }) || !owners.resource_plan_is_fresh(resources.shard_plan())
            || !indexes.prestate_is_fresh(entries, &owners)
            || !staged.prestate_is_fresh(&owners)
        {
            return Err(super::ingress::ConcurrentRetainedIngressError::Stale);
        }
        let source_advance = owners
            .prepare_owner_source_advance(owner_source_counts)
            .ok_or(super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::CounterExhausted,
            ))?;
        let (resource_shards, capacity) = resources.into_shared_commit_parts();
        let capacity = match capacity.begin() {
            Ok(capacity) => capacity,
            Err(ResourceCapacityBeginError::StaleActiveWorkRevision) => {
                return Err(super::ingress::ConcurrentRetainedIngressError::Stale);
            }
            Err(ResourceCapacityBeginError::Capacity(_)) => {
                return Err(super::ingress::ConcurrentRetainedIngressError::Fault(
                    AuthorityFault::ResourceProjection,
                ));
            }
        };
        #[cfg(test)]
        entries.enter_concurrent_removal_probe();
        for update in updates {
            let key = update.key;
            let shard = entries.owner_shard(&key);
            let previous = owners.replace(shard, key, Some(update.after));
            // The exact owner version and removal revision were checked while
            // this same physical cut was held, so replacement cardinality
            // cannot change between validation and mutation.
            debug_assert_eq!(previous.is_some(), update.before.is_some());
            if let Some(previous) = previous {
                retired.push(previous);
            }
        }
        owners.apply_proposed_counts(proposed_counts);
        owners.apply_resource_plan(resource_shards);
        indexes.apply_sharded(entries, &mut owners);
        owners.apply_owner_source_advance(source_advance);
        #[cfg(test)]
        entries.enter_shared_ingress_probe(
            super::super::shard::SharedIngressProbePhase::FinalCutBeforeActivation,
        );
        let dependency = staged.activate(token, owners);
        let resource_health = capacity.finish();
        let _reserved_clock_high_water = clocks;
        Ok((dependency, resource_health, retired))
    }

    pub(super) fn commit_shared_owner_removal_rows<C>(
        &self,
        token: &ApplyToken,
        removal: OwnerRemovalBatch,
        staged: super::ingress::StagedRetainedIngress<'_>,
        control: C,
        clocks: AuthorityClocks,
    ) -> Result<
        (DependencyFinalization, ResourceCommitHealth, RetiredOwners),
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
        staged.extend_final_support(&mut reads, &mut support);
        control.extend_final_support(entries, &mut reads, &mut support);
        // Canonical ascending acquisition is sparse-write/full-read for Remote
        // expiry and sparse-write for peer revocation. The fixed 64-shard walk
        // is the only global ordering cost; no global writer is acquired.
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
        let membership_fresh = membership.prestate_is_fresh_before_dependency_stage(
            entries,
            &cut,
            staged.dependency_visibility(),
        );
        let staged_fresh = staged.prestate_is_fresh(&cut);
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
            | ResourceError::CapacityBankFault
            | ResourceError::Allocation => super::ingress::ConcurrentRetainedIngressError::Fault(
                AuthorityFault::ResourceProjection,
            ),
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
        #[cfg(test)]
        entries.enter_shared_owner_commit_probe();
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
        let dependency = staged.activate(token, cut);
        let resource_health = capacity.finish();
        let _reserved_clock_high_water = clocks;
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
    ) -> Result<(), PlanError> {
        self.state
            .owner_resources
            .entries
            .try_reserve_keys(keys)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))
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

    #[cfg(test)]
    pub(super) fn peer_bans_for_plan(&self) -> PeerBanPlanner<'_> {
        PeerBanPlanner {
            peer_bans: &self.state.peer_bans,
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

    pub(super) fn entries_and_indexes_for_plan(&self) -> (&ShardedOwnerMap, IndexPlanner<'_>) {
        (
            &self.state.owner_resources.entries,
            IndexPlanner {
                indexes: &self.state.indexes,
            },
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

    fn into_fresh_generation(
        self,
        dependency_publication: super::FreshDependencyPublication,
    ) -> FreshGeneration {
        FreshGeneration {
            entries: self.state.owner_resources.entries,
            resources: self.state.owner_resources.resources,
            scheduler: self.state.scheduler,
            dependencies: self.state.dependencies,
            dependency_publication,
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
    pub(in crate::authority) fn indexes_for_test_plan(&self) -> IndexPlanner<'_> {
        IndexPlanner {
            indexes: &self.state.indexes,
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
    pub(in crate::authority) fn replace_next_version_for_test(
        &mut self,
        _test: &super::test_support::AuthorityTestToken,
        version: EntryVersion,
    ) {
        let mut clocks = self.state.clocks.snapshot();
        clocks.next_version = version;
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
            dependency_publication: super::FreshDependencyPublication::default(),
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
        self.dependency_publication
            .absorb(committed.into_scratch_dependency_finalization())
    }

    pub(super) fn clocks(&self) -> AuthorityClocks {
        self.authority.state.clocks.snapshot()
    }

    pub(super) fn into_fresh_generation(self) -> FreshGeneration {
        self.authority
            .into_fresh_generation(self.dependency_publication)
    }
}

pub(super) fn commit(plan: PreparedApply<'_>) -> CommittedDelta {
    let token = ApplyToken(());
    plan.apply_with(&token)
}
