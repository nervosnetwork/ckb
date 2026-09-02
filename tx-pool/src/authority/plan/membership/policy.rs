use super::{
    AcceptedOrderKey, AncestorAggregate, CapturedAccepted, DescendantAggregate, EvictionOrderKey,
    MembershipConfig, MembershipPolicyWitness, MembershipReject, OwnerReadFact,
};
use crate::authority::{
    plan::{AuthorityFault, PlanError, StalePlan, TxPoolAuthority},
    resources::AcceptedResources,
    shard::ShardedAcceptedReadGuard,
    state::{AcceptedEntry, DependencyKey, OwnedTx, RawTxHash},
};
use ckb_types::packed::OutPoint;
use std::{
    collections::{BTreeSet, HashSet},
    ops::Deref,
};

mod sealed {
    pub(in crate::authority::plan::membership) trait Sealed {}
}

pub(super) trait PolicyMode: sealed::Sealed {
    type Accepted<'authority>: Deref<Target = AcceptedEntry>;

    fn dispatch<Input, T, Direct, Exact>(
        &mut self,
        input: Input,
        direct: Direct,
        exact: Exact,
    ) -> T
    where
        Direct: FnOnce(Input) -> T,
        Exact: FnOnce(&mut MembershipPolicyWitness, Input) -> T;

    fn observe_accepted_owner<'authority>(
        &mut self,
        authority: &'authority TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Self::Accepted<'authority>, PlanError>;

    fn finish(self, authority: &TxPoolAuthority) -> Result<MembershipPolicyWitness, PlanError>;
}

pub(super) struct ExclusiveMode;

pub(super) struct OptimisticMode {
    witness: MembershipPolicyWitness,
}

impl sealed::Sealed for ExclusiveMode {}
impl sealed::Sealed for OptimisticMode {}

impl PolicyMode for ExclusiveMode {
    type Accepted<'authority> = ShardedAcceptedReadGuard<'authority>;

    #[inline(always)]
    fn dispatch<Input, T, Direct, Exact>(
        &mut self,
        input: Input,
        direct: Direct,
        _exact: Exact,
    ) -> T
    where
        Direct: FnOnce(Input) -> T,
        Exact: FnOnce(&mut MembershipPolicyWitness, Input) -> T,
    {
        direct(input)
    }

    fn observe_accepted_owner<'authority>(
        &mut self,
        authority: &'authority TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Self::Accepted<'authority>, PlanError> {
        authority
            .entries
            .get(hash)
            .and_then(|owner| owner.into_accepted().ok())
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))
    }

    fn finish(self, _authority: &TxPoolAuthority) -> Result<MembershipPolicyWitness, PlanError> {
        Ok(MembershipPolicyWitness::default())
    }
}

impl PolicyMode for OptimisticMode {
    type Accepted<'authority> = CapturedAccepted;

    #[inline(always)]
    fn dispatch<Input, T, Direct, Exact>(
        &mut self,
        input: Input,
        _direct: Direct,
        exact: Exact,
    ) -> T
    where
        Direct: FnOnce(Input) -> T,
        Exact: FnOnce(&mut MembershipPolicyWitness, Input) -> T,
    {
        exact(&mut self.witness, input)
    }

    fn observe_accepted_owner<'authority>(
        &mut self,
        authority: &'authority TxPoolAuthority,
        hash: &RawTxHash,
    ) -> Result<Self::Accepted<'authority>, PlanError> {
        self.witness.observe_accepted_owner(authority, hash)
    }

    fn finish(self, authority: &TxPoolAuthority) -> Result<MembershipPolicyWitness, PlanError> {
        self.witness
            .prove_coherent(authority)
            .then_some(self.witness)
            .ok_or(PlanError::Stale(StalePlan::AcceptedObservation))
    }
}

pub(super) struct PolicyContext<'authority, Mode> {
    authority: &'authority TxPoolAuthority,
    mode: Mode,
    exact_projected: Option<AcceptedResources>,
}

impl<'authority> PolicyContext<'authority, ExclusiveMode> {
    pub(super) fn exclusive(authority: &'authority TxPoolAuthority) -> Self {
        Self {
            authority,
            mode: ExclusiveMode,
            exact_projected: None,
        }
    }
}

impl<'authority> PolicyContext<'authority, OptimisticMode> {
    pub(super) fn optimistic(
        authority: &'authority TxPoolAuthority,
        dependency_consumer_bound: usize,
    ) -> Self {
        Self {
            authority,
            mode: OptimisticMode {
                witness: MembershipPolicyWitness::bounded_for_shared(dependency_consumer_bound),
            },
            exact_projected: None,
        }
    }

    pub(super) fn optimistic_with_witness(
        authority: &'authority TxPoolAuthority,
        witness: MembershipPolicyWitness,
    ) -> Self {
        Self {
            authority,
            mode: OptimisticMode { witness },
            exact_projected: None,
        }
    }
}

