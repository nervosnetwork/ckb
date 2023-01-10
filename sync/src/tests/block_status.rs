use std::collections::HashSet;

use ckb_shared::block_status::BlockStatus;

fn all() -> Vec<BlockStatus> {
    vec![
        BlockStatus::UNKNOWN,
        BlockStatus::HEADER_VALID,
        BlockStatus::BLOCK_RECEIVED,
        BlockStatus::BLOCK_STORED,
        BlockStatus::BLOCK_VALID,
        BlockStatus::BLOCK_INVALID,
    ]
}

fn assert_contain(includes: Vec<BlockStatus>, target: BlockStatus) {
    let all = all();
    let excludes = all.iter().filter(|s1| !includes.iter().any(|s2| &s2 == s1));
    includes.iter().for_each(|status| {
        assert!(
            status.contains(target),
            "{:?} should contains {:?}",
            status,
            target
        )
    });
    excludes.into_iter().for_each(|status| {
        assert!(
            !status.contains(target),
            "{:?} should not contains {:?}",
            status,
            target
        )
    });
}

#[test]
fn test_all_different() {
    let set: HashSet<_> = all().into_iter().collect();
    assert_eq!(set.len(), all().len());
}

#[test]
fn test_unknown() {
    assert!(BlockStatus::UNKNOWN.is_empty());
}

#[test]
fn test_header_valid() {
    let target = BlockStatus::HEADER_VALID;
    let includes = vec![
        BlockStatus::HEADER_VALID,
        BlockStatus::BLOCK_RECEIVED,
        BlockStatus::BLOCK_STORED,
        BlockStatus::BLOCK_VALID,
    ];
    assert_contain(includes, target);
}

#[test]
fn test_block_received() {
    let target = BlockStatus::BLOCK_RECEIVED;
    let includes = vec![
        BlockStatus::BLOCK_RECEIVED,
        BlockStatus::BLOCK_STORED,
        BlockStatus::BLOCK_VALID,
    ];
    assert_contain(includes, target);
}

#[test]
fn test_block_stored() {
    let target = BlockStatus::BLOCK_STORED;
    let includes = vec![BlockStatus::BLOCK_STORED, BlockStatus::BLOCK_VALID];
    assert_contain(includes, target);
}

#[test]
fn test_block_valid() {
    let target = BlockStatus::BLOCK_VALID;
    let includes = vec![BlockStatus::BLOCK_VALID];
    assert_contain(includes, target);
}

#[test]
fn test_block_invalid() {
    let target = BlockStatus::BLOCK_INVALID;
    let includes = vec![BlockStatus::BLOCK_INVALID];
    assert_contain(includes, target);
}
