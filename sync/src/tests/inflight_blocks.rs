use crate::types::InflightBlocks;
use crate::BLOCK_DOWNLOAD_TIMEOUT;
use ckb_types::prelude::*;
use ckb_types::{h256, H256};
use std::collections::HashSet;
use std::iter::FromIterator;

#[test]
fn inflight_blocks_count() {
    let mut inflight_blocks = InflightBlocks::default();

    // allow 2 peer for one block
    assert!(inflight_blocks.insert(1.into(), h256!("0x1").pack()));
    assert!(inflight_blocks.insert(2.into(), h256!("0x1").pack()));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x1").pack()));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1").pack()));

    assert!(inflight_blocks.insert(1.into(), h256!("0x2").pack()));

    assert_eq!(inflight_blocks.total_inflight_count(), 2); // 0x1 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(1.into()), 2);
    assert_eq!(inflight_blocks.peer_inflight_count(2.into()), 1); // one block inflight
    assert_eq!(
        inflight_blocks.inflight_block_by_peer(1.into()).cloned(),
        Some(HashSet::from_iter(vec![
            h256!("0x1").pack(),
            h256!("0x2").pack()
        ]))
    );

    // receive block 0x1
    inflight_blocks.remove_by_block(h256!("0x1").pack());

    assert_eq!(inflight_blocks.total_inflight_count(), 1); // 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(1.into()), 1);
    assert_eq!(inflight_blocks.peer_inflight_count(2.into()), 0);
    assert_eq!(
        inflight_blocks
            .inflight_block_by_peer(1.into())
            .map(|set| set.iter().collect()),
        Some(vec![&h256!("0x2").pack()])
    );
}

#[test]
fn inflight_blocks_state() {
    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), h256!("0x1").pack()));
    assert!(inflight_blocks.insert(2.into(), h256!("0x1").pack()));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x1").pack()));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1").pack()));
    assert!(inflight_blocks.insert(1.into(), h256!("0x2").pack()));

    assert!(inflight_blocks.insert(3.into(), h256!("0x3").pack()));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x1").pack())
            .cloned()
            .map(|state| { state.peers }),
        Some(HashSet::from_iter(vec![1.into(), 2.into()]))
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3").pack())
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );

    // peer 1 disconnect
    inflight_blocks.remove_by_peer(1.into());
    assert_eq!(inflight_blocks.inflight_block_by_peer(1.into()), None);

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x1").pack())
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(2.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3").pack())
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn inflight_blocks_timeout() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), h256!("0x1").pack()));
    assert!(inflight_blocks.insert(1.into(), h256!("0x2").pack()));
    assert!(inflight_blocks.insert(2.into(), h256!("0x2").pack()));
    assert!(inflight_blocks.insert(1.into(), h256!("0x3").pack()));
    assert!(inflight_blocks.insert(2.into(), h256!("0x3").pack()));

    faketime::write_millis(&faketime_file, BLOCK_DOWNLOAD_TIMEOUT + 1).expect("write millis");

    assert!(!inflight_blocks.insert(3.into(), h256!("0x3").pack()));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x2").pack()));
    assert!(inflight_blocks.insert(4.into(), h256!("0x4").pack()));
    assert!(inflight_blocks.insert(1.into(), h256!("0x4").pack()));

    inflight_blocks.prune();
    assert!(inflight_blocks.insert(3.into(), h256!("0x2").pack()));
    assert!(inflight_blocks.insert(3.into(), h256!("0x3").pack()));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3").pack())
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x2").pack())
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x4").pack())
            .cloned()
            .map(|state| { state.peers }),
        Some(HashSet::from_iter(vec![1.into(), 4.into()]))
    );
}
