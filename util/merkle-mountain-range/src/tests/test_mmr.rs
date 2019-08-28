use crate::{
    leaf_index_to_pos,
    tests_util::{MemStore, NumberHash},
    MMRBatch, MMR,
};
use faster_hex::hex_string;
use lazy_static::lazy_static;
use proptest::prelude::*;
use std::convert::TryFrom;

fn test_mmr(count: u32, proof_elem: u32) {
    let store = MemStore::default();
    let mut batch = MMRBatch::new(store);
    let mut mmr = MMR::new(0, &mut batch);
    let positions: Vec<u64> = (0u32..count)
        .map(|i| mmr.push(NumberHash::try_from(i).unwrap()).unwrap())
        .collect();
    let root = mmr.get_root().expect("get root").unwrap();
    let proof = mmr
        .gen_proof(positions[proof_elem as usize])
        .expect("gen proof");
    batch.commit().expect("write changes");
    let result = proof
        .verify(
            root,
            positions[proof_elem as usize],
            NumberHash::try_from(proof_elem).unwrap(),
        )
        .unwrap();
    assert!(result);
}

#[test]
fn test_mmr_root() {
    let store = MemStore::default();
    let mut batch = MMRBatch::new(store);
    let mut mmr = MMR::new(0, &mut batch);
    (0u32..11).for_each(|i| {
        mmr.push(NumberHash::try_from(i).unwrap()).unwrap();
    });
    let root = mmr.get_root().expect("get root").unwrap();
    let hex_root = hex_string(&root.0).unwrap();
    assert_eq!(
        "d4aa7a8acce692f046d3b968650723b627b1a0431a659f190823a3bf4c918f0b",
        hex_root
    );
}

#[test]
fn test_mmr_3_peaks() {
    test_mmr(11, 5);
}

#[test]
fn test_mmr_2_peaks() {
    test_mmr(10, 5);
}

#[test]
fn test_mmr_1_peak() {
    test_mmr(8, 5);
}

#[test]
fn test_mmr_first_elem_proof() {
    test_mmr(11, 0);
}

#[test]
fn test_mmr_last_elem_proof() {
    test_mmr(11, 10);
}

#[test]
fn test_mmr_1_elem() {
    test_mmr(1, 0);
}

#[test]
fn test_mmr_2_elems() {
    test_mmr(2, 0);
    test_mmr(2, 1);
}

prop_compose! {
    fn count_elem(count: u32)
                (elem in 0..count)
                -> (u32, u32) {
                    (count, elem)
    }
}
lazy_static! {
    static ref POSITIONS: Vec<u64> = {
        let store = MemStore::default();
        let mut batch = MMRBatch::new(store);
        let mut mmr = MMR::new(0, &mut batch);
        (0u32..100_000)
            .map(|i| mmr.push(NumberHash::try_from(i).unwrap()).unwrap())
            .collect()
    };
}

proptest! {
    #[test]
    fn test_random_mmr((count , elem) in count_elem(500)) {
        test_mmr(count, elem);
    }

    #[test]
    fn test_leaf_index_to_pos(index in 0..POSITIONS.len()) {
        let pos = leaf_index_to_pos(index as u64);
        assert_eq!(pos, POSITIONS[index]);
    }
}
