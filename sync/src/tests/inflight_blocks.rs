use crate::types::InflightBlocks;
use crate::BLOCK_DOWNLOAD_TIMEOUT;
use numext_fixed_hash::{h256, H256};

#[test]
fn inflight_blocks_count() {
    let mut inflight_blocks = InflightBlocks::default();

    // allow 2 peer for one block
    assert!(inflight_blocks.insert(1.into(), h256!("0x1")));
    assert!(inflight_blocks.insert(2.into(), h256!("0x1")));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x1")));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1")));

    assert!(inflight_blocks.insert(1.into(), h256!("0x2")));

    assert_eq!(inflight_blocks.total_inflight_count(), 2); // 0x1 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(&(1.into())), 2); // one block inflight
    assert_eq!(inflight_blocks.peer_inflight_count(&(2.into())), 1);
    assert_eq!(
        inflight_blocks
            .inflight_block_by_peer(&(1.into()))
            .map(|set| set.iter().collect()),
        Some(vec![&h256!("0x1"), &h256!("0x2")])
    );

    //receive block 0x1
    inflight_blocks.remove_by_block(&h256!("0x1"));

    assert_eq!(inflight_blocks.total_inflight_count(), 1); // 0x2
    assert_eq!(inflight_blocks.peer_inflight_count(&(1.into())), 1);
    assert_eq!(inflight_blocks.peer_inflight_count(&(2.into())), 0);
    assert_eq!(
        inflight_blocks
            .inflight_block_by_peer(&(1.into()))
            .map(|set| set.iter().collect()),
        Some(vec![&h256!("0x2")])
    );
}

#[test]
fn inflight_blocks_state() {
    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), h256!("0x1")));
    assert!(inflight_blocks.insert(2.into(), h256!("0x1")));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x1")));

    // peer 1 inflight
    assert!(!inflight_blocks.insert(1.into(), h256!("0x1")));
    assert!(inflight_blocks.insert(1.into(), h256!("0x2")));

    assert!(inflight_blocks.insert(3.into(), h256!("0x3")));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x1"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(1.into()), &(2.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );

    // peer 1 disconnect
    inflight_blocks.remove_by_peer(&(1.into()));
    assert_eq!(inflight_blocks.inflight_block_by_peer(&(1.into())), None);

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x1"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(2.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );
}

#[test]
fn inflight_blocks_timeout() {
    let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
    faketime::enable(&faketime_file);
    let mut inflight_blocks = InflightBlocks::default();

    assert!(inflight_blocks.insert(1.into(), h256!("0x1")));
    assert!(inflight_blocks.insert(1.into(), h256!("0x2")));
    assert!(inflight_blocks.insert(2.into(), h256!("0x2")));
    assert!(inflight_blocks.insert(1.into(), h256!("0x3")));
    assert!(inflight_blocks.insert(2.into(), h256!("0x3")));

    faketime::write_millis(&faketime_file, BLOCK_DOWNLOAD_TIMEOUT + 1).expect("write millis");

    assert!(!inflight_blocks.insert(3.into(), h256!("0x3")));
    assert!(!inflight_blocks.insert(3.into(), h256!("0x2")));
    assert!(inflight_blocks.insert(4.into(), h256!("0x4")));
    assert!(inflight_blocks.insert(1.into(), h256!("0x4")));

    inflight_blocks.prune();
    assert!(inflight_blocks.insert(3.into(), h256!("0x2")));
    assert!(inflight_blocks.insert(3.into(), h256!("0x3")));

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x3"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x2"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(3.into())])
    );

    assert_eq!(
        inflight_blocks
            .inflight_state_by_block(&h256!("0x4"))
            .map(|state| state.peers.iter().collect()),
        Some(vec![&(4.into()), &(1.into())])
    );
}
