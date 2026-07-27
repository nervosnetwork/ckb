//! Regression test for the one-shot post-startup reconcile.
//!
//! Reorg notifications are skipped while the node is in its startup reload,
//! so `remove_onchain_entries` runs once on the first reorg (fresh snapshot)
//! to sweep entries committed on-chain and zombies whose inputs can never
//! resolve — and is then gated off: the full scan is too expensive to
//! repeat on every block.

use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use ckb_types::core::Capacity;
use ckb_types::packed::Byte32;
use ckb_types::prelude::Unpack;
use ckb_util::LinkedHashSet;
use std::collections::HashSet;

use super::support::{WorkerSet, harness};
use crate::test_support::{MOCK_CYCLES, MOCK_SIZE, build_tx, build_tx_with_header_dep};

#[tokio::test]
async fn onchain_reconcile_runs_once_and_sweeps_zombies() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live_outpoint = h.out_points[0].clone();
    let header_dep_outpoint = h.out_points[1].clone();

    let mut tx_pool = service.pool.tx_pool.write().await;

    // Healthy entry: spends a live genesis cell and references a header on
    // the active main chain. The header check must not turn the startup
    // sweep into a blanket removal of transactions with header deps.
    let active_header = tx_pool.snapshot().consensus().genesis_block().hash();
    let healthy = build_tx_with_header_dep(
        vec![(&live_outpoint.tx_hash(), live_outpoint.index().unpack())],
        vec![active_header],
        1,
    );
    let healthy_id = healthy.proposal_short_id();
    // Zombie entry: its input is neither in-pool nor live on-chain.
    let zombie = build_tx(vec![(&Byte32::new([31u8; 32]), 0)], 1);
    let zombie_id = zombie.proposal_short_id();
    // Zombie child: spends the zombie's output; the zombie is still in-pool
    // at collection time, so it must be taken down by the cascade.
    let zombie_child = build_tx(vec![(&zombie.hash(), 0)], 1);
    let zombie_child_id = zombie_child.proposal_short_id();

    // Header zombie: its input remains live, but its header dependency is
    // absent from the active main chain (as happens when startup skips the
    // reorg which detached that header). Its descendant must be removed too.
    let header_zombie = build_tx_with_header_dep(
        vec![(
            &header_dep_outpoint.tx_hash(),
            header_dep_outpoint.index().unpack(),
        )],
        vec![Byte32::new([33u8; 32])],
        1,
    );
    let header_zombie_id = header_zombie.proposal_short_id();
    let header_zombie_child = build_tx(vec![(&header_zombie.hash(), 0)], 1);
    let header_zombie_child_id = header_zombie_child.proposal_short_id();

    for tx in [
        healthy,
        zombie,
        zombie_child,
        header_zombie,
        header_zombie_child,
    ] {
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
                Status::Pending,
            )
            .unwrap();
    }

    let run_reorg = |tx_pool: &mut crate::TxPool| {
        let snapshot = tx_pool.cloned_snapshot();
        crate::process::reorg::update_tx_pool_for_reorg(
            tx_pool,
            &LinkedHashSet::default(),
            &HashSet::default(),
            HashSet::default(),
            snapshot,
            false,
        )
        .unwrap()
    };

    // First reorg: the reconcile runs.
    run_reorg(&mut tx_pool);
    assert!(tx_pool.onchain_reconcile_done);
    assert!(
        tx_pool.pool_map.get_by_id(&healthy_id).is_some(),
        "healthy entry must survive the reconcile"
    );
    assert!(
        tx_pool.pool_map.get_by_id(&zombie_id).is_none(),
        "zombie entry must be swept"
    );
    assert!(
        tx_pool.pool_map.get_by_id(&zombie_child_id).is_none(),
        "zombie child must be cascaded"
    );
    assert!(
        tx_pool.pool_map.get_by_id(&header_zombie_id).is_none(),
        "entry with a detached header dependency must be swept"
    );
    assert!(
        tx_pool
            .pool_map
            .get_by_id(&header_zombie_child_id)
            .is_none(),
        "detached-header descendant must be cascaded"
    );

    // Later reorgs: the reconcile is gated off and must not sweep again.
    let zombie2 = build_tx(vec![(&Byte32::new([32u8; 32]), 0)], 1);
    let zombie2_id = zombie2.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(zombie2, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();
    run_reorg(&mut tx_pool);
    assert!(
        tx_pool.pool_map.get_by_id(&zombie2_id).is_some(),
        "the reconcile must run only once"
    );
}
