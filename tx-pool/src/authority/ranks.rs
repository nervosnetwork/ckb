//! Incremental pending-rank index for `get_pool_tx_details`.
//!
//! Pending accepted transactions are stored in sorted chunks paired with a
//! direct hash lookup so inserts, removals, and rank queries stay sub-linear
//! without scanning the entire pool or splitting across shards.

use super::{
    membership::{self, Aggregate},
    model::{Entry, Error, Status},
    query::score,
    state::FastHashMap,
};
use crate::component::sort_key::TransactionPriority;
use ckb_snapshot::Snapshot;

pub(super) type Priority = TransactionPriority<[u8; 32]>;

const CHUNK_TARGET: usize = 128;
const CHUNK_SPLIT: usize = CHUNK_TARGET * 2;

#[derive(Default)]
pub(super) struct RankedPriorities {
    generation: u64,
    longest: usize,
    chunks: Vec<Vec<Priority>>,
    by_hash: FastHashMap<[u8; 32], Priority>,
}

impl RankedPriorities {
    pub(super) fn empty(generation: u64) -> Self {
        Self {
            generation,
            longest: 0,
            chunks: Vec::new(),
            by_hash: FastHashMap::default(),
        }
    }

    pub(super) fn invalid() -> Self {
        Self::empty(u64::MAX)
    }

    pub(super) fn invalidate(&mut self) {
        if self.generation != u64::MAX || !self.by_hash.is_empty() {
            *self = Self::invalid();
        }
    }

    pub(super) fn is_current(&self, generation: u64, limit: usize) -> bool {
        generation != u64::MAX && self.generation == generation && self.longest <= limit
    }

