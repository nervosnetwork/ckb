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
    pub(super) entries: HashMap<RawTxHash, OwnedTx>,
    pub(super) indexes: AuthorityIndexes,
    pub(super) source_versions: AuthoritySourceVersions,
    pub(super) resources: ResourceLedger,
    pub(super) membership: MembershipProjection,
    pub(super) scheduler: FairFrontier,
    pub(super) dependencies: DependencyFrontier,
    pub(super) effects: EffectLog,
    pub(super) peer_bans: PeerBanRegistry,
    pub(super) membership_config: MembershipConfig,
    pub(super) clocks: AuthorityClocks,
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

impl Deref for TxPoolAuthority {
    type Target = AuthorityState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

/// Capability whose constructor is private to this sealing module.
pub(super) struct ApplyToken(());

/// Physical-allocation capability for resource planning.
///
/// This deliberately does not implement `DerefMut`: callers can compile
/// resource deltas, but cannot replace or otherwise mutate the authoritative
/// ledger directly.
pub(in crate::authority) struct ResourcePlanner<'state> {
    ledger: &'state mut ResourceLedger,
}

impl ResourcePlanner<'_> {
    pub(in crate::authority) fn plan_replace(
        &mut self,
        key: RawTxHash,
        expected: Option<ChargeRecord>,
        after: Option<ChargeRecord>,
    ) -> Result<ResourcePlan, ResourceError> {
        self.ledger.plan_replace(key, expected, after)
    }

    pub(in crate::authority) fn plan_batch(
        &mut self,
        changes: Vec<(RawTxHash, Option<ChargeRecord>, Option<ChargeRecord>)>,
    ) -> Result<ResourceBatchPlan, ResourceError> {
        self.ledger.plan_batch(changes)
    }

    pub(in crate::authority) fn plan_compute_release(
        &self,
        key: RawTxHash,
        expected: ChargeRecord,
        after: ChargeRecord,
    ) -> Result<ResourcePlan, ComputeReleaseError> {
        self.ledger.plan_compute_release(key, expected, after)
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
        ))
    }

    fn assemble(
        limits: ResourceLimits,
        verify_order: VerifyOrder,
        effects: EffectLog,
        membership_config: MembershipConfig,
        chain_view: ChainViewId,
    ) -> Self {
        Self {
            state: AuthorityState {
                generation: PoolGeneration(0),
                chain_view,
                entries: HashMap::new(),
                indexes: AuthorityIndexes::default(),
                source_versions: AuthoritySourceVersions::initial(),
                resources: ResourceLedger::new(limits),
                membership: MembershipProjection::default(),
                scheduler: FairFrontier::new(verify_order),
                dependencies: DependencyFrontier::default(),
                effects,
                peer_bans: PeerBanRegistry::default(),
                membership_config,
                clocks: AuthorityClocks::first(),
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
        Self::assemble(limits, verify_order, effects, membership_config, chain_view)
    }

    pub(super) fn write<'authority>(
        &'authority mut self,
        _token: &ApplyToken,
    ) -> &'authority mut AuthorityState {
        &mut self.state
    }

    pub(super) fn reserve_primary_owner_insertions(
        &mut self,
        additional: usize,
    ) -> Result<(), PlanError> {
        if additional == 0 {
            return Ok(());
        }
        self.state
            .entries
            .try_reserve(additional)
            .map_err(|_| PlanError::Backpressure(Backpressure::Allocation))
    }

    pub(super) fn resources_for_plan(&mut self) -> ResourcePlanner<'_> {
        ResourcePlanner {
            ledger: &mut self.state.resources,
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
    ) -> (
        &HashMap<RawTxHash, OwnedTx>,
        IndexPlanner<'_>,
        AuthoritySourceVersions,
    ) {
        (
            &self.state.entries,
            IndexPlanner {
                indexes: &mut self.state.indexes,
            },
            self.state.source_versions,
        )
    }

    pub(super) fn entries_and_indexes_for_plan(
        &mut self,
    ) -> (&HashMap<RawTxHash, OwnedTx>, IndexPlanner<'_>) {
        (
            &self.state.entries,
            IndexPlanner {
                indexes: &mut self.state.indexes,
            },
        )
    }

    pub(super) fn owner_removal_plan_parts(
        &mut self,
    ) -> (
        &HashMap<RawTxHash, OwnedTx>,
        ResourcePlanner<'_>,
        &FairFrontier,
        &DependencyFrontier,
        &AuthoritySourceVersions,
        IndexPlanner<'_>,
    ) {
        (
            &self.state.entries,
            ResourcePlanner {
                ledger: &mut self.state.resources,
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
            entries: self.state.entries,
            indexes: self.state.indexes,
            resources: self.state.resources,
            membership: self.state.membership,
            scheduler: self.state.scheduler,
            dependencies: self.state.dependencies,
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn resources_for_test_plan(&mut self) -> ResourcePlanner<'_> {
        ResourcePlanner {
            ledger: &mut self.state.resources,
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
        self.state.clocks.next_sequence = sequence;
    }

    #[cfg(test)]
    pub(in crate::authority) fn replace_next_version_for_test(
        &mut self,
        _test: &super::test_support::AuthorityTestToken,
        version: EntryVersion,
    ) {
        self.state.clocks.next_version = version;
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
        chain_view: ChainViewId,
        generation: PoolGeneration,
        clocks: AuthorityClocks,
    ) -> Self {
        let mut authority =
            TxPoolAuthority::assemble(limits, verify_order, effects, membership_config, chain_view);
        authority.state.generation = generation;
        authority.state.clocks = clocks;
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
        self.authority.state.clocks
    }

    pub(super) fn into_fresh_generation(self) -> FreshGeneration {
        self.authority.into_fresh_generation()
    }
}

pub(super) fn commit(plan: PreparedApply<'_>) -> CommittedDelta {
    let token = ApplyToken(());
    plan.apply_with(&token)
}
