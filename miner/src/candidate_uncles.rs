use ckb_core::uncle::UncleBlock;
use ckb_core::BlockNumber;
use std::collections::{btree_map::Entry, BTreeMap, HashSet};
use std::sync::Arc;

#[cfg(not(test))]
const MAX_CANDIDATE_UNCLES: usize = 128;
#[cfg(test)]
const MAX_CANDIDATE_UNCLES: usize = 4;

#[cfg(not(test))]
const MAX_PER_HEIGHT: usize = 10;
#[cfg(test)]
const MAX_PER_HEIGHT: usize = 2;

pub struct CandidateUncles {
    pub(in crate::candidate_uncles) map: BTreeMap<BlockNumber, HashSet<Arc<UncleBlock>>>,
    count: usize,
}

impl CandidateUncles {
    pub fn new() -> CandidateUncles {
        CandidateUncles {
            map: BTreeMap::new(),
            count: 0,
        }
    }

    pub fn insert(&mut self, uncle: Arc<UncleBlock>) -> bool {
        let number = uncle.header().number();
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

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn values(&self) -> impl Iterator<Item = &Arc<UncleBlock>> {
        self.map.values().flat_map(HashSet::iter)
    }

    pub fn remove(&mut self, uncle: &Arc<UncleBlock>) -> bool {
        let number = uncle.header().number();

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

    pub fn clear(&mut self) {
        self.map.clear();
        self.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::HeaderBuilder;

    #[test]
    fn test_candidate_uncles_basic() {
        let mut candidate_uncles = CandidateUncles::new();
        let block = &BlockBuilder::default().build();
        assert!(candidate_uncles.insert(Arc::new(block.into())));
        assert_eq!(candidate_uncles.len(), 1);
        // insert duplicate
        assert!(!candidate_uncles.insert(Arc::new(block.into())));
        assert_eq!(candidate_uncles.len(), 1);

        assert!(candidate_uncles.remove(&Arc::new(block.into())));
        assert_eq!(candidate_uncles.len(), 0);
        assert_eq!(candidate_uncles.map.len(), 0);
    }

    #[test]
    fn test_candidate_uncles_max_size() {
        let mut candidate_uncles = CandidateUncles::new();

        let mut blocks = Vec::new();
        for i in 0..(MAX_CANDIDATE_UNCLES + 3) {
            let block = BlockBuilder::from_header_builder(
                HeaderBuilder::default().number(i as BlockNumber),
            )
            .build();
            blocks.push(block);
        }

        for block in &blocks {
            candidate_uncles.insert(Arc::new(block.into()));
        }
        let first_key = *candidate_uncles.map.keys().next().unwrap();
        assert_eq!(candidate_uncles.len(), MAX_CANDIDATE_UNCLES);
        assert_eq!(first_key, 3);

        candidate_uncles.clear();
        for block in blocks.iter().rev() {
            candidate_uncles.insert(Arc::new(block.into()));
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
            let block =
                BlockBuilder::from_header_builder(HeaderBuilder::default().timestamp(i as u64))
                    .build();
            blocks.push(block);
        }

        for block in &blocks {
            candidate_uncles.insert(Arc::new(block.into()));
        }
        assert_eq!(candidate_uncles.map.len(), 1);
        assert_eq!(candidate_uncles.len(), MAX_PER_HEIGHT);
    }
}
