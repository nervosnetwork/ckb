use crate::relayer::block_uncles_verifier::BlockUnclesVerifier;
use crate::{Status, StatusCode};
use ckb_types::{
    core::BlockBuilder,
    packed::{CompactBlock, CompactBlockBuilder},
    prelude::*,
};

fn build_compact_block_with_uncles() -> (CompactBlock, Vec<ckb_types::core::UncleBlockView>) {
    let uncles = vec![
        BlockBuilder::default().nonce(1u128).build().as_uncle(),
        BlockBuilder::default().nonce(2u128).build().as_uncle(),
    ];
    let uncle_hashes = uncles.iter().map(|uncle| uncle.hash()).collect::<Vec<_>>();
    let block = CompactBlockBuilder::default().uncles(uncle_hashes).build();
    (block, uncles)
}

#[test]
fn test_invalid_len() {
    let (block, uncles) = build_compact_block_with_uncles();

    assert_eq!(
        BlockUnclesVerifier::verify(&block, &[0, 1], &uncles[..1]),
        StatusCode::BlockUnclesLengthIsUnmatchedWithPendingCompactBlock.into(),
    );
}

#[test]
fn test_unmatched_hash() {
    let (block, _) = build_compact_block_with_uncles();
    let wrong_uncle = BlockBuilder::default().nonce(3u128).build().as_uncle();

    assert_eq!(
        BlockUnclesVerifier::verify(&block, &[0], &[wrong_uncle]),
        StatusCode::BlockUnclesAreUnmatchedWithPendingCompactBlock.into(),
    );
}

#[test]
fn test_ok() {
    let (block, uncles) = build_compact_block_with_uncles();

    assert_eq!(
        BlockUnclesVerifier::verify(&block, &[0, 1], &uncles),
        Status::ok(),
    );
}
