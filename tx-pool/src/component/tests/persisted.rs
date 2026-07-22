//! Regression test: the persisted pool file must be topologically ordered.
//!
//! `get_all_txs` iterates the slab-backed entry map in slot order (vacant
//! slots are reused), which is not the insertion order. The reload path
//! replays the file serially without retry, so a child stored before its
//! parent would be dropped as stale (and recorded in recent_reject).
//! `TxPool::save_into_file` therefore sorts parents before children before
//! writing.

use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use ckb_types::core::{Capacity, TransactionView};
use ckb_types::packed::Byte32;
use std::collections::HashMap;

use super::harness::{WorkerSet, harness};
use super::util::{MOCK_CYCLES, MOCK_SIZE, build_tx};

#[tokio::test]
async fn persisted_file_is_topologically_ordered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let service = harness(1).workers(WorkerSet::None).build().service;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.config.persisted_data = tmp.path().join("tx_pool");

    // Several chains with interleaved insertions, plus removals to free
    // early slab slots so later insertions reuse them out of order.
    let insert_chain =
        |tx_pool: &mut crate::TxPool, seed: u8, len: usize| -> Vec<TransactionView> {
            let mut prev = Byte32::new([seed; 32]);
            let mut chain = Vec::new();
            for _ in 0..len {
                let link = build_tx(vec![(&prev, 0)], 1);
                prev = link.hash();
                tx_pool
                    .pool_map
                    .add_entry(
                        TxEntry::dummy_resolve(
                            link.clone(),
                            MOCK_CYCLES,
                            Capacity::shannons(1_000),
                            MOCK_SIZE,
                        ),
                        Status::Pending,
                    )
                    .unwrap();
                chain.push(link);
            }
            chain
        };

    let mut chains = Vec::new();
    for seed in 1..=3u8 {
        chains.push(insert_chain(&mut tx_pool, seed, 6));
    }
    for chain in &chains {
        tx_pool.pool_map.remove_entry(&chain[1].proposal_short_id());
    }
    chains.push(insert_chain(&mut tx_pool, 250, 4));

    tx_pool.save_into_file().expect("save succeeds");
    let loaded = tx_pool.load_from_file().expect("load succeeds");
    assert!(!loaded.is_empty());

    // For every parent/child pair present in the file, the parent must
    // come first.
    let position: HashMap<Byte32, usize> = loaded
        .iter()
        .enumerate()
        .map(|(i, tx)| (tx.hash(), i))
        .collect();
    for (child_pos, tx) in loaded.iter().enumerate() {
        for input in tx.input_pts_iter() {
            if let Some(&parent_pos) = position.get(&input.tx_hash()) {
                assert!(
                    parent_pos < child_pos,
                    "parent must precede child in the persisted file"
                );
            }
        }
    }
}
