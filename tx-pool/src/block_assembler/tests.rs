use ckb_types::{
    core::{BlockBuilder, BlockNumber},
    prelude::*,
};

use crate::block_assembler::candidate_uncles::{
    CandidateUncles, MAX_CANDIDATE_UNCLES, MAX_PER_HEIGHT,
};

#[test]
fn test_candidate_uncles_basic() {
    let mut candidate_uncles = CandidateUncles::new();
    let block = &BlockBuilder::default().build().as_uncle();
    assert!(candidate_uncles.insert(block.clone()));
    assert_eq!(candidate_uncles.len(), 1);
    // insert duplicate
    assert!(!candidate_uncles.insert(block.clone()));
    assert_eq!(candidate_uncles.len(), 1);

    assert!(candidate_uncles.remove_by_number(&block));
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
