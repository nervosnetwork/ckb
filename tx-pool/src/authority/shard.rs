//! Fixed physical partition for the tx-pool authority.
//!
//! This module owns only representation and routing. It contains no operation
//! enum or policy table: semantic delta types fold their own real keys.

use super::{
    plan::StatusCounts,
    resources::{
        AcceptedResources, ChargeProjection, ResourceError, ResourceTotals, ResourceVector,
    },
    state::{AcceptedEntry, AcceptedStatus, OwnedTx, RawTxHash},
};
use ckb_network::PeerIndex;
use ckb_util::parking_lot::{MappedRwLockReadGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{
    collections::{HashMap, hash_map},
    fmt,
    hash::{BuildHasher, Hash, Hasher, RandomState},
    ops::Deref,
};

pub(super) const AUTHORITY_SHARD_COUNT: usize = 64;

#[derive(Clone)]
pub(super) struct AuthorityShardRouter {
    state: RandomState,
}

impl AuthorityShardRouter {
    pub(super) fn new() -> Self {
        Self {
            state: RandomState::new(),
        }
    }

    fn shard<K: Hash>(&self, domain: &'static [u8], key: &K) -> usize {
        let mut hasher = self.state.build_hasher();
        domain.hash(&mut hasher);
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (AUTHORITY_SHARD_COUNT - 1)
    }

    fn owner(&self, key: &RawTxHash) -> usize {
        self.shard(b"owner-resource/owner", key)
    }

    fn peer_resource(&self, peer: &PeerIndex) -> usize {
        self.shard(b"owner-resource/peer", peer)
    }
}

/// Sole physical owner map, partitioned once for one authority-layout
/// lifetime. The current outer authority guard still provides exclusion while
/// the remaining domains migrate; no duplicate flat owner map exists.
pub(in crate::authority) struct ShardedOwnerMap {
    router: AuthorityShardRouter,
    shards: Box<[RwLock<AuthorityShard>; AUTHORITY_SHARD_COUNT]>,
}

#[derive(Debug, Default)]
struct AuthorityShard {
    owners: HashMap<RawTxHash, OwnedTx>,
    membership_counts: StatusCounts,
    resources: ShardResourceAggregate,
    peer_resources: HashMap<PeerIndex, ResourceVector>,
}

/// A point read keeps the owning shard locked for the complete lifetime of
/// the borrowed transaction.  Returning this guard instead of cloning an
/// `OwnedTx` is the first production representation boundary that remains
/// sound after the outer authority lock is removed.
pub(in crate::authority) struct ShardedOwnerReadGuard<'map> {
    owner: MappedRwLockReadGuard<'map, OwnedTx>,
}

pub(in crate::authority) struct ShardedAcceptedReadGuard<'map> {
    entry: MappedRwLockReadGuard<'map, AcceptedEntry>,
}

impl Deref for ShardedOwnerReadGuard<'_> {
    type Target = OwnedTx;

    fn deref(&self) -> &Self::Target {
        &self.owner
    }
}

impl<'map> ShardedOwnerReadGuard<'map> {
    pub(in crate::authority) fn into_accepted(
        self,
    ) -> Result<ShardedAcceptedReadGuard<'map>, Self> {
        match MappedRwLockReadGuard::try_map(self.owner, |owner| match owner {
            OwnedTx::Accepted(entry) => Some(entry),
            OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => None,
        }) {
            Ok(entry) => Ok(ShardedAcceptedReadGuard { entry }),
            Err(owner) => Err(Self { owner }),
        }
    }
}

impl Deref for ShardedAcceptedReadGuard<'_> {
    type Target = AcceptedEntry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

/// One coherent full-owner read cut.  Full snapshots and ordered queries
/// deliberately hold all fixed shards; point and bounded reads use the point
/// guard above instead.
pub(in crate::authority) struct ShardedOwnerReadCut<'map> {
    shards: [RwLockReadGuard<'map, AuthorityShard>; AUTHORITY_SHARD_COUNT],
}

#[derive(Clone, Copy, Default)]
pub(in crate::authority) struct ShardWriteSupport(u64);

impl ShardWriteSupport {
    fn insert(&mut self, shard: usize) {
        self.0 |= 1u64 << shard;
    }

