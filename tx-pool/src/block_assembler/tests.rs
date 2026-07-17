use ckb_types::core::{BlockBuilder, BlockNumber, EpochNumberWithFraction};

use crate::block_assembler::candidate_uncles::{
    CandidateUncles, MAX_CANDIDATE_UNCLES, MAX_PER_HEIGHT,
};

use super::CellLivenessMemo;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_snapshot::Snapshot;
use ckb_store::attach_block_cell;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{BlockExt, cell::CellChecker},
    packed::{Byte32, OutPoint},
};
use std::sync::Arc;

fn genesis_snapshot() -> Arc<Snapshot> {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    let epoch_ext = consensus.genesis_epoch_ext().clone();
    {
        let db_txn = store.store().begin_transaction();
        let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn.insert_block(genesis).unwrap();
        db_txn.attach_block(genesis).unwrap();
        attach_block_cell(&db_txn, genesis).unwrap();
        db_txn
            .insert_block_epoch_index(&genesis.hash(), &last_block_hash_in_previous_epoch)
            .unwrap();
        db_txn
            .insert_epoch_ext(&last_block_hash_in_previous_epoch, &epoch_ext)
            .unwrap();
        db_txn
            .insert_block_ext(
                &genesis.hash(),
                &BlockExt {
                    received_at: 0,
                    total_difficulty: U256::zero(),
                    total_uncles_count: 0,
                    verified: Some(true),
                    txs_fees: vec![],
                    cycles: None,
                    txs_sizes: None,
                },
            )
            .unwrap();
        db_txn.commit().unwrap();
    }

    Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        epoch_ext,
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

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

    // A tip change clears the memo automatically.
    memo.tip_hash = Some(Byte32::zero());
    assert_eq!(memo.get_or_load(&snapshot, &live_out_point), Some(true));
    assert_eq!(memo.inner.len(), 1);
    assert_eq!(
        memo.get_or_load(&snapshot, &live_out_point),
        snapshot.is_live(&live_out_point)
    );
}

#[test]
fn test_candidate_uncles_basic() {
    let mut candidate_uncles = CandidateUncles::new();
    let block = &BlockBuilder::default().build().as_uncle();
    assert!(candidate_uncles.insert(block.clone()));
    assert_eq!(candidate_uncles.len(), 1);
    // insert duplicate
    assert!(!candidate_uncles.insert(block.clone()));
    assert_eq!(candidate_uncles.len(), 1);

    assert!(candidate_uncles.remove_by_number(block));
    assert_eq!(candidate_uncles.len(), 0);
    assert_eq!(candidate_uncles.map.len(), 0);
}

#[test]
fn test_candidate_uncles_max_size() {
    let mut candidate_uncles = CandidateUncles::new();

    let mut blocks = Vec::new();
    for i in 0..(MAX_CANDIDATE_UNCLES + 3) {
        let number = i as BlockNumber;
        let block = BlockBuilder::default()
            .number(number)
            .epoch(EpochNumberWithFraction::new(
                number / 1000,
                number % 1000,
                10000,
            ))
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
            .timestamp(i as u64)
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
