//! Fixed physical partition for the tx-pool authority.
//!
//! This module owns only representation and routing. It contains no operation
//! enum or policy table: semantic delta types fold their own real keys.

use super::{
    plan::StatusCounts,
    state::{AcceptedStatus, OwnedTx, RawTxHash},
};
use std::{
    collections::{HashMap, hash_map},
    fmt,
    hash::{BuildHasher, Hash, Hasher, RandomState},
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
}

/// Sole physical owner map, partitioned once for one authority-layout
/// lifetime. The current outer authority guard still provides exclusion while
/// the remaining domains migrate; no duplicate flat owner map exists.
pub(in crate::authority) struct ShardedOwnerMap {
    router: AuthorityShardRouter,
    shards: Box<[AuthorityShard; AUTHORITY_SHARD_COUNT]>,
}

#[derive(Debug, Default)]
struct AuthorityShard {
    owners: HashMap<RawTxHash, OwnedTx>,
    membership_counts: StatusCounts,
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
            shards: Box::new(std::array::from_fn(|_| AuthorityShard::default())),
        }
    }

    #[cfg(test)]
    pub(in crate::authority) fn from_iter_for_test(
        entries: impl IntoIterator<Item = (RawTxHash, OwnedTx)>,
    ) -> Self {
        let mut result = Self::new(AuthorityShardRouter::new());
        for (key, owner) in entries {
            result.insert(key, owner);
        }
        result
    }

    pub(in crate::authority) fn router(&self) -> AuthorityShardRouter {
        self.router.clone()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn get(&self, key: &RawTxHash) -> Option<&OwnedTx> {
        self.shards[self.router.owner(key)].owners.get(key)
    }

    pub(in crate::authority) fn contains_key(&self, key: &RawTxHash) -> bool {
        self.get(key).is_some()
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn insert(
        &mut self,
        key: RawTxHash,
        owner: OwnedTx,
    ) -> Option<OwnedTx> {
        let shard = self.router.owner(&key);
        self.shards[shard].owners.insert(key, owner)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn remove(&mut self, key: &RawTxHash) -> Option<OwnedTx> {
        self.shards[self.router.owner(key)].owners.remove(key)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.owners.len()).sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn capacity(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.owners.capacity())
            .sum()
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
            shard.owners.try_reserve(additional)?;
        }
        Ok(())
    }

    pub(in crate::authority) fn status_counts(&self) -> Option<StatusCounts> {
        self.shards
            .iter()
            .try_fold(StatusCounts::default(), |total, shard| {
                total.checked_add_counts(shard.membership_counts)
            })
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
            let target = targets[shard].get_or_insert(self.shards[shard].membership_counts);
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
        reason = "planned shard ids originate only from the fixed array enumeration"
    )]
    pub(in crate::authority) fn apply_status_counts(&mut self, plan: ShardStatusCountPlan) {
        for (shard, counts) in plan.0 {
            self.shards[usize::from(shard)].membership_counts = counts;
        }
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

impl<'map> IntoIterator for &'map ShardedOwnerMap {
    type Item = (&'map RawTxHash, &'map OwnedTx);
    type IntoIter = ShardedOwnerIter<'map>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub(in crate::authority) struct ShardedOwnerIter<'map> {
    shards: std::slice::Iter<'map, AuthorityShard>,
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
