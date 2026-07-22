use ckb_types::core::{BlockNumber, EpochExt, UncleBlockView};
use std::collections::{BTreeMap, HashSet, btree_map::Entry};

use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;

#[cfg(not(test))]
const MAX_CANDIDATE_UNCLES: usize = 128;
#[cfg(test)]
pub(crate) const MAX_CANDIDATE_UNCLES: usize = 4;

#[cfg(not(test))]
const MAX_PER_HEIGHT: usize = 10;
#[cfg(test)]
pub(crate) const MAX_PER_HEIGHT: usize = 2;

/// Candidate uncles container
pub struct CandidateUncles {
    pub(crate) map: BTreeMap<BlockNumber, HashSet<UncleBlockView>>,
    count: usize,
}

impl CandidateUncles {
    /// Construct new candidate uncles container
    pub fn new() -> CandidateUncles {
        CandidateUncles {
            map: BTreeMap::new(),
            count: 0,
        }
    }

    /// insert new candidate uncles
    /// If the map did not have this value present, true is returned.
    /// If the map did have this value present, false is returned.
    pub fn insert(&mut self, uncle: UncleBlockView) -> bool {
        let number: BlockNumber = uncle.header().number();
        if self.count >= MAX_CANDIDATE_UNCLES {
            let first_key = *self.map.keys().next().expect("length checked");
            if number > first_key {
                if let Some(set) = self.map.remove(&first_key) {
                    self.count -= set.len();
                }
            } else if number < first_key {
                return false;
            }
            // `number == first_key`: fall through into the existing height
            // set. MAX_CANDIDATE_UNCLES is a soft cap and MAX_PER_HEIGHT
            // bounds the excess — rejecting the boundary height while
            // evicting it for higher heights would be arbitrary.
        }

        let set = self.map.entry(number).or_default();
        if set.len() < MAX_PER_HEIGHT {
            let ret = set.insert(uncle);
            if ret {
                self.count += 1;
            }
            ret
        } else {
            false
        }
    }

    /// Returns the number of elements in the container.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns true if the container contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    /// Removing all values.
    pub fn clear(&mut self) {
        self.map.clear();
        self.count = 0;
    }

    /// Returns true if the container contains a value.
    pub fn contains(&self, uncle: &UncleBlockView) -> bool {
        let number: BlockNumber = uncle.header().number();
        self.map
            .get(&number)
            .map(|set| set.contains(uncle))
            .unwrap_or(false)
    }

    /// Gets an iterator over the values of the map, in order by block_number.
    pub fn values(&self) -> impl Iterator<Item = &UncleBlockView> {
        self.map.values().flat_map(HashSet::iter)
    }

    /// Removes uncles from the container by specified uncle's number
    pub fn remove_by_number(&mut self, uncle: &UncleBlockView) -> bool {
        let number: BlockNumber = uncle.header().number();

        if let Entry::Occupied(mut entry) = self.map.entry(number) {
            let set = entry.get_mut();
            if set.remove(uncle) {
                self.count -= 1;
                if set.is_empty() {
                    entry.remove();
                }
                return true;
            }
        }
        false
    }

    /// Get uncles from snapshot and current states.
    // A block B1 is considered to be the uncle of another block B2 if all of the following conditions are met:
    // (1) they are in the same epoch, sharing the same difficulty;
    // (2) height(B2) > height(B1);
    // (3) B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle;
    // and (4) B2 is the first block in its chain to refer to B1.
    pub fn prepare_uncles(
        &mut self,
        snapshot: &Snapshot,
        current_epoch_ext: &EpochExt,
    ) -> Vec<UncleBlockView> {
        let candidate_number = snapshot.tip_number() + 1;
        let epoch_number = current_epoch_ext.number();
        let max_uncles_num = snapshot.consensus().max_uncles_num();
        let mut uncles: Vec<UncleBlockView> = Vec::with_capacity(max_uncles_num);
        let mut removed = Vec::new();

        for uncle in self.values() {
            if uncles.len() == max_uncles_num {
                break;
            }
            let parent_hash = uncle.header().parent_hash();
            let hash = uncle.hash();
            // we should keep candidate util next epoch
            if uncle.compact_target() != current_epoch_ext.compact_target()
                || uncle.epoch().number() != epoch_number
                || snapshot.is_main_chain(&hash)
                || snapshot.is_uncle(&hash)
            {
                // Wrong epoch/target, or already embedded/on the main
                // chain: stale — drop it so it stops occupying the
                // candidate budget.
                removed.push(uncle.clone());
            } else if uncle.number() < candidate_number
                && (uncles.iter().any(|u| u.hash() == parent_hash)
                    || snapshot.is_main_chain(&parent_hash)
                    || snapshot.is_uncle(&parent_hash))
            {
                uncles.push(uncle.clone());
            }
        }

        for r in removed {
            self.remove_by_number(&r);
        }
        uncles
    }
}

impl Default for CandidateUncles {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::core::UncleBlockView;
    use ckb_types::packed;

    fn uncle(number: BlockNumber, seed: u8) -> UncleBlockView {
        ckb_types::core::BlockBuilder::default()
            .number(number)
            .epoch(ckb_types::core::EpochNumberWithFraction::new(0, 0, 1).full_value())
            .parent_hash(packed::Byte32::new([seed; 32]))
            .build()
            .as_uncle()
    }

    /// A full container must still accept an uncle at the *boundary*
    /// height (equal to the lowest stored height) into that height's
    /// existing set when it has room — instead of rejecting it while
    /// evicting the whole lowest set for strictly higher heights.
    #[test]
    fn full_container_accepts_uncle_at_lowest_existing_height() {
        let mut container = CandidateUncles::new();

        // Fill to capacity: height 1 has room left in its set, the others
        // are full. (Test constants: MAX_CANDIDATE_UNCLES=4, MAX_PER_HEIGHT=2.)
        assert!(container.insert(uncle(1, 100)));
        assert!(container.insert(uncle(2, 101)));
        assert!(container.insert(uncle(2, 102)));
        assert!(container.insert(uncle(3, 103)));
        assert_eq!(container.len(), MAX_CANDIDATE_UNCLES);

        // Boundary height with room: accepted into the existing set (the
        // soft cap may be exceeded by one set, bounded by MAX_PER_HEIGHT).
        assert!(
            container.insert(uncle(1, 200)),
            "boundary-height uncle must be accepted into the existing set"
        );
        assert!(
            !container.insert(uncle(1, 201)),
            "but the per-height cap still holds"
        );

        // A strictly lower height is rejected.
        assert!(!container.insert(uncle(0, 202)));

        // A higher height evicts the whole lowest set to make room.
        assert!(container.insert(uncle(4, 203)));
        assert!(
            !container.values().any(|u| u.header().number() == 1),
            "the lowest height set must be evicted for the higher uncle"
        );
    }
}