    fn contains(self, shard: usize) -> bool {
        self.0 & (1u64 << shard) != 0
    }
}

/// Sorted fixed-layout write bundle. Construction walks the 64 physical
/// shards in ascending order and allocates nothing, so two disjoint bundles
/// can overlap while a multi-shard transition remains atomic to readers.
pub(in crate::authority) struct ShardedOwnerWriteCut<'map> {
    shards: [Option<RwLockWriteGuard<'map, AuthorityShard>>; AUTHORITY_SHARD_COUNT],
}

impl ShardedOwnerWriteCut<'_> {
    #[expect(
        clippy::expect_used,
        reason = "support is folded from the same owner/status/resource plans consumed by this sealed write cut"
    )]
    fn shard_mut(&mut self, shard: usize) -> &mut AuthorityShard {
        self.shards
            .get_mut(shard)
            .and_then(Option::as_deref_mut)
            .expect("write support contains every shard consumed by the prepared owner delta")
    }

    pub(in crate::authority) fn replace(
        &mut self,
        shard: usize,
        key: RawTxHash,
        after: Option<OwnedTx>,
    ) -> Option<OwnedTx> {
        let owners = &mut self.shard_mut(shard).owners;
        match after {
            Some(after) => owners.insert(key, after),
            None => owners.remove(&key),
        }
    }

    pub(in crate::authority) fn apply_status_counts(&mut self, plan: ShardStatusCountPlan) {
        for (shard, counts) in plan.0 {
            self.shard_mut(usize::from(shard)).membership_counts = counts;
        }
    }

    pub(in crate::authority) fn apply_resource_plan(&mut self, plan: ShardResourcePlan) {
        for (shard, aggregate) in plan.aggregates {
            self.shard_mut(usize::from(shard)).resources = aggregate;
        }
        for (shard, peer, target) in plan.peers {
            let rows = &mut self.shard_mut(usize::from(shard)).peer_resources;
            if target == ResourceVector::default() {
                rows.remove(&peer);
            } else {
                rows.insert(peer, target);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ShardResourceAggregate {
    totals: ResourceTotals,
    accepted: AcceptedResources,
}

impl ShardResourceAggregate {
    fn checked_remove(self, charge: ChargeProjection) -> Result<Self, ResourceError> {
        Ok(Self {
            totals: self.totals.checked_remove(charge)?,
            accepted: charge
                .accepted
                .map_or(Some(self.accepted), |resources| {
                    self.accepted.checked_sub(resources)
                })
                .ok_or(ResourceError::Arithmetic)?,
        })
    }

    fn checked_add(self, charge: ChargeProjection) -> Result<Self, ResourceError> {
        Ok(Self {
            totals: self.totals.checked_add(charge)?,
            accepted: charge
                .accepted
                .map_or(Some(self.accepted), |resources| {
                    self.accepted.checked_add(resources)
                })
                .ok_or(ResourceError::Arithmetic)?,
        })
    }
}

pub(in crate::authority) struct ShardResourcePlan {
    aggregates: Vec<(u8, ShardResourceAggregate)>,
    peers: Vec<(u8, PeerIndex, ResourceVector)>,
}

#[cfg(test)]
impl ShardResourcePlan {
    pub(in crate::authority) fn extend_shard_support(
        &self,
        support: &mut super::shard_support::AuthorityShardSupport,
    ) {
        for (_, peer, _) in &self.peers {
            support.insert(b"owner-resource/peer", peer);
        }
    }
}

impl fmt::Debug for ShardedOwnerMap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ShardedOwnerMap")
            .field("shards", &AUTHORITY_SHARD_COUNT)
            .field("owners", &self.len())
            .finish()
    }
}

impl ShardedOwnerMap {
    pub(in crate::authority) fn new(router: AuthorityShardRouter) -> Self {
        Self {
            router,
            shards: Box::new(std::array::from_fn(|_| {
                RwLock::new(AuthorityShard::default())
            })),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn from_iter_for_test(
        entries: impl IntoIterator<Item = (RawTxHash, OwnedTx)>,
    ) -> Self {
        let mut result = Self::new(AuthorityShardRouter::new());
        for (key, owner) in entries {
            let after = ChargeProjection::from_validated(Some(owner.charge_record()))
                .expect("fixture owner contains a valid production charge");
            let plan = result
                .plan_resource_transitions(std::iter::once((
                    &key,
                    ChargeProjection::from_validated(None)
                        .expect("an absent owner has an empty resource projection"),
                    after,
                )))
                .expect("fixture resource aggregates remain representable");
            result.insert(key, owner);
            result.apply_resource_plan(plan);
        }
        result
    }

    pub(in crate::authority) fn router(&self) -> AuthorityShardRouter {
        self.router.clone()
    }

    pub(in crate::authority) fn owner_resource_write_support<'key>(
        &self,
        owner_keys: impl IntoIterator<Item = &'key RawTxHash>,
        status_counts: &ShardStatusCountPlan,
        resources: &ShardResourcePlan,
    ) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for key in owner_keys {
            support.insert(self.router.owner(key));
        }
        for (shard, _) in &status_counts.0 {
            support.insert(usize::from(*shard));
        }
        for (shard, _) in &resources.aggregates {
            support.insert(usize::from(*shard));
        }
        for (shard, _, _) in &resources.peers {
            support.insert(usize::from(*shard));
        }
        support
    }

    #[cfg(test)]
    pub(in crate::authority) fn owner_write_support<'key>(
        &self,
        owner_keys: impl IntoIterator<Item = &'key RawTxHash>,
    ) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for key in owner_keys {
            support.insert(self.router.owner(key));
        }
        support
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "array::from_fn supplies only indices in the fixed 64-shard array"
    )]
    pub(in crate::authority) fn write_cut(
        &self,
        support: ShardWriteSupport,
    ) -> ShardedOwnerWriteCut<'_> {
        ShardedOwnerWriteCut {
            shards: std::array::from_fn(|shard| {
                support.contains(shard).then(|| self.shards[shard].write())
            }),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn try_write_cut(
        &self,
        support: ShardWriteSupport,
    ) -> Option<ShardedOwnerWriteCut<'_>> {
        let mut unavailable = false;
        let shards = std::array::from_fn(|shard| {
            if !support.contains(shard) {
                return None;
            }
            match self.shards[shard].try_write() {
                Some(guard) => Some(guard),
                None => {
                    unavailable = true;
                    None
                }
            }
        });
        (!unavailable).then_some(ShardedOwnerWriteCut { shards })
    }

    pub(in crate::authority) fn owner_shard(&self, key: &RawTxHash) -> usize {
        self.router.owner(key)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<ShardedOwnerReadGuard<'_>> {
        RwLockReadGuard::try_map(self.shards[self.router.owner(key)].read(), |shard| {
            shard.owners.get(key)
        })
        .ok()
        .map(|owner| ShardedOwnerReadGuard { owner })
    }

    pub(in crate::authority) fn contains_key(&self, key: &RawTxHash) -> bool {
        self.get(key).is_some()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    #[cfg(test)]
    pub(in crate::authority) fn insert(
        &mut self,
        key: RawTxHash,
        owner: OwnedTx,
    ) -> Option<OwnedTx> {
        let shard = self.router.owner(&key);
        self.shards[shard].get_mut().owners.insert(key, owner)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().owners.len())
            .sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn capacity(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().owners.capacity())
            .sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn snapshot_for_test(&self) -> Vec<(RawTxHash, OwnedTx)> {
        let owners = self.read_all();
        owners
            .iter()
            .map(|(hash, owner)| (hash.clone(), owner.clone()))
            .collect()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn try_reserve_keys<'key>(
        &mut self,
        keys: impl IntoIterator<Item = &'key RawTxHash>,
    ) -> Result<(), std::collections::TryReserveError> {
        let mut additional = [0usize; AUTHORITY_SHARD_COUNT];
        for key in keys {
            let shard = self.router.owner(key);
            additional[shard] = additional[shard].saturating_add(1);
        }
        for (shard, additional) in self.shards.iter_mut().zip(additional) {
            shard.get_mut().owners.try_reserve(additional)?;
        }
        Ok(())
    }

    pub(in crate::authority) fn status_counts(&self) -> Option<StatusCounts> {
        self.shards
            .iter()
            .try_fold(StatusCounts::default(), |total, shard| {
                total.checked_add_counts(shard.read().membership_counts)
            })
    }

    pub(in crate::authority) fn resource_totals(
        &self,
    ) -> Option<(ResourceTotals, AcceptedResources)> {
        self.shards.iter().try_fold(
            (ResourceTotals::default(), AcceptedResources::default()),
            |(totals, accepted), shard| {
                let shard = shard.read();
                Some((
                    ResourceTotals {
                        preaccepted: totals
                            .preaccepted
                            .checked_add(shard.resources.totals.preaccepted)?,
                        remote: totals.remote.checked_add(shard.resources.totals.remote)?,
                        replacement_history: totals
                            .replacement_history
                            .checked_add(shard.resources.totals.replacement_history)?,
                    },
                    accepted.checked_add(shard.resources.accepted)?,
                ))
            },
        )
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "peer_resource() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn peer_resource(&self, peer: PeerIndex) -> ResourceVector {
        self.shards[self.router.peer_resource(&peer)]
            .read()
            .peer_resources
            .get(&peer)
            .copied()
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::authority) fn peer_resources_snapshot_for_test(
        &self,
    ) -> HashMap<PeerIndex, ResourceVector> {
        let mut peers = HashMap::new();
        for shard in &self.shards[..] {
            let shard = shard.read();
            peers.extend(
                shard
                    .peer_resources
                    .iter()
                    .map(|(peer, resources)| (*peer, *resources)),
            );
        }
        peers
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "router outputs mask to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn plan_resource_transitions<'change>(
        &self,
        changes: impl IntoIterator<Item = (&'change RawTxHash, ChargeProjection, ChargeProjection)>,
    ) -> Result<ShardResourcePlan, ResourceError> {
        let mut aggregate_targets = [None; AUTHORITY_SHARD_COUNT];
        let mut peer_targets = HashMap::new();
        for (key, before, after) in changes {
            let owner_shard = self.router.owner(key);
            let aggregate = aggregate_targets[owner_shard]
                .get_or_insert(self.shards[owner_shard].read().resources);
            *aggregate = aggregate.checked_remove(before)?.checked_add(after)?;

            for (peer, resources, add) in before
                .peer
                .map(|(peer, resources)| (peer, resources, false))
                .into_iter()
                .chain(after.peer.map(|(peer, resources)| (peer, resources, true)))
            {
                let target = peer_targets
                    .entry(peer)
                    .or_insert_with(|| self.peer_resource(peer));
                *target = if add {
                    target
                        .checked_add(resources)
                        .ok_or(ResourceError::Arithmetic)?
                } else {
                    target
                        .checked_sub(resources)
                        .ok_or(ResourceError::Arithmetic)?
                };
            }
        }

        let mut peer_insertions = [0usize; AUTHORITY_SHARD_COUNT];
        for (peer, target) in &peer_targets {
            let shard = self.router.peer_resource(peer);
            if *target != ResourceVector::default()
                && !self.shards[shard].read().peer_resources.contains_key(peer)
            {
                peer_insertions[shard] = peer_insertions[shard]
                    .checked_add(1)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        for (shard, additional) in self.shards.iter().zip(peer_insertions) {
            shard
                .write()
                .peer_resources
                .try_reserve(additional)
                .map_err(|_| ResourceError::Allocation)?;
        }

        let mut aggregates = Vec::new();
        aggregates
            .try_reserve(AUTHORITY_SHARD_COUNT)
            .map_err(|_| ResourceError::Allocation)?;
        aggregates.extend(
            aggregate_targets
                .into_iter()
                .enumerate()
                .filter_map(|(shard, target)| target.map(|target| (shard as u8, target))),
        );
        let mut peers = Vec::new();
        peers
            .try_reserve(peer_targets.len())
            .map_err(|_| ResourceError::Allocation)?;
        peers.extend(
            peer_targets
                .into_iter()
                .map(|(peer, target)| (self.router.peer_resource(&peer) as u8, peer, target)),
        );
        Ok(ShardResourcePlan { aggregates, peers })
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "planned shard ids originate only from masked router outputs or fixed enumeration"
    )]
    #[cfg(test)]
    pub(in crate::authority) fn apply_resource_plan(&mut self, plan: ShardResourcePlan) {
        for (shard, aggregate) in plan.aggregates {
            self.shards[usize::from(shard)].get_mut().resources = aggregate;
        }
        for (shard, peer, target) in plan.peers {
            let rows = &mut self.shards[usize::from(shard)].get_mut().peer_resources;
            if target == ResourceVector::default() {
                rows.remove(&peer);
            } else {
                rows.insert(peer, target);
            }
        }
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn plan_status_counts<'change>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'change RawTxHash,
                Option<AcceptedStatus>,
                Option<AcceptedStatus>,
            ),
        >,
    ) -> Result<ShardStatusCountPlan, ShardStatusCountPlanError> {
        let mut targets = [None; AUTHORITY_SHARD_COUNT];
        for (key, before, after) in changes {
            let shard = self.router.owner(key);
            let target = targets[shard].get_or_insert(self.shards[shard].read().membership_counts);
            if let Some(before) = before {
                *target = target
                    .checked_sub(before)
                    .ok_or(ShardStatusCountPlanError::Projection)?;
            }
            if let Some(after) = after {
                *target = target
                    .checked_add(after)
                    .ok_or(ShardStatusCountPlanError::Arithmetic)?;
            }
        }
        let mut planned = Vec::new();
        planned
            .try_reserve(AUTHORITY_SHARD_COUNT)
            .map_err(|_| ShardStatusCountPlanError::Allocation)?;
        planned.extend(
            targets
                .into_iter()
                .enumerate()
                .filter_map(|(shard, counts)| counts.map(|counts| (shard as u8, counts))),
        );
        Ok(ShardStatusCountPlan(planned))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "array::from_fn supplies only indices in the fixed 64-shard array"
    )]
    pub(in crate::authority) fn read_all(&self) -> ShardedOwnerReadCut<'_> {
        ShardedOwnerReadCut {
            shards: std::array::from_fn(|shard| self.shards[shard].read()),
        }
    }
}

