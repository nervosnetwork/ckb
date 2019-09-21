use ckb_types::{core::BlockNumber, core::UncleBlockView};
use std::collections::{btree_map::Entry, BTreeMap, HashSet};

#[cfg(not(test))]
const MAX_CANDIDATE_UNCLES: usize = 128;
#[cfg(test)]
const MAX_CANDIDATE_UNCLES: usize = 4;

#[cfg(not(test))]
const MAX_PER_HEIGHT: usize = 10;
#[cfg(test)]
const MAX_PER_HEIGHT: usize = 2;

pub struct CandidateUncles {
    pub(crate) map: BTreeMap<BlockNumber, HashSet<UncleBlockView>>,
    count: usize,
}

impl CandidateUncles {
    pub fn new() -> CandidateUncles {
        CandidateUncles {
            map: BTreeMap::new(),
            count: 0,
        }
    }

    pub fn insert(&mut self, uncle: UncleBlockView) -> bool {
        let number: BlockNumber = uncle.header().number();
        if self.count >= MAX_CANDIDATE_UNCLES {
            let first_key = *self.map.keys().next().expect("length checked");
            if number > first_key {
                if let Some(set) = self.map.remove(&first_key) {
                    self.count -= set.len();
                }
            } else {
                return false;
            }
        }

        let set = self.map.entry(number).or_insert_with(HashSet::new);
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

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.count
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.map.clear();
        self.count = 0;
    }

    pub fn values(&self) -> impl Iterator<Item = &UncleBlockView> {
        self.map.values().flat_map(HashSet::iter)
    }

    pub fn remove(&mut self, uncle: &UncleBlockView) -> bool {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::core::BlockBuilder;
    use ckb_types::prelude::*;

    #[test]
    fn test_candidate_uncles_basic() {
        let mut candidate_uncles = CandidateUncles::new();
        let block = &BlockBuilder::default().build().as_uncle();
        assert!(candidate_uncles.insert(block.clone()));
        assert_eq!(candidate_uncles.len(), 1);
        // insert duplicate
        assert!(!candidate_uncles.insert(block.clone()));
        assert_eq!(candidate_uncles.len(), 1);

        assert!(candidate_uncles.remove(&block));
        assert_eq!(candidate_uncles.len(), 0);
        assert_eq!(candidate_uncles.map.len(), 0);
    }

    #[test]
    fn test_candidate_uncles_max_size() {
        let mut candidate_uncles = CandidateUncles::new();

        let mut blocks = Vec::new();
        for i in 0..(MAX_CANDIDATE_UNCLES + 3) {
            let block = BlockBuilder::default()
                .number((i as BlockNumber).pack())
                .build()
                .as_uncle();
            blocks.push(block);
        }

        for block in &blocks {
            candidate_uncles.insert(block.clone());
        }
        let first_key = *candidate_uncles.map.keys().next().unwrap();
        assert_eq!(candidate_uncles.len(), MAX_CANDIDATE_UNCLES);
        assert_eq!(first_key, 3);

        candidate_uncles.clear();
        for block in blocks.iter().rev() {
            candidate_uncles.insert(block.clone());
        }
        let first_key = *candidate_uncles.map.keys().next().unwrap();
        assert_eq!(candidate_uncles.len(), MAX_CANDIDATE_UNCLES);
        assert_eq!(first_key, 3);
    }

    #[test]
    fn test_candidate_uncles_max_per_height() {
        let mut candidate_uncles = CandidateUncles::new();

        let mut blocks = Vec::new();
        for i in 0..(MAX_PER_HEIGHT + 3) {
            let block = BlockBuilder::default()
                .timestamp((i as u64).pack())
                .build()
                .as_uncle();
            blocks.push(block);
        }

        for block in &blocks {
            candidate_uncles.insert(block.clone());
        }
        assert_eq!(candidate_uncles.map.len(), 1);
        assert_eq!(candidate_uncles.len(), MAX_PER_HEIGHT);
    }
}