impl<'authority, Mode> PolicyContext<'authority, Mode>
where
    Mode: PolicyMode,
{
    pub(super) fn config(&self) -> MembershipConfig {
        self.authority.membership_config
    }

    fn dispatch<T>(
        &mut self,
        direct: impl FnOnce() -> T,
        exact: impl FnOnce(&mut MembershipPolicyWitness) -> T,
    ) -> T {
        self.mode
            .dispatch((), |()| direct(), |witness, ()| exact(witness))
    }

    pub(super) fn observe_owner(
        &mut self,
        hash: &RawTxHash,
    ) -> Result<Option<OwnerReadFact>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || {
                Ok(authority
                    .entries
                    .get(hash)
                    .as_deref()
                    .map(MembershipPolicyWitness::owner_fact))
            },
            |witness| witness.observe_owner(authority, hash),
        )
    }

    pub(super) fn observe_accepted_owner(
        &mut self,
        hash: &RawTxHash,
    ) -> Result<Mode::Accepted<'authority>, PlanError> {
        self.mode.observe_accepted_owner(self.authority, hash)
    }

    pub(super) fn observe_spender(
        &mut self,
        out_point: &OutPoint,
    ) -> Result<Option<RawTxHash>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.spender(out_point)),
            |witness| witness.observe_spender(authority, out_point),
        )
    }

    pub(super) fn observe_dependency_consumers(
        &mut self,
        key: DependencyKey,
    ) -> Result<Option<BTreeSet<RawTxHash>>, PlanError> {
        let authority = self.authority;
        self.mode.dispatch(
            key,
            |key| {
                authority
                    .dependencies
                    .consumers_for(&key)
                    .map_err(PlanError::from)
            },
            |witness, key| witness.observe_dependency_consumers(authority, key),
        )
    }

    pub(super) fn observe_dependency_owner(
        &mut self,
        hash: &RawTxHash,
        key: &DependencyKey,
    ) -> Result<(bool, bool), PlanError> {
        let authority = self.authority;
        self.dispatch(
            || {
                let owner = authority
                    .entries
                    .get(hash)
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                Ok((
                    owner.dependencies().contains(key),
                    matches!(&*owner, OwnedTx::Accepted(_)),
                ))
            },
            |witness| {
                let owner = witness
                    .capture_owner_value(authority, hash)?
                    .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
                Ok((
                    owner.dependencies().contains(key),
                    matches!(owner, OwnedTx::Accepted(_)),
                ))
            },
        )
    }

    pub(super) fn observe_parents(
        &mut self,
        hash: &RawTxHash,
    ) -> Result<Option<HashSet<RawTxHash>>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.parents(hash)),
            |witness| witness.observe_parents(authority, hash),
        )
    }

    pub(super) fn observe_children(
        &mut self,
        hash: &RawTxHash,
    ) -> Result<Option<HashSet<RawTxHash>>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || authority.membership.accepted_child_row(hash).map(Some),
            |witness| witness.observe_children(authority, hash),
        )
    }

    pub(super) fn observe_ancestor(
        &mut self,
        hash: &RawTxHash,
    ) -> Result<Option<AncestorAggregate>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.ancestor_aggregate(hash)),
            |witness| witness.observe_ancestor(authority, hash),
        )
    }

    pub(super) fn observe_descendant(
        &mut self,
        hash: &RawTxHash,
    ) -> Result<Option<DescendantAggregate>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.descendant_aggregate(hash)),
            |witness| witness.observe_descendant(authority, hash),
        )
    }

    pub(super) fn observe_accepted_order(
        &mut self,
        key: &AcceptedOrderKey,
    ) -> Result<bool, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.contains_accepted_order(key)),
            |witness| witness.observe_accepted_order(authority, key),
        )
    }

    pub(super) fn observe_eviction_order(
        &mut self,
        key: &EvictionOrderKey,
    ) -> Result<bool, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.contains_eviction_order(key)),
            |witness| witness.observe_eviction_order(authority, key),
        )
    }

    pub(super) fn eviction_order(&mut self) -> Result<Vec<EvictionOrderKey>, PlanError> {
        let authority = self.authority;
        self.dispatch(
            || Ok(authority.membership.eviction_order()),
            |witness| witness.observe_capacity_eviction_order(authority),
        )
    }

    pub(super) fn initial_accepted_fits(
        &mut self,
        released: AcceptedResources,
        added: AcceptedResources,
    ) -> Result<bool, PlanError> {
        let authority = self.authority;
        let exact_projected = &mut self.exact_projected;
        self.mode.dispatch(
            (),
            |()| {
                let projected = exact_projection(authority, exact_projected, released, added)?;
                Ok(accepted_fits(authority, projected))
            },
            |_witness, ()| {
                authority
                    .resources_for_plan()
                    .membership_accepted_transition_fits(released, added)
                    .map_err(|_| PlanError::Fault(AuthorityFault::ResourceProjection))
            },
        )
    }

    pub(super) fn exact_accepted_projection(
        &mut self,
        released: AcceptedResources,
        added: AcceptedResources,
    ) -> Result<AcceptedResources, PlanError> {
        exact_projection(self.authority, &mut self.exact_projected, released, added)
    }

    pub(super) fn accepted_fits(&self, projected: AcceptedResources) -> bool {
        accepted_fits(self.authority, projected)
    }

    pub(super) fn finish(self) -> Result<MembershipPolicyWitness, PlanError> {
        self.mode.finish(self.authority)
    }
}

fn exact_projection(
    authority: &TxPoolAuthority,
    cached: &mut Option<AcceptedResources>,
    released: AcceptedResources,
    added: AcceptedResources,
) -> Result<AcceptedResources, PlanError> {
    if let Some(projected) = *cached {
        return Ok(projected);
    }
    let projected = authority
        .resources
        .read(&authority.entries)
        .accepted()
        .checked_sub(released)
        .ok_or(PlanError::Fault(AuthorityFault::ResourceProjection))?
        .checked_add(added)
        .ok_or(PlanError::Membership(MembershipReject::AggregateOverflow))?;
    *cached = Some(projected);
    Ok(projected)
}

fn accepted_fits(authority: &TxPoolAuthority, projected: AcceptedResources) -> bool {
    authority
        .resources
        .read(&authority.entries)
        .accepted_fits(projected)
}
