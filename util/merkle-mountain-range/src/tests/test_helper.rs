use super::{MergeNumberHash, NumberHash};
use crate::{
    helper::{get_peaks, pos_height_in_tree},
    leaf_index_to_mmr_size, leaf_index_to_pos,
    util::MemStore,
    MMR,
};
use lazy_static::lazy_static;
use proptest::prelude::*;

lazy_static! {
    /// Positions of 0..100_000 elem
    static ref INDEX_TO_POS: Vec<u64> = {
        let store = MemStore::default();
        let mut mmr = MMR::<_,MergeNumberHash,_>::new(0, &store);
        (0u32..100_000)
            .map(|i| mmr.push(NumberHash::from(i)).unwrap())
            .collect()
    };
    /// mmr size when 0..100_000 elem
    static ref INDEX_TO_MMR_SIZE: Vec<u64> = {
        let store = MemStore::default();
        let mut mmr = MMR::<_,MergeNumberHash,_>::new(0, &store);
        (0u32..100_000)
            .map(|i| {
                mmr.push(NumberHash::from(i)).unwrap();
                mmr.mmr_size()
            })
            .collect()
    };
}

#[test]
fn test_leaf_index_to_pos() {
    assert_eq!(leaf_index_to_pos(0), 0);
    assert_eq!(leaf_index_to_pos(1), 1);
    assert_eq!(leaf_index_to_pos(2), 3);
}

#[test]
fn test_leaf_index_to_mmr_size() {
    assert_eq!(leaf_index_to_mmr_size(0), 1);
    assert_eq!(leaf_index_to_mmr_size(1), 3);
    assert_eq!(leaf_index_to_mmr_size(2), 4);
}

#[test]
fn test_pos_height_in_tree() {
    assert_eq!(pos_height_in_tree(0), 0);
    assert_eq!(pos_height_in_tree(1), 0);
    assert_eq!(pos_height_in_tree(2), 1);
    assert_eq!(pos_height_in_tree(3), 0);
    assert_eq!(pos_height_in_tree(4), 0);
    assert_eq!(pos_height_in_tree(6), 2);
    assert_eq!(pos_height_in_tree(7), 0);
}

#[test]
fn test_get_peaks() {
    assert_eq!(get_peaks(0), vec![0]);
    assert_eq!(get_peaks(1), vec![0]);
    assert_eq!(get_peaks(2), vec![0]);
    assert_eq!(get_peaks(3), vec![2]);
    assert_eq!(get_peaks(4), vec![2, 3]);
    assert_eq!(get_peaks(5), vec![2, 3]);
    assert_eq!(get_peaks(6), vec![2, 5]);
    assert_eq!(get_peaks(7), vec![6]);
    assert_eq!(get_peaks(19), vec![14, 17, 18]);
}

proptest! {
    #[test]
    fn test_leaf_index_to_pos_randomly(index in 0..INDEX_TO_POS.len()) {
        let pos = leaf_index_to_pos(index as u64);
        assert_eq!(pos, INDEX_TO_POS[index]);
    }

    #[test]
    fn test_leaf_index_to_mmr_size_randomly(index in 0..INDEX_TO_MMR_SIZE.len()) {
        assert_eq!(leaf_index_to_mmr_size(index as u64), INDEX_TO_MMR_SIZE[index]);
    }
}
