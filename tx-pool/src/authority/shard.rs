//! Fixed physical partition for the tx-pool authority.
//!
//! This module owns only representation and routing. It contains no operation
//! enum or policy table: semantic delta types fold their own real keys.

use super::{
    dependency::{DependencyLevel, UnindexedDependencyLevel},
    indexes::{AcceptedDeadlineKey, DeadlineKey},
    plan::{AcceptedOrderKey, AncestorAggregate, DescendantAggregate, EvictionOrderKey},
    resources::{
        AcceptedResources, ChargeProjection, ResourceError, ResourceTotals, ResourceVector,
    },
    source::PoolTemplateVersions,
    state::{
        AcceptedEntry, AcceptedStatus, ApplySequence, DependencyKey, DependencyOrigin, OwnedTx,
        ProposalId, RawTxHash,
    },
};
use ahash::RandomState;
use ckb_network::PeerIndex;
use ckb_types::packed::OutPoint;
#[cfg(test)]
use ckb_util::parking_lot::Mutex;
use ckb_util::parking_lot::{MappedRwLockReadGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{
    collections::{BTreeSet, HashMap, HashSet, hash_map},
    fmt,
    hash::{BuildHasher, Hash, Hasher},
    ops::Deref,
    sync::Arc,
};

pub(super) const AUTHORITY_SHARD_COUNT: usize = 64;

#[derive(Clone)]
pub(super) struct AuthorityShardRouter {
    state: RandomState,
}

impl AuthorityShardRouter {
    pub(super) fn new() -> Self {
        let source = std::collections::hash_map::RandomState::new();
        let seed = |index: u8| {
            let mut hasher = source.build_hasher();
            b"ckb-tx-pool/authority-shard-router".hash(&mut hasher);
            index.hash(&mut hasher);
            hasher.finish()
        };
        Self {
            state: RandomState::with_seeds(seed(0), seed(1), seed(2), seed(3)),
        }
    }

    pub(in crate::authority) fn shard<K: Hash>(&self, domain: &'static [u8], key: &K) -> usize {
        let mut hasher = self.state.build_hasher();
        domain.hash(&mut hasher);
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (AUTHORITY_SHARD_COUNT - 1)
    }

    pub(in crate::authority) fn owner(&self, key: &RawTxHash) -> usize {
        self.shard(b"owner-resource/owner", key)
    }

    fn peer_resource(&self, peer: &PeerIndex) -> usize {
        self.shard(b"owner-resource/peer", peer)
    }
}

/// Sole physical owner map, partitioned once for one authority-layout
/// lifetime. The current outer authority guard still provides exclusion while
/// the remaining domains migrate; no duplicate flat owner map exists.
#[derive(Clone)]
pub(in crate::authority) struct ShardedOwnerMap {
    pub(in crate::authority) layout: Arc<AuthorityShardLayout>,
}

pub(in crate::authority) struct AuthorityShardLayout {
    pub(in crate::authority) router: AuthorityShardRouter,
    pub(in crate::authority) shards: Box<[RwLock<AuthorityShard>; AUTHORITY_SHARD_COUNT]>,
    #[cfg(test)]
    concurrent_removal_probe: Mutex<Option<Arc<ConcurrentRemovalProbe>>>,
    #[cfg(test)]
    concurrent_removal_plan_probe: Mutex<Option<Arc<std::sync::Barrier>>>,
}

#[cfg(test)]
pub(in crate::authority) struct ConcurrentRemovalProbe {
    entered: std::sync::mpsc::Sender<()>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
impl ConcurrentRemovalProbe {
    pub(in crate::authority) fn new() -> (
        Arc<Self>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        (
            Arc::new(Self {
                entered,
                release: Mutex::new(released),
            }),
            observed,
            release,
        )
    }

    fn enter(&self) {
        let _ = self.entered.send(());
        let _ = self.release.lock().recv();
    }
}

#[derive(Debug, Default)]
pub(in crate::authority) struct AuthorityShard {
    owners: HashMap<RawTxHash, OwnedTx>,
    proposed_count: usize,
    resources: ShardResourceAggregate,
    peer_resources: HashMap<PeerIndex, ResourceVector>,
    pub(in crate::authority) proposals: HashMap<ProposalId, RawTxHash>,
    pub(in crate::authority) preaccepted_by_peer: HashMap<PeerIndex, HashSet<RawTxHash>>,
    pub(in crate::authority) context_sensitive_accepted: HashSet<RawTxHash>,
    pub(in crate::authority) deadlines: BTreeSet<DeadlineKey>,
    pub(in crate::authority) accepted_deadlines: BTreeSet<AcceptedDeadlineKey>,
    pub(in crate::authority) spenders: HashMap<OutPoint, RawTxHash>,
    pub(in crate::authority) dependency_readers: HashMap<OutPoint, HashSet<RawTxHash>>,
    pub(in crate::authority) parents: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) children: HashMap<RawTxHash, HashSet<RawTxHash>>,
    pub(in crate::authority) ancestor_aggregates: HashMap<RawTxHash, AncestorAggregate>,
    pub(in crate::authority) descendant_aggregates: HashMap<RawTxHash, DescendantAggregate>,
    pub(in crate::authority) accepted_order: BTreeSet<AcceptedOrderKey>,
    pub(in crate::authority) eviction_order: BTreeSet<EvictionOrderKey>,
    pub(in crate::authority) dependency_consumers:
        std::collections::BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    pub(in crate::authority) dependency_waiters:
        std::collections::BTreeMap<DependencyKey, BTreeSet<RawTxHash>>,
    pub(in crate::authority) dependency_keys_by_origin:
        std::collections::BTreeMap<DependencyOrigin, BTreeSet<DependencyKey>>,
    pub(in crate::authority) dependency_levels:
        std::collections::BTreeMap<DependencyKey, DependencyLevel>,
    pub(in crate::authority) dependency_unindexed: UnindexedDependencyLevel,
    template_proposals_source: u128,
    template_transactions_source: u128,
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
    router: AuthorityShardRouter,
    shards: [RwLockReadGuard<'map, AuthorityShard>; AUTHORITY_SHARD_COUNT],
}

#[derive(Clone, Copy, Default)]
pub(in crate::authority) struct ShardWriteSupport(u64);

impl ShardWriteSupport {
    pub(in crate::authority) fn insert(&mut self, shard: usize) {
        self.0 |= 1u64 << shard;
    }

    pub(in crate::authority) fn include(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[cfg(test)]
    pub(in crate::authority) fn is_disjoint(self, other: Self) -> bool {
        self.0 & other.0 == 0
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

    pub(in crate::authority) fn projection_shard_mut(
        &mut self,
        shard: usize,
    ) -> &mut AuthorityShard {
        self.shard_mut(shard)
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

    pub(in crate::authority) fn owner_version(
        &self,
        shard: usize,
        key: &RawTxHash,
    ) -> Option<super::state::EntryVersion> {
        self.shards
            .get(shard)
            .and_then(Option::as_deref)
            .and_then(|shard| shard.owners.get(key))
            .map(|owner| owner.record().version)
    }

    pub(in crate::authority) fn apply_proposed_counts(&mut self, plan: ShardProposedCountPlan) {
        for (shard, proposed) in plan.0 {
            self.shard_mut(usize::from(shard)).proposed_count = proposed;
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

    pub(in crate::authority) fn remove_current_owner_resources(
        &mut self,
        entries: &ShardedOwnerMap,
        key: &RawTxHash,
    ) -> Result<Option<OwnedTx>, ResourceError> {
        let owner_shard = entries.layout.router.owner(key);
        let Some(owner) = self
            .shards
            .get(owner_shard)
            .and_then(Option::as_deref)
            .and_then(|shard| shard.owners.get(key))
        else {
            return Ok(None);
        };
        let charge = ChargeProjection::from_validated(Some(owner.charge_record()))?;
        let removes_proposed = matches!(
            owner,
            OwnedTx::Accepted(entry) if entry.status() == AcceptedStatus::Proposed
        );
        let shard = self
            .shards
            .get(owner_shard)
            .and_then(Option::as_deref)
            .ok_or(ResourceError::Arithmetic)?;
        let aggregate_after = shard.resources.checked_remove(charge)?;
        let proposed_after = if removes_proposed {
            shard
                .proposed_count
                .checked_sub(1)
                .ok_or(ResourceError::Arithmetic)?
        } else {
            shard.proposed_count
        };
        let peer_after = charge
            .peer
            .map(|(peer, resources)| {
                let peer_shard = entries.layout.router.peer_resource(&peer);
                let current = self
                    .shards
                    .get(peer_shard)
                    .and_then(Option::as_deref)
                    .and_then(|shard| shard.peer_resources.get(&peer))
                    .copied()
                    .unwrap_or_default();
                current
                    .checked_sub(resources)
                    .map(|after| (peer_shard, peer, after))
                    .ok_or(ResourceError::Arithmetic)
            })
            .transpose()?;

        let removed = {
            let shard = self.shard_mut(owner_shard);
            shard.resources = aggregate_after;
            shard.proposed_count = proposed_after;
            shard.owners.remove(key)
        };
        if let Some((peer_shard, peer, after)) = peer_after {
            let peers = &mut self.shard_mut(peer_shard).peer_resources;
            if after == ResourceVector::default() {
                peers.remove(&peer);
            } else {
                peers.insert(peer, after);
            }
        }
        Ok(removed)
    }

    pub(in crate::authority) fn apply_template_selection_sources(
        &mut self,
        proposals: Option<ApplySequence>,
        transactions: Option<ApplySequence>,
    ) {
        for shard in self.shards.iter_mut().flatten() {
            if let Some(sequence) = proposals {
                shard.template_proposals_source = shard.template_proposals_source.max(sequence.0);
            }
            if let Some(sequence) = transactions {
                shard.template_transactions_source =
                    shard.template_transactions_source.max(sequence.0);
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
            layout: Arc::new(AuthorityShardLayout {
                router,
                shards: Box::new(std::array::from_fn(|_| {
                    RwLock::new(AuthorityShard::default())
                })),
                #[cfg(test)]
                concurrent_removal_probe: Mutex::new(None),
                #[cfg(test)]
                concurrent_removal_plan_probe: Mutex::new(None),
            }),
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
        self.layout.router.clone()
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_concurrent_removal_probe(
        &self,
        probe: Option<Arc<ConcurrentRemovalProbe>>,
    ) {
        *self.layout.concurrent_removal_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_concurrent_removal_probe(&self) {
        let probe = self.layout.concurrent_removal_probe.lock().clone();
        if let Some(probe) = probe {
            probe.enter();
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn set_concurrent_removal_plan_probe(
        &self,
        probe: Option<Arc<std::sync::Barrier>>,
    ) {
        *self.layout.concurrent_removal_plan_probe.lock() = probe;
    }

    #[cfg(test)]
    pub(in crate::authority) fn enter_concurrent_removal_plan_probe(&self) {
        let probe = self.layout.concurrent_removal_plan_probe.lock().clone();
        if let Some(probe) = probe {
            probe.wait();
        }
    }

    pub(in crate::authority) fn owner_resource_write_support<'key>(
        &self,
        owner_keys: impl IntoIterator<Item = &'key RawTxHash>,
        proposed_counts: &ShardProposedCountPlan,
        resources: &ShardResourcePlan,
    ) -> ShardWriteSupport {
        let mut support = ShardWriteSupport::default();
        for key in owner_keys {
            support.insert(self.layout.router.owner(key));
        }
        for (shard, _) in &proposed_counts.0 {
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
            support.insert(self.layout.router.owner(key));
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
                support
                    .contains(shard)
                    .then(|| self.layout.shards[shard].write())
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
            match self.layout.shards[shard].try_write() {
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
        self.layout.router.owner(key)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<ShardedOwnerReadGuard<'_>> {
        RwLockReadGuard::try_map(
            self.layout.shards[self.layout.router.owner(key)].read(),
            |shard| shard.owners.get(key),
        )
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
        let shard = self.layout.router.owner(&key);
        self.layout.shards[shard].write().owners.insert(key, owner)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.layout
            .shards
            .iter()
            .map(|shard| shard.read().owners.len())
            .sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn capacity(&self) -> usize {
        self.layout
            .shards
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
            let shard = self.layout.router.owner(key);
            additional[shard] = additional[shard].saturating_add(1);
        }
        for (shard, additional) in self.layout.shards.iter().zip(additional) {
            shard.write().owners.try_reserve(additional)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::authority) fn status_counts(&self) -> Option<super::plan::StatusCounts> {
        let owners = self.read_all();
        let counts = owners.status_counts()?;
        (owners.proposed_count()? == counts.proposed).then_some(counts)
    }

    pub(in crate::authority) fn resource_totals(
        &self,
    ) -> Option<(ResourceTotals, AcceptedResources)> {
        self.layout.shards.iter().try_fold(
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
        self.layout.shards[self.layout.router.peer_resource(&peer)]
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
        for shard in &self.layout.shards[..] {
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
            let owner_shard = self.layout.router.owner(key);
            let aggregate = aggregate_targets[owner_shard]
                .get_or_insert(self.layout.shards[owner_shard].read().resources);
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
            let shard = self.layout.router.peer_resource(peer);
            if *target != ResourceVector::default()
                && !self.layout.shards[shard]
                    .read()
                    .peer_resources
                    .contains_key(peer)
            {
                peer_insertions[shard] = peer_insertions[shard]
                    .checked_add(1)
                    .ok_or(ResourceError::Arithmetic)?;
            }
        }
        for (shard, additional) in self.layout.shards.iter().zip(peer_insertions) {
            if additional == 0 {
                continue;
            }
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
            peer_targets.into_iter().map(|(peer, target)| {
                (self.layout.router.peer_resource(&peer) as u8, peer, target)
            }),
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
            self.layout.shards[usize::from(shard)].write().resources = aggregate;
        }
        for (shard, peer, target) in plan.peers {
            let mut shard = self.layout.shards[usize::from(shard)].write();
            let rows = &mut shard.peer_resources;
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
    pub(in crate::authority) fn plan_proposed_counts<'change>(
        &self,
        changes: impl IntoIterator<
            Item = (
                &'change RawTxHash,
                Option<AcceptedStatus>,
                Option<AcceptedStatus>,
            ),
        >,
    ) -> Result<ShardProposedCountPlan, ShardProposedCountPlanError> {
        let mut targets = [None; AUTHORITY_SHARD_COUNT];
        let mut changed_shards = 0usize;
        for (key, before, after) in changes {
            let before = before == Some(AcceptedStatus::Proposed);
            let after = after == Some(AcceptedStatus::Proposed);
            if before == after {
                continue;
            }
            let shard = self.layout.router.owner(key);
            let (base, target) = targets[shard].get_or_insert_with(|| {
                let current = self.layout.shards[shard].read().proposed_count;
                (current, current)
            });
            let was_changed = *base != *target;
            if before {
                *target = target
                    .checked_sub(1)
                    .ok_or(ShardProposedCountPlanError::Projection)?;
            }
            if after {
                *target = target
                    .checked_add(1)
                    .ok_or(ShardProposedCountPlanError::Arithmetic)?;
            }
            let is_changed = *base != *target;
            match (was_changed, is_changed) {
                (false, true) => {
                    changed_shards = changed_shards
                        .checked_add(1)
                        .ok_or(ShardProposedCountPlanError::Arithmetic)?;
                }
                (true, false) => {
                    changed_shards = changed_shards
                        .checked_sub(1)
                        .ok_or(ShardProposedCountPlanError::Projection)?;
                }
                (false, false) | (true, true) => {}
            }
        }
        let mut planned = Vec::new();
        planned
            .try_reserve(changed_shards)
            .map_err(|_| ShardProposedCountPlanError::Allocation)?;
        planned.extend(
            targets
                .into_iter()
                .enumerate()
                .filter_map(|(shard, target)| {
                    target
                        .filter(|(before, after)| before != after)
                        .map(|(_, after)| (shard as u8, after))
                }),
        );
        Ok(ShardProposedCountPlan(planned))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "array::from_fn supplies only indices in the fixed 64-shard array"
    )]
    pub(in crate::authority) fn read_all(&self) -> ShardedOwnerReadCut<'_> {
        ShardedOwnerReadCut {
            router: self.layout.router.clone(),
            shards: std::array::from_fn(|shard| self.layout.shards[shard].read()),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn try_read_all(&self) -> Option<ShardedOwnerReadCut<'_>> {
        let guards = self
            .layout
            .shards
            .iter()
            .map(RwLock::try_read)
            .collect::<Option<Vec<_>>>()?;
        let shards = guards.try_into().ok()?;
        Some(ShardedOwnerReadCut {
            router: self.layout.router.clone(),
            shards,
        })
    }
}

impl ShardedOwnerReadCut<'_> {
    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<&OwnedTx> {
        self.shards[self.router.owner(key)].owners.get(key)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.owners.len()).sum()
    }

    pub(in crate::authority) fn proposed_count(&self) -> Option<usize> {
        self.shards.iter().try_fold(0usize, |total, shard| {
            total.checked_add(shard.proposed_count)
        })
    }

    pub(in crate::authority) fn accepted_count(&self) -> Option<usize> {
        self.shards.iter().try_fold(0usize, |total, shard| {
            total.checked_add(shard.resources.accepted.entries)
        })
    }

    #[cfg(test)]
    pub(in crate::authority) fn status_counts(&self) -> Option<super::plan::StatusCounts> {
        self.values().try_fold(
            super::plan::StatusCounts::default(),
            |counts, owner| match owner {
                OwnedTx::Accepted(entry) => counts.checked_add(entry.status()),
                OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_) => Some(counts),
            },
        )
    }

    pub(in crate::authority) fn accepted_resources(&self) -> Option<AcceptedResources> {
        self.shards
            .iter()
            .try_fold(AcceptedResources::default(), |total, shard| {
                total.checked_add(shard.resources.accepted)
            })
    }

    pub(in crate::authority) fn resource_totals(
        &self,
    ) -> Option<(ResourceTotals, AcceptedResources)> {
        self.shards.iter().try_fold(
            (ResourceTotals::default(), AcceptedResources::default()),
            |(totals, accepted), shard| {
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

    pub(in crate::authority) fn template_sources(
        &self,
        mut base: PoolTemplateVersions,
    ) -> PoolTemplateVersions {
        for shard in &self.shards {
            base.proposals = base
                .proposals
                .max(ApplySequence(shard.template_proposals_source));
            base.transactions = base
                .transactions
                .max(ApplySequence(shard.template_transactions_source));
        }
        base
    }

    pub(in crate::authority) fn membership_parents(
        &self,
        key: &RawTxHash,
    ) -> Option<&HashSet<RawTxHash>> {
        let shard = self
            .shards
            .get(self.router.shard(b"membership/parents", key))?;
        shard.parents.get(key)
    }

    pub(in crate::authority) fn proposal_owner(&self, proposal: &ProposalId) -> Option<&RawTxHash> {
        self.shards
            .get(self.router.shard(b"index/proposal", proposal))?
            .proposals
            .get(proposal)
    }

    pub(in crate::authority) fn membership_ancestor(
        &self,
        key: &RawTxHash,
    ) -> Option<AncestorAggregate> {
        self.shards
            .get(self.router.shard(b"membership/ancestor", key))?
            .ancestor_aggregates
            .get(key)
            .copied()
    }

    pub(in crate::authority) fn membership_descendant(
        &self,
        key: &RawTxHash,
    ) -> Option<DescendantAggregate> {
        self.shards
            .get(self.router.shard(b"membership/descendant", key))?
            .descendant_aggregates
            .get(key)
            .copied()
    }

    pub(in crate::authority) fn accepted_order(&self) -> Vec<AcceptedOrderKey> {
        let count = self
            .shards
            .iter()
            .map(|shard| shard.accepted_order.len())
            .sum();
        let mut order = Vec::with_capacity(count);
        for shard in &self.shards {
            order.extend(shard.accepted_order.iter().cloned());
        }
        order.sort_unstable();
        order
    }

    pub(in crate::authority) fn contains_accepted_order(&self, key: &AcceptedOrderKey) -> bool {
        self.shards
            .get(self.router.shard(b"membership/accepted-order", key.hash()))
            .is_some_and(|shard| shard.accepted_order.contains(key))
    }

    pub(in crate::authority) fn contains_eviction_order(&self, key: &EvictionOrderKey) -> bool {
        self.shards
            .get(self.router.shard(b"membership/eviction-order", &key.hash))
            .is_some_and(|shard| shard.eviction_order.contains(key))
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
pub(in crate::authority) enum ShardProposedCountPlanError {
    Projection,
    Arithmetic,
    Allocation,
}

#[derive(Debug, Default)]
pub(in crate::authority) struct ShardProposedCountPlan(Vec<(u8, usize)>);

#[cfg(test)]
impl ShardProposedCountPlan {
    pub(in crate::authority) fn len(&self) -> usize {
        self.0.len()
    }
}
