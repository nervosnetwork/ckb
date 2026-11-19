use super::{CellLivenessMemo, MemoizedChecker};
use crate::block_assembler::tests::{genesis_snapshot, snapshot_with_consensus};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_types::{
    core::cell::CellChecker,
    packed::{Byte32, OutPoint},
};
use std::sync::Arc;

#[test]
fn cell_liveness_memo_caches_and_invalidates_on_tip_change() {
    let snapshot = genesis_snapshot();
    // The cellbase output of the genesis block is live in the snapshot.
    let live_out_point =
        snapshot.consensus().genesis_block().transactions()[0].output_pts()[0].clone();
    let unknown_out_point = OutPoint::new(Byte32::zero(), 0);

    let mut memo = CellLivenessMemo::default();
    // First lookup populates the memo and matches a direct snapshot query.
    assert_eq!(memo.get_or_load(&snapshot, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);
    // Second lookup is served from the memo without growing it.
    assert_eq!(memo.get_or_load(&snapshot, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);

    // Unknown out-points are memoized as not-live.
    assert_eq!(memo.get_or_load(&snapshot, &unknown_out_point), None);
    assert_eq!(memo.inner.len(), 2);
    assert_eq!(memo.get_or_load(&snapshot, &unknown_out_point), None);
    assert_eq!(memo.inner.len(), 2);

    // A genuinely different chain snapshot clears the memo automatically.
    let genesis = snapshot
        .consensus()
        .genesis_block()
        .as_advanced_builder()
        .timestamp(snapshot.tip_header().timestamp().checked_add(1).unwrap())
        .build();
    let next = snapshot_with_consensus(Arc::new(
        ConsensusBuilder::default().genesis_block(genesis).build(),
    ));
    assert_ne!(snapshot.tip_hash(), next.tip_hash());
    assert_eq!(memo.get_or_load(&next, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);
    assert!(!memo.inner.contains(&unknown_out_point));
    assert_eq!(
        memo.get_or_load(&next, &live_out_point),
        next.is_live(&live_out_point)
    );
}

#[test]
fn cell_liveness_memo_preserves_recent_live_and_unknown_entries_under_churn() {
    let snapshot = genesis_snapshot();
    let live = snapshot.consensus().genesis_block().transactions()[0].output_pts()[0].clone();
    let unknown = OutPoint::new(Byte32::zero(), 0);
    let mut memo = CellLivenessMemo::for_block_bytes(3 * 36);
    assert_eq!(memo.inner.cap(), 3);
    for index in 1..32 {
        assert_eq!(memo.get_or_load(&snapshot, &live), Some(true));
        assert_eq!(memo.get_or_load(&snapshot, &unknown), None);
        let cold = OutPoint::new(Byte32::zero(), index);
        assert_eq!(memo.get_or_load(&snapshot, &cold), None);
        assert_eq!(memo.inner.len(), 3);
        assert_eq!(memo.inner.peek(&live), Some(&Some(true)));
        assert_eq!(memo.inner.peek(&unknown), Some(&None));
        if index > 1 {
            let previous = OutPoint::new(Byte32::zero(), index - 1);
            assert!(!memo.inner.contains(&previous));
        }
    }
}

#[test]
fn cell_liveness_memo_has_a_nonzero_cap_and_detaches_retained_keys() {
    use ckb_types::{bytes::Bytes, prelude::Entity};

    let snapshot = genesis_snapshot();
    let mut memo = CellLivenessMemo::for_block_bytes(0);
    assert_eq!(memo.inner.cap(), 1);
    let backing = Bytes::from(vec![0xacu8; 128 * 1024]);
    let point = OutPoint::new_unchecked(backing.slice(..36));
    assert_eq!(memo.get_or_load(&snapshot, &point), None);
    let (retained, value) = memo.inner.peek_lru().unwrap();
    assert_eq!(retained, &point);
    assert_eq!(value, &None);
    assert_ne!(retained.as_slice().as_ptr(), point.as_slice().as_ptr());
    let next = OutPoint::new(Byte32::zero(), 1);
    assert_eq!(memo.get_or_load(&snapshot, &next), None);
    assert_eq!(memo.inner.len(), 1);
    assert!(!memo.inner.contains(&point));
}

#[test]
fn cell_liveness_memo_keeps_block_overlay_ahead_of_cached_chain_unknown() {
    use ckb_types::{
        core::{TransactionBuilder, cell::TransactionsChecker},
        packed::CellOutput,
    };
    use ckb_util::Mutex;

    let snapshot = genesis_snapshot();
    let producer = TransactionBuilder::default()
        .output(CellOutput::default())
        .build();
    let point = producer.output_pts()[0].clone();
    let mut inner = CellLivenessMemo::for_block_bytes(36);
    assert_eq!(inner.get_or_load(&snapshot, &point), None);
    let memo = Mutex::new(inner);
    let with_producer = TransactionsChecker::new(std::iter::once(&producer));
    let checker = MemoizedChecker {
        transactions_checker: &with_producer,
        snapshot: &snapshot,
        memo: &memo,
    };
    assert_eq!(checker.is_live(&point), Some(true));
    assert_eq!(memo.lock().inner.peek(&point), Some(&None));
    let empty = TransactionsChecker::new(std::iter::empty());
    assert_eq!(
        MemoizedChecker {
            transactions_checker: &empty,
            snapshot: &snapshot,
            memo: &memo,
        }
        .is_live(&point),
        None
    );
}
