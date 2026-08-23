//! Fixed physical partition for the tx-pool authority.
//!
//! This module owns only representation and routing. It contains no operation
//! enum or policy table: semantic delta types fold their own real keys.

use super::state::{OwnedTx, RawTxHash};
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
    shards: Box<[HashMap<RawTxHash, OwnedTx>; AUTHORITY_SHARD_COUNT]>,
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
            shards: Box::new(std::array::from_fn(|_| HashMap::new())),
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
        self.shards[self.router.owner(key)].get(key)
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
        self.shards[shard].insert(key, owner)
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "owner() masks to the fixed 64-entry array range"
    )]
    pub(in crate::authority) fn remove(&mut self, key: &RawTxHash) -> Option<OwnedTx> {
        self.shards[self.router.owner(key)].remove(key)
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.shards.iter().map(HashMap::len).sum()
    }

    #[cfg(test)]
    pub(in crate::authority) fn capacity(&self) -> usize {
        self.shards.iter().map(HashMap::capacity).sum()
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
            shard.try_reserve(additional)?;
        }
        Ok(())
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
    shards: std::slice::Iter<'map, HashMap<RawTxHash, OwnedTx>>,
    current: Option<hash_map::Iter<'map, RawTxHash, OwnedTx>>,
}

impl<'map> Iterator for ShardedOwnerIter<'map> {
    type Item = (&'map RawTxHash, &'map OwnedTx);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(item) = self.current.as_mut().and_then(Iterator::next) {
                return Some(item);
            }
            self.current = Some(self.shards.next()?.iter());
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
            .map(HashMap::len)
            .fold(current, usize::saturating_add)
    }
}
