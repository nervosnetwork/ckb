//! Fixed shard storage, keyed routing and ordered lock acquisition.
//!
//! Only this module constructs bounded addresses and ordered guard collections.
//! Acquired guards retain their physical address and cannot be reordered by callers.
//! Store and Apply still choose the semantic keys and cross-family lock order;
//! direct access to a single lock does not enforce ordering between separate calls.

use ckb_types::{
    packed::{Byte32, ProposalShortId},
    prelude::*,
};
use ckb_util::parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hash, Hasher},
};

pub(super) const SHARDS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub(super) struct ShardIndex(usize);

impl ShardIndex {
    fn from_hash(hash: u64) -> Self {
        Self((hash as usize) % SHARDS)
    }

    pub(super) fn all() -> impl DoubleEndedIterator<Item = Self> + ExactSizeIterator {
        (0..SHARDS).map(Self)
    }

    #[cfg(test)]
    pub(super) fn position(self) -> usize {
        self.0
    }
}

/// One seed routes owners, proposal IDs and all gate families consistently.
#[derive(Default)]
pub(super) struct Routing(RandomState);

impl Routing {
    pub(super) fn key<T: Hash + ?Sized>(&self, key: &T) -> ShardIndex {
        ShardIndex::from_hash(self.0.hash_one(key))
    }

    pub(super) fn owner(&self, hash: &Byte32) -> ShardIndex {
        // Packed ProposalShortId hashes raw bytes, without a slice-length prefix.
        let mut hasher = self.0.build_hasher();
        hasher.write(proposal_key(hash));
        ShardIndex::from_hash(hasher.finish())
    }
}

#[expect(
    clippy::expect_used,
    reason = "A valid Byte32 has 32 bytes, so its 10-byte proposal prefix always exists."
)]
pub(super) fn proposal_key(hash: &Byte32) -> &[u8; ProposalShortId::TOTAL_SIZE] {
    hash.as_slice()
        .first_chunk()
        .expect("a transaction hash contains a proposal ID")
}

/// Fixed storage keeps addresses, complete captures and revision vectors aligned.
/// The representation and element order are exactly those of the underlying array.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub(super) struct Shards<T>([T; SHARDS]);

impl<T> Shards<T> {
    pub(super) fn new(mut value: impl FnMut() -> T) -> Self {
        Self(std::array::from_fn(|_| value()))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "Only this module constructs ShardIndex, always within the fixed array bounds."
    )]
    pub(super) fn at(&self, index: ShardIndex) -> &T {
        &self.0[index.0]
    }

    // Reconstruction's cursor may point one past the last shard.
    pub(super) fn at_position(&self, position: usize) -> Option<&T> {
        self.0.get(position)
    }

    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = &T> {
        self.0.iter()
    }

    pub(super) fn map<'a, U>(&'a self, project: impl FnMut(&'a T) -> U) -> Shards<U> {
        Shards(self.0.each_ref().map(project))
    }
}

impl<T> Shards<RwLock<T>> {
    pub(super) fn read_all(&self) -> Shards<RwLockReadGuard<'_, T>> {
        self.map(RwLock::read)
    }

    /// Acquire every requested shard once, in ascending order; writes dominate.
    /// Only this producer can construct a ShardGuards collection.
    pub(super) fn acquire(&self, footprint: &LockFootprint) -> ShardGuards<'_, T> {
        let mut guards = Vec::with_capacity(footprint.len());
        for (index, write) in footprint.iter() {
            debug_assert!(guards.last().is_none_or(|(previous, _)| *previous < index));
            let lock = self.at(index);
            guards.push((
                index,
                if write {
                    Guard::Write(lock.write())
                } else {
                    Guard::Read(lock.read())
                },
            ));
        }
        ShardGuards(guards)
    }
}

pub(super) enum Guard<'a, T> {
    Read(RwLockReadGuard<'a, T>),
    Write(RwLockWriteGuard<'a, T>),
}

impl<T> Guard<'_, T> {
    pub(super) fn get(&self) -> &T {
        match self {
            Self::Read(guard) => guard,
            Self::Write(guard) => guard,
        }
    }

    pub(super) fn get_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Read(_) => None,
            Self::Write(guard) => Some(guard),
        }
    }
}

/// Sparse guards retain their address, order and access mode until release.
/// Iteration exposes protected values, never the mutable guard/address pairs.
pub(super) struct ShardGuards<'a, T>(Vec<(ShardIndex, Guard<'a, T>)>);

impl<T> ShardGuards<'_, T> {
    pub(super) fn get(&self, index: ShardIndex) -> Option<&T> {
        self.0
            .binary_search_by_key(&index, |(index, _)| *index)
            .ok()
            .and_then(|position| self.0.get(position))
            .map(|(_, guard)| guard.get())
    }

    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = (ShardIndex, &T)> {
        self.0.iter().map(|(index, guard)| (*index, guard.get()))
    }

    pub(super) fn writes(&self) -> impl Iterator<Item = (ShardIndex, &T)> {
        self.0.iter().filter_map(|(index, guard)| match guard {
            Guard::Read(_) => None,
            Guard::Write(value) => Some((*index, &**value)),
        })
    }

    pub(super) fn writes_mut(&mut self) -> impl Iterator<Item = (ShardIndex, &mut T)> {
        self.0
            .iter_mut()
            .filter_map(|(index, guard)| guard.get_mut().map(|value| (*index, value)))
    }
}

/// Present bits select shards; write bits only upgrade existing read requests.
/// Iteration yields each requested address once, in ascending lock order.
#[derive(Default)]
pub(super) struct LockFootprint {
    present: [u64; SHARDS.div_ceil(64)],
    write: [u64; SHARDS.div_ceil(64)],
}

impl LockFootprint {
    #[expect(
        clippy::indexing_slicing,
        reason = "A bounded ShardIndex selects a word inside the SHARDS-sized bitmap."
    )]
    pub(super) fn insert(&mut self, index: ShardIndex, write: bool) {
        let bit = 1_u64 << (index.0 % 64);
        self.present[index.0 / 64] |= bit;
        if write {
            self.write[index.0 / 64] |= bit;
        }
    }

    pub(super) fn len(&self) -> usize {
        self.present
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Only a nonzero word is decremented; set bits originate from bounded ShardIndex values."
    )]
    pub(super) fn iter(&self) -> impl Iterator<Item = (ShardIndex, bool)> + '_ {
        self.present
            .iter()
            .zip(&self.write)
            .enumerate()
            .flat_map(|(word, (&present, &write))| {
                let mut remaining = present;
                std::iter::from_fn(move || {
                    if remaining == 0 {
                        return None;
                    }
                    let bit = remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    Some((ShardIndex(word * 64 + bit), write & (1_u64 << bit) != 0))
                })
            })
    }
}