    pub(super) fn update(
        &mut self,
        generation: u64,
        hash: &[u8; 32],
        before: Option<&Entry>,
        after: Option<&Entry>,
        snapshot: &Snapshot,
        ancestors: Option<Aggregate>,
    ) {
        if !self.is_current(generation, usize::MAX) {
            self.invalidate();
            return;
        }
        if let Some(value) = before.and_then(Entry::accepted)
            && value.status(snapshot) != Status::Proposed
            && !self.remove(hash)
        {
            return;
        }
        if let Some(owner) = after
            && let Some(value) = owner.accepted()
        {
            let Some(sum) =
                ancestors.or_else(|| value.parents.is_empty().then(|| Aggregate::one(value)))
            else {
                self.invalidate();
                return;
            };
            self.longest = self.longest.max(sum.count);
            if value.status(snapshot) != Status::Proposed {
                let Ok(score) = score(value, sum) else {
                    self.invalidate();
                    return;
                };
                self.insert(Priority {
                    score,
                    arrival: owner.arrival,
                    hash: *hash,
                });
            }
        }
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Chunk split indices are bounded by CHUNK_SPLIT."
    )]
    pub(super) fn insert(&mut self, priority: Priority) {
        let hash = priority.hash;
        if self.by_hash.insert(hash, priority.clone()).is_some() {
            // Unexpected duplicate priority key invalidates the projection.
            self.invalidate();
            return;
        }
        if self.chunks.is_empty() {
            self.chunks.push(vec![priority]);
            return;
        }
        let chunk_idx = self
            .chunks
            .partition_point(|chunk| chunk.last().is_some_and(|last| last < &priority))
            .min(self.chunks.len().saturating_sub(1));
        let Some(chunk) = self.chunks.get_mut(chunk_idx) else {
            self.invalidate();
            return;
        };
        let pos = chunk.partition_point(|entry| entry < &priority);
        chunk.insert(pos, priority);
        if chunk.len() >= CHUNK_SPLIT {
            let upper = chunk.split_off(CHUNK_TARGET);
            self.chunks.insert(chunk_idx + 1, upper);
        }
    }

    /// Retire one pending entry by hash in O(log chunks + chunk_len) without a
    /// full scan. A missing member invalidates the projection.
    pub(super) fn remove(&mut self, hash: &[u8; 32]) -> bool {
        let Some(priority) = self.by_hash.remove(hash) else {
            self.invalidate();
            return false;
        };
        let chunk_idx = self
            .chunks
            .partition_point(|chunk| chunk.last().is_some_and(|last| last < &priority));
        let Some(chunk) = self.chunks.get_mut(chunk_idx) else {
            self.invalidate();
            return false;
        };
        let Ok(pos) = chunk.binary_search(&priority) else {
            self.invalidate();
            return false;
        };
        chunk.remove(pos);
        if chunk.is_empty() {
            self.chunks.remove(chunk_idx);
        }
        true
    }

    /// Number of indexed pending entries strictly ahead of `target`.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Counts are bounded by the chunked pending population."
    )]
    pub(super) fn ahead(&self, target: &Priority) -> usize {
        let chunk_idx = self
            .chunks
            .partition_point(|chunk| chunk.last().is_some_and(|last| last <= target));
        let Some((first, tail)) = self.chunks.get(chunk_idx..).and_then(|s| s.split_first()) else {
            return 0;
        };
        let pos = first.partition_point(|entry| entry <= target);
        (first.len() - pos) + tail.iter().map(Vec::len).sum::<usize>()
    }

    /// Rebuild the complete pending rank index off-lock from a captured accepted cut.
    pub(super) fn rebuild(
        owners: &[std::sync::Arc<Entry>],
        snapshot: &Snapshot,
        generation: u64,
        limit: usize,
    ) -> Result<Self, Error> {
        let entries: Vec<&Entry> = owners.iter().map(std::sync::Arc::as_ref).collect();
        let mut longest = 0usize;
        let mut pending = Vec::new();
        membership::visit_ancestors(&entries, limit, |root, _, sum| {
            let Some(owner) = owners.get(root) else {
                return Err(Error::Fault("rank rebuild ordinal"));
            };
            let value = owner.accepted().ok_or(Error::Stale)?;
            longest = longest.max(sum.count);
            if value.status(snapshot) != Status::Proposed {
                pending.push(Priority {
                    score: score(value, sum)?,
                    arrival: owner.arrival,
                    hash: owner.hash_bytes(),
                });
            }
            Ok(())
        })?;
        pending.sort_unstable();
        let mut by_hash = FastHashMap::with_capacity_and_hasher(pending.len(), Default::default());
        for priority in &pending {
            by_hash.insert(priority.hash, priority.clone());
        }
        let chunks = pending
            .chunks(CHUNK_TARGET)
            .map(|slice| slice.to_vec())
            .collect();
        Ok(Self {
            generation,
            longest,
            chunks,
            by_hash,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::sort_key::AncestorsScoreSortKey;
    use ckb_types::core::Capacity;

    fn priority(fee: u64, arrival: u64, id: u32) -> Priority {
        let mut hash = [0u8; 32];
        hash[..4].copy_from_slice(&id.to_be_bytes());
        Priority {
            score: AncestorsScoreSortKey {
                fee: Capacity::shannons(fee),
                weight: 100,
                ancestors_fee: Capacity::shannons(fee),
                ancestors_weight: 100,
            },
            arrival,
            hash,
        }
    }

    #[test]
    fn chunked_ranks_match_sorted_reference_across_splits_and_removals() {
        let mut ranks = RankedPriorities::empty(0);
        let mut reference = Vec::new();
        for i in 0..1_000u32 {
            let fee = u64::from((i * 37) % 113);
            let p = priority(fee, u64::from(i), i);
            ranks.insert(p.clone());
            reference.push(p);
        }
        reference.sort();
        for (idx, p) in reference.iter().enumerate() {
            assert_eq!(ranks.ahead(p), reference.len() - 1 - idx);
        }
        for i in (0..1_000u32).step_by(2) {
            let mut hash = [0u8; 32];
            hash[..4].copy_from_slice(&i.to_be_bytes());
            assert!(ranks.remove(&hash));
            reference.retain(|p| p.hash != hash);
        }
        assert!(ranks.is_current(0, usize::MAX));
        for (idx, p) in reference.iter().enumerate() {
            assert_eq!(ranks.ahead(p), reference.len() - 1 - idx);
        }
    }
}
