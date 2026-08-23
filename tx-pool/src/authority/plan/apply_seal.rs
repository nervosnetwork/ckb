use super::super::resources::ResourceCapacityCommit;
use super::super::shard::{
    AuthorityShardRouter, ShardWriteSupport, ShardedOwnerMap, ShardedOwnerWriteCut,
};
use super::*;
use std::ops::Deref;

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
    pub(super) scheduler: FairFrontier,
    pub(super) dependencies: DependencyFrontier,
    pub(super) effects: EffectLog,
    pub(super) peer_bans: PeerBanRegistry,
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
pub(super) struct ApplyToken(());

#[derive(Clone, Copy)]
pub(super) enum PreviousOwnerDisposition {
    Retire,
    Drop,
}

pub(super) struct OwnerResourceUpdate {
    key: RawTxHash,
    after: Option<OwnedTx>,
    previous: PreviousOwnerDisposition,
}

impl OwnerResourceUpdate {
    pub(super) fn new(
        key: RawTxHash,
        after: Option<OwnedTx>,
        previous: PreviousOwnerDisposition,
    ) -> Self {
        Self {
            key,
            after,
            previous,
        }
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
    status_counts: super::super::shard::ShardStatusCountPlan,
    support: ShardWriteSupport,
}

impl<I> PreparedOwnerResourceDelta<I> {
    pub(super) fn batch(
        updates: I,
        resources: ResourceBatchPlan,
        status_counts: super::super::shard::ShardStatusCountPlan,
        support: ShardWriteSupport,
    ) -> Self {
        Self {
            updates,
            resources: PreparedResourceApply::Batch(resources),
            status_counts,
            support,
        }
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
            status_counts: Default::default(),
            support,
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
    ledger: &'state mut ResourceLedger,
}

impl ResourcePlanner<'_> {
    pub(in crate::authority) fn plan_replace(
        &mut self,
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
        &mut self,
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

    pub(in crate::authority) fn plan_compute_release(
        &self,
        key: RawTxHash,
        expected: ChargeRecord,
        after: ChargeRecord,
    ) -> Result<ResourcePlan, ComputeReleaseError> {
        let before_projection = ChargeProjection::from_validated(Some(expected))
            .map_err(|_| ComputeReleaseError::Projection)?;
        let after_projection = ChargeProjection::from_validated(Some(after))
            .map_err(|_| ComputeReleaseError::Projection)?;
        let shards = self
            .entries
            .plan_resource_transitions(std::iter::once((&key, before_projection, after_projection)))
            .map_err(|error| match error {
                ResourceError::Arithmetic => ComputeReleaseError::Arithmetic,
                ResourceError::PreAcceptedLimit
                | ResourceError::RemoteLimit
                | ResourceError::PeerLimit(_)
                | ResourceError::ReplacementHistoryLimit
                | ResourceError::AcceptedLimit
                | ResourceError::ExistingChargeMismatch
                | ResourceError::DuplicateChange
                | ResourceError::ComputeEnvelope
                | ResourceError::AttributionMismatch
                | ResourceError::CapacityBankFault
                | ResourceError::Allocation => ComputeReleaseError::Projection,
            })?;
        let entries = self.entries;
        self.ledger
            .plan_compute_release(entries, expected, after, shards, || {
                entries.get(&key).as_deref().map(OwnedTx::charge_record)
            })
    }
}

/// Physical-allocation capability for index planning.
pub(in crate::authority) struct IndexPlanner<'state> {
    indexes: &'state mut AuthorityIndexes,
}

impl IndexPlanner<'_> {
    pub(in crate::authority) fn plan_replace(
        &mut self,
        key: &RawTxHash,
        before: Option<&OwnedTx>,
        after: Option<&OwnedTx>,
    ) -> Result<IndexDelta, IndexError> {
        self.indexes.plan_replace(key, before, after)
    }

    pub(in crate::authority) fn plan_stable_replace(
        &mut self,
        key: &RawTxHash,
        before: &OwnedTx,
        after: &OwnedTx,
    ) -> Result<IndexDelta, StableIndexError> {
        self.indexes.plan_stable_replace(key, before, after)
    }

    pub(in crate::authority) fn plan_replacements<'entry>(
        &mut self,
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
    effects: &'state mut EffectLog,
}