impl ShardedOwnerReadCut<'_> {
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<&OwnedTx> {
        self.shards.iter().find_map(|shard| shard.owners.get(key))
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.owners.len()).sum()
    }

    pub(in crate::authority) fn status_counts(&self) -> Option<StatusCounts> {
        self.shards
            .iter()
            .try_fold(StatusCounts::default(), |total, shard| {
                total.checked_add_counts(shard.membership_counts)
            })
    }

    pub(in crate::authority) fn iter(&self) -> ShardedOwnerIter<'_> {
        ShardedOwnerIter {
            shards: self.shards.iter(),
            current: None,
        }
    }

    pub(in crate::authority) fn values(&self) -> impl ExactSizeIterator<Item = &OwnedTx> + '_ {
        self.iter().map(|(_, owner)| owner)
    }
}

impl<'map> IntoIterator for &'map ShardedOwnerReadCut<'_> {
    type Item = (&'map RawTxHash, &'map OwnedTx);
    type IntoIter = ShardedOwnerIter<'map>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub(in crate::authority) struct ShardedOwnerIter<'map> {
    shards: std::slice::Iter<'map, RwLockReadGuard<'map, AuthorityShard>>,
    current: Option<hash_map::Iter<'map, RawTxHash, OwnedTx>>,
}

impl<'map> Iterator for ShardedOwnerIter<'map> {
    type Item = (&'map RawTxHash, &'map OwnedTx);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.current.as_mut().and_then(Iterator::next) {
                return Some(item);
            }
            self.current = Some(self.shards.next()?.owners.iter());
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.len()))
    }
}

impl ExactSizeIterator for ShardedOwnerIter<'_> {
    fn len(&self) -> usize {
        let current = self.current.as_ref().map_or(0, ExactSizeIterator::len);
        self.shards
            .clone()
            .map(|shard| shard.owners.len())
            .fold(current, usize::saturating_add)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum ShardStatusCountPlanError {
    Projection,
    Arithmetic,
    Allocation,
}

#[derive(Debug, Default)]
pub(in crate::authority) struct ShardStatusCountPlan(Vec<(u8, StatusCounts)>);
