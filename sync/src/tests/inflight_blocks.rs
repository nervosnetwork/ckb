use crate::types::InflightBlocks;
use crate::BLOCK_DOWNLOAD_TIMEOUT;
use ckb_types::prelude::*;
use ckb_types::{h256, H256};
use std::collections::HashSet;
use std::iter::FromIterator;

#[test]
fn inflight_blocks_count() {
    let mut inflight_blocks = InflightBlocks::default();

    // don't allow 2 peer for one block
    assert!(inflight_blocks.insert(2.into(), h256!("0x1").pack(), 1));
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1").pack(), 1));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1").pack(), 1));

    assert!(inflight_blocks.insert(1.into(), h256!("0x2").pack(), 2));

    assert_eq!(inflight_blocks.total_inflight_count(), 2); // 0x1 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(1.into()), 1);
    assert_eq!(inflight_blocks.peer_inflight_count(2.into()), 1); // one block inflight
    assert_eq!(
        inflight_blocks.inflight_block_by_peer(1.into()).cloned(),
        Some(HashSet::from_iter(vec![h256!("0x2").pack()]))
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

    assert!(inflight_blocks.insert(1.into(), h256!("0x1").pack(), 1));
    assert!(!inflight_blocks.insert(2.into(), h256!("0x1").pack(), 1));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x1").pack(), 1));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1").pack(), 1));
    assert!(inflight_blocks.insert(1.into(), h256!("0x2").pack(), 2));

    assert!(inflight_blocks.insert(3.into(), h256!("0x3").pack(), 3));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x1").pack())
            .cloned()
            .map(|state| { state.peer }),
        Some(1.into())
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3").pack())
            .map(|state| state.peer),
        Some(3.into())
    );

    // peer 1 disconnect
    inflight_blocks.remove_by_peer(1.into());
    assert_eq!(inflight_blocks.inflight_block_by_peer(1.into()), None);

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x1").pack())
            .map(|state| state.peer),
        None
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3").pack())
            .map(|state| state.peer),
        Some(3.into())
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn inflight_blocks_timeout() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), h256!("0x1").pack(), 1));
    assert!(inflight_blocks.insert(1.into(), h256!("0x2").pack(), 2));
    assert!(inflight_blocks.insert(2.into(), h256!("0x3").pack(), 3));
    assert!(!inflight_blocks.insert(1.into(), h256!("0x3").pack(), 3));
    assert!(inflight_blocks.insert(1.into(), h256!("0x4").pack(), 4));
    assert!(inflight_blocks.insert(2.into(), h256!("0x5").pack(), 5));
    assert!(!inflight_blocks.insert(2.into(), h256!("0x5").pack(), 5));

    faketime::write_millis(&faketime_file, BLOCK_DOWNLOAD_TIMEOUT + 1).expect("write millis");

    assert!(!inflight_blocks.insert(3.into(), h256!("0x3").pack(), 3));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x2").pack(), 2));
    assert!(inflight_blocks.insert(4.into(), h256!("0x6").pack(), 6));
    assert!(inflight_blocks.insert(1.into(), h256!("0x7").pack(), 7));

    let peers = inflight_blocks.prune(0);
    assert_eq!(peers, HashSet::from_iter(vec![1.into(), 2.into()]));
    assert!(inflight_blocks.insert(3.into(), h256!("0x2").pack(), 2));
    assert!(inflight_blocks.insert(3.into(), h256!("0x3").pack(), 3));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3").pack())
            .map(|state| state.peer),
        Some(3.into())
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x2").pack())
            .map(|state| state.peer),
        Some(3.into())
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x6").pack())
            .cloned()
            .map(|state| state.peer),
        Some(4.into())
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn inflight_trace_number_state() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);

    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), h256!("0x1").pack(), 1));
    assert!(inflight_blocks.insert(2.into(), h256!("0x2").pack(), 2));
    assert!(inflight_blocks.insert(3.into(), h256!("0x3").pack(), 3));
    assert!(inflight_blocks.insert(4.into(), h256!("0x33").pack(), 3));
    assert!(inflight_blocks.insert(5.into(), h256!("0x4").pack(), 4));
    assert!(inflight_blocks.insert(6.into(), h256!("0x5").pack(), 5));
    assert!(inflight_blocks.insert(7.into(), h256!("0x55").pack(), 5));

    let list = inflight_blocks.prune(2);
    assert!(list.is_empty());

    let (next_number, (blocks, time)) = inflight_blocks.trace_number.iter().next().unwrap();

    assert_eq!(next_number, &3);
    assert_eq!(
        blocks,
        &HashSet::from_iter(vec![h256!("0x3").pack(), h256!("0x33").pack()])
    );
    assert!(time.is_none());

    // When an orphan block is inserted
    {
        if let Some((_, time)) = inflight_blocks.trace_number.get_mut(&3) {
            *time = Some(faketime::unix_time_as_millis())
        }
    }

    faketime::write_millis(&faketime_file, 2000).expect("write millis");

    let list = inflight_blocks.prune(2);
    assert!(list.is_empty());

    let (next_number, (blocks, time)) = inflight_blocks.trace_number.iter().next().unwrap();

    assert_eq!(next_number, &4);
    assert_eq!(blocks, &HashSet::from_iter(vec![h256!("0x4").pack()]));
    assert!(time.is_none());

    assert!(inflight_blocks
        .inflight_state_by_block(&h256!("0x3").pack())
        .is_none());
    assert!(inflight_blocks
        .inflight_state_by_block(&h256!("0x33").pack())
        .is_none());

    assert_eq!(inflight_blocks.peer_can_fetch_count(3.into()), 8);
    assert_eq!(inflight_blocks.peer_can_fetch_count(4.into()), 8);
}