impl EffectPlanner<'_> {
    pub(in crate::authority) fn plan_publication(
        &mut self,
        publication: &EffectPublication,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.effects.plan_publication(publication, sequence)
    }

    pub(in crate::authority) fn plan_chain_rebuildable(
        &mut self,
        effects: Vec<CommittedEffect>,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.effects.plan_chain_rebuildable(effects, sequence)
    }
}

/// Physical-allocation capability for peer-ban planning.
pub(in crate::authority) struct PeerBanPlanner<'state> {
    peer_bans: &'state mut PeerBanRegistry,
}

impl PeerBanPlanner<'_> {
    pub(in crate::authority) fn plan_record(
        &mut self,
        peer: ckb_network::PeerIndex,
        observed_at: Instant,
    ) -> Result<PeerBanDelta, PeerBanError> {
        self.peer_bans.plan_record(peer, observed_at)
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
        Self {
            state: AuthorityState {
                generation: PoolGeneration(0),
                chain_view,
                owner_resources: OwnerResourceAuthority {
                    entries: ShardedOwnerMap::new(router),
                    resources: ResourceLedger::new(limits),
                },
                indexes: AuthorityIndexes::default(),
                source_versions: AuthoritySourceVersions::initial(),
                membership: MembershipProjection::default(),
                scheduler: FairFrontier::new(verify_order),
                dependencies: DependencyFrontier::default(),
                effects,
                peer_bans: PeerBanRegistry::default(),
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
        &mut self,
        token: &ApplyToken,
        delta: PreparedOwnerResourceDelta<I>,
        retired: &mut Vec<OwnedTx>,
    ) where
        I: IntoIterator<Item = OwnerResourceUpdate>,
    {
        let owner_resources = &self.state.owner_resources;
        let mut owners = owner_resources.entries.write_cut(delta.support);
        for update in delta.updates {
            let shard = owner_resources.entries.owner_shard(&update.key);
            let previous = owners.replace(shard, update.key, update.after);
            match (update.previous, previous) {
                (PreviousOwnerDisposition::Retire, Some(owner)) => retired.push(owner),
                (PreviousOwnerDisposition::Retire, None) => {}
                (PreviousOwnerDisposition::Drop, previous) => drop(previous),
            }
        }
        owners.apply_status_counts(delta.status_counts);
        let capacity = delta.resources.apply_shards(&mut owners);
        drop(owners);
        capacity.commit();
        let _ = token;
    }

    pub(super) fn replace_owner_resources(
        &mut self,
        token: &ApplyToken,
        entries: ShardedOwnerMap,
        resources: ResourceLedger,
    ) -> (ShardedOwnerMap, ResourceLedger) {
        let owner_resources = &mut self.state.owner_resources;
        let previous_entries = std::mem::replace(&mut owner_resources.entries, entries);
        let previous_resources = std::mem::replace(&mut owner_resources.resources, resources);
        let _ = token;
        (previous_entries, previous_resources)
    }

    pub(super) fn reserve_primary_owner_insertions<'key>(
        &mut self,
        keys: impl IntoIterator<Item = &'key RawTxHash>,
    ) -> Result<(), PlanError> {
        self.state
            .owner_resources
            .entries
            .try_reserve_keys(keys)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))
    }

    pub(super) fn resources_for_plan(&mut self) -> ResourcePlanner<'_> {
        ResourcePlanner {
            entries: &self.state.owner_resources.entries,
            ledger: &mut self.state.owner_resources.resources,
        }
    }

    pub(super) fn indexes_for_plan(&mut self) -> IndexPlanner<'_> {
        IndexPlanner {
            indexes: &mut self.state.indexes,
        }
    }

    pub(super) fn effects_for_plan(&mut self) -> EffectPlanner<'_> {
        EffectPlanner {
            effects: &mut self.state.effects,
        }
    }

    pub(super) fn peer_bans_for_plan(&mut self) -> PeerBanPlanner<'_> {
        PeerBanPlanner {
            peer_bans: &mut self.state.peer_bans,
        }
    }

    pub(super) fn reserve_membership_owner_insertions(
        &mut self,
        input_insertions: usize,
        owner_insertions: usize,
    ) -> Result<(), PlanError> {
        self.state
            .membership
            .reserve_owner_insertion_capacity(input_insertions, owner_insertions)
    }

    pub(super) fn reserve_membership_dependency_rows(
        &mut self,
        additional: usize,
    ) -> Result<(), PlanError> {
        self.state
            .membership
            .reserve_dependency_reader_rows(additional)
    }

    pub(super) fn reserve_membership_dependency_row(
        &mut self,
        dependency: &OutPoint,
        additional: usize,
    ) -> Result<(), PlanError> {
        self.state
            .membership
            .reserve_dependency_reader_row(dependency, additional)
    }

    pub(super) fn reserve_membership_child_row(
        &mut self,
        parent: &RawTxHash,
        additional: usize,
    ) -> Result<(), PlanError> {
        self.state.membership.reserve_child_row(parent, additional)
    }

    pub(super) fn reserve_membership_parent_row(
        &mut self,
        child: &RawTxHash,
        additional: usize,
    ) -> Result<(), PlanError> {
        self.state.membership.reserve_parent_row(child, additional)
    }

    pub(super) fn owner_derivation_parts(
        &mut self,
    ) -> (&ShardedOwnerMap, IndexPlanner<'_>, &AuthoritySourceVersions) {
        (
            &self.state.owner_resources.entries,
            IndexPlanner {
                indexes: &mut self.state.indexes,
            },
            &self.state.source_versions,
        )
    }

    pub(super) fn entries_and_indexes_for_plan(&mut self) -> (&ShardedOwnerMap, IndexPlanner<'_>) {
        (
            &self.state.owner_resources.entries,
            IndexPlanner {
                indexes: &mut self.state.indexes,
            },
        )
    }

    pub(super) fn owner_removal_plan_parts(
        &mut self,
    ) -> (
        &ShardedOwnerMap,
        ResourcePlanner<'_>,
        &FairFrontier,
        &DependencyFrontier,
        &AuthoritySourceVersions,
        IndexPlanner<'_>,
    ) {
        (
            &self.state.owner_resources.entries,
            ResourcePlanner {
                entries: &self.state.owner_resources.entries,
                ledger: &mut self.state.owner_resources.resources,
            },
            &self.state.scheduler,
            &self.state.dependencies,
            &self.state.source_versions,
            IndexPlanner {
                indexes: &mut self.state.indexes,
            },
        )
    }

    fn into_fresh_generation(self) -> FreshGeneration {
        FreshGeneration {
            entries: self.state.owner_resources.entries,
            indexes: self.state.indexes,
            resources: self.state.owner_resources.resources,
            membership: self.state.membership,
            scheduler: self.state.scheduler,
            dependencies: self.state.dependencies,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn resources_for_test_plan(&mut self) -> ResourcePlanner<'_> {
        ResourcePlanner {
            entries: &self.state.owner_resources.entries,
            ledger: &mut self.state.owner_resources.resources,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn indexes_for_test_plan(&mut self) -> IndexPlanner<'_> {
        IndexPlanner {
            indexes: &mut self.state.indexes,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn effects_for_test_plan(&mut self) -> EffectPlanner<'_> {
        EffectPlanner {
            effects: &mut self.state.effects,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn peer_bans_for_test_plan(&mut self) -> PeerBanPlanner<'_> {
        PeerBanPlanner {
            peer_bans: &mut self.state.peer_bans,
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
        self.state.peer_bans = PeerBanRegistry::with_limit_for_test(capacity);
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
        Self { authority }
    }

    pub(super) fn resources(&self) -> &ResourceLedger {
        &self.authority.state.resources
    }

    pub(super) fn apply_charged_admission(
        &mut self,
        admission: ChargedAdmission,
    ) -> Result<(), PlanError> {
        drop(self.authority.plan_charged_admission(admission)?.apply());
        Ok(())
    }

    pub(super) fn clocks(&self) -> AuthorityClocks {
        self.authority.state.clocks.snapshot()
    }

    pub(super) fn into_fresh_generation(self) -> FreshGeneration {
        self.authority.into_fresh_generation()
    }
}

pub(super) fn commit(plan: PreparedApply<'_>) -> CommittedDelta {
    let token = ApplyToken(());
    plan.apply_with(&token)
}
