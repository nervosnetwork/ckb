use crate::types::InflightBlocks;
use ckb_constant::sync::BLOCK_DOWNLOAD_TIMEOUT;
use ckb_types::BlockNumberAndHash;
use ckb_types::h256;
use std::collections::HashSet;

#[test]
fn inflight_blocks_count() {
    let mut inflight_blocks = InflightBlocks::default();

    // don't allow 2 peer for one block
    assert!(inflight_blocks.insert(2.into(), (1, h256!("0x1").into()).into()));
    assert!(!inflight_blocks.insert(1.into(), (1, h256!("0x1").into()).into()));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), (1, h256!("0x1").into()).into()));

    assert!(inflight_blocks.insert(1.into(), (2, h256!("0x2").into()).into()));

    assert_eq!(inflight_blocks.total_inflight_count(), 2); // 0x1 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(1.into()), 1);
    assert_eq!(inflight_blocks.peer_inflight_count(2.into()), 1); // one block inflight
    assert_eq!(
        inflight_blocks.inflight_block_by_peer(1.into()).cloned(),
        Some(HashSet::from_iter(vec![(2, h256!("0x2").into()).into()]))
    );

    // receive block 0x1
    inflight_blocks.remove_by_block((1, h256!("0x1").into()).into());

    assert_eq!(inflight_blocks.total_inflight_count(), 1); // 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(1.into()), 1);
    assert_eq!(inflight_blocks.peer_inflight_count(2.into()), 0);
    assert_eq!(
        inflight_blocks
            .inflight_block_by_peer(1.into())
            .map(|set| set.iter().collect()),
        Some(vec![&(2, h256!("0x2").into()).into()])
    );
}

#[test]
fn inflight_blocks_state() {
    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), (1, h256!("0x1").into()).into()));
    assert!(!inflight_blocks.insert(2.into(), (1, h256!("0x1").into()).into()));
    assert!(!inflight_blocks.insert(3.into(), (1, h256!("0x1").into()).into()));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), (1, h256!("0x1").into()).into()));
    assert!(inflight_blocks.insert(1.into(), (2, h256!("0x2").into()).into()));

    assert!(inflight_blocks.insert(3.into(), (3, h256!("0x3").into()).into()));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(1, h256!("0x1").into()).into())
            .cloned()
            .map(|state| { state.peer }),
        Some(1.into())
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(3, h256!("0x3").into()).into())
            .map(|state| state.peer),
        Some(3.into())
    );

    // peer 1 disconnect
    inflight_blocks.remove_by_peer(1.into());
    assert_eq!(inflight_blocks.inflight_block_by_peer(1.into()), None);

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(1, h256!("0x1").into()).into())
            .map(|state| state.peer),
        None
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(3, h256!("0x3").into()).into())
            .map(|state| state.peer),
        Some(3.into())
    );
}

#[test]
fn inflight_blocks_timeout() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);
    let mut inflight_blocks = InflightBlocks::default();
    inflight_blocks.protect_num = 0;

    assert!(inflight_blocks.insert(1.into(), (1, h256!("0x1").into()).into()));
    assert!(inflight_blocks.insert(1.into(), (2, h256!("0x2").into()).into()));
    assert!(inflight_blocks.insert(2.into(), (3, h256!("0x3").into()).into()));
    assert!(!inflight_blocks.insert(1.into(), (3, h256!("0x3").into()).into()));
    assert!(inflight_blocks.insert(1.into(), (4, h256!("0x4").into()).into()));
    assert!(inflight_blocks.insert(2.into(), (5, h256!("0x5").into()).into()));
    assert!(!inflight_blocks.insert(2.into(), (5, h256!("0x5").into()).into()));

    _faketime_guard.set_faketime(BLOCK_DOWNLOAD_TIMEOUT + 1);

    assert!(!inflight_blocks.insert(3.into(), (3, h256!("0x3").into()).into()));
    assert!(!inflight_blocks.insert(3.into(), (2, h256!("0x2").into()).into()));
    assert!(inflight_blocks.insert(4.into(), (6, h256!("0x6").into()).into()));
    assert!(inflight_blocks.insert(1.into(), (7, h256!("0x7").into()).into()));

    let peers = inflight_blocks.prune(0);
    assert_eq!(peers, HashSet::from_iter(vec![1.into()]));
    assert!(inflight_blocks.insert(3.into(), (2, h256!("0x2").into()).into()));
    assert!(inflight_blocks.insert(3.into(), (3, h256!("0x3").into()).into()));

    assert_eq!(inflight_blocks.peer_can_fetch_count(2.into()), 32 >> 4);

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(3, h256!("0x3").into()).into())
            .map(|state| state.peer),
        Some(3.into())
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(2, h256!("0x2").into()).into())
            .map(|state| state.peer),
        Some(3.into())
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&(6, h256!("0x6").into()).into())
            .cloned()
            .map(|state| state.peer),
        Some(4.into())
    );
}

#[test]
fn inflight_trace_number_state() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(0);

    let mut inflight_blocks = InflightBlocks::default();
    inflight_blocks.protect_num = 0;

    assert!(inflight_blocks.insert(1.into(), (1, h256!("0x1").into()).into()));
    assert!(inflight_blocks.insert(2.into(), (2, h256!("0x2").into()).into()));
    assert!(inflight_blocks.insert(3.into(), (3, h256!("0x3").into()).into()));
    assert!(inflight_blocks.insert(4.into(), (3, h256!("0x33").into()).into()));
    assert!(inflight_blocks.insert(5.into(), (4, h256!("0x4").into()).into()));
    assert!(inflight_blocks.insert(6.into(), (5, h256!("0x5").into()).into()));
    assert!(inflight_blocks.insert(7.into(), (5, h256!("0x55").into()).into()));

    let list = inflight_blocks.prune(2);
    assert!(list.is_empty());

    assert!(inflight_blocks.trace_number.is_empty());
    assert!(inflight_blocks.restart_number == 0);

    // When 2 + 512 number block request send out
    inflight_blocks.mark_slow_block(2);

    assert_eq!(
        inflight_blocks
            .trace_number
            .keys()
            .cloned()
            .collect::<HashSet<BlockNumberAndHash>>(),
        HashSet::from_iter(vec![
            (1, h256!("0x1").into()).into(),
            (2, h256!("0x2").into()).into(),
            (3, h256!("0x3").into()).into(),
            (3, h256!("0x33").into()).into()
        ])
    );

    _faketime_guard.set_faketime(2000);

    let list = inflight_blocks.prune(2);
    assert!(list.is_empty());

    assert!(inflight_blocks.restart_number == 3);

    assert!(
        inflight_blocks
            .inflight_state_by_block(&(3, h256!("0x3").into()).into())
            .is_none()
    );
    assert!(
        inflight_blocks
            .inflight_state_by_block(&(3, h256!("0x33").into()).into())
            .is_none()
    );

    assert_eq!(inflight_blocks.peer_can_fetch_count(3.into()), 32 >> 1);
    assert_eq!(inflight_blocks.peer_can_fetch_count(4.into()), 32 >> 1);
}
