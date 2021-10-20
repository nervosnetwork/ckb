use ckb_types::{core::BlockNumber, core::UncleBlockView};
use std::collections::{btree_map::Entry, BTreeMap, HashSet};

#[cfg(not(test))]
const MAX_CANDIDATE_UNCLES: usize = 128;
#[cfg(test)]
pub(crate) const MAX_CANDIDATE_UNCLES: usize = 4;

#[cfg(not(test))]
const MAX_PER_HEIGHT: usize = 10;
#[cfg(test)]
pub(crate) const MAX_PER_HEIGHT: usize = 2;

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
