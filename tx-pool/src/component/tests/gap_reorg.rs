//! Regression: reorg mine-mode must fully re-evaluate Gap entries.
//!
//! A Gap tx whose short id is no longer in the tip's gap or proposed
//! windows must be demoted to Pending so `get_proposals` can re-package
//! it. Without demotion the tx stays RPC-visible as "pending" (Gap maps
//! to Pending) while never being proposed or committed again.

use crate::component::entry::TxEntry;
use crate::component::pool_map::Status;
use ckb_proposal_table::ProposalView;
use ckb_snapshot::Snapshot;
use ckb_types::core::Capacity;
use ckb_types::packed::ProposalShortId;
use ckb_types::prelude::Unpack;
use ckb_util::LinkedHashSet;
use std::collections::HashSet;
use std::sync::Arc;

use super::harness::{WorkerSet, harness};
use super::util::{MOCK_CYCLES, MOCK_SIZE, build_tx};

fn snapshot_with_proposals(
    base: &Snapshot,
    store: &ckb_test_chain_utils::MockStore,
    gap: HashSet<ProposalShortId>,
    set: HashSet<ProposalShortId>,
) -> Arc<Snapshot> {
    Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        store.store().get_snapshot(),
        ProposalView::new(gap, set),
        base.cloned_consensus(),
    ))
}

fn run_mine_mode_reorg(tx_pool: &mut crate::TxPool, snapshot: Arc<Snapshot>) -> Vec<Status> {
    let outcome = crate::process::reorg::update_tx_pool_for_reorg(
        tx_pool,
        &LinkedHashSet::default(),
        &HashSet::default(),
        HashSet::default(),
        snapshot,
        true,
    );
    outcome
        .notify_events
        .into_iter()
        .map(|(_, status)| status)
        .collect()
}

fn entry_status(tx_pool: &crate::TxPool, id: &ProposalShortId) -> Status {
    tx_pool
        .pool_map
        .get_by_id(id)
        .expect("entry present")
        .status
}

/// Gap short id leaves both windows → demote to Pending and notify.
#[tokio::test]
async fn reorg_demotes_stale_gap_to_pending() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;

    let mut tx_pool = service.pool.tx_pool.write().await;
    // Skip the one-shot onchain reconcile so it does not interfere.
    tx_pool.onchain_reconcile_done = true;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Gap,
        )
        .unwrap();

    // Empty proposal view: the Gap short id is neither gap nor proposed.
    let snapshot =
        snapshot_with_proposals(tx_pool.snapshot(), &store, HashSet::new(), HashSet::new());
    let notifies = run_mine_mode_reorg(&mut tx_pool, snapshot);

    assert_eq!(
        entry_status(&tx_pool, &id),
        Status::Pending,
        "stale Gap must demote to Pending"
    );
    assert!(
        notifies.contains(&Status::Pending),
        "demotion must emit a Pending notify so callers/assembler can react"
    );
    // Critically: get_proposals must see it again.
    let proposals = tx_pool.get_proposals(10, &HashSet::new());
    assert!(
        proposals.contains(&id),
        "demoted Pending must be selectable by get_proposals"
    );
}

/// Gap short id still in the gap window → stay Gap (not re-proposed).
#[tokio::test]
async fn reorg_keeps_gap_when_still_in_gap_window() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Gap,
        )
        .unwrap();

    let mut gap = HashSet::new();
    gap.insert(id.clone());
    let snapshot = snapshot_with_proposals(tx_pool.snapshot(), &store, gap, HashSet::new());
    let notifies = run_mine_mode_reorg(&mut tx_pool, snapshot);

    assert_eq!(
        entry_status(&tx_pool, &id),
        Status::Gap,
        "Gap still in the gap window must not demote"
    );
    assert!(
        !notifies.contains(&Status::Pending),
        "no Pending notify for a still-valid Gap"
    );
    // Gap must remain invisible to get_proposals.
    let proposals = tx_pool.get_proposals(10, &HashSet::new());
    assert!(
        !proposals.contains(&id),
        "Gap must not be re-proposed while still in the gap window"
    );
}

/// Gap short id enters the proposed window → promote to Proposed.
#[tokio::test]
async fn reorg_promotes_gap_to_proposed_when_in_proposed_window() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Gap,
        )
        .unwrap();

    let mut set = HashSet::new();
    set.insert(id.clone());
    let snapshot = snapshot_with_proposals(tx_pool.snapshot(), &store, HashSet::new(), set);
    let notifies = run_mine_mode_reorg(&mut tx_pool, snapshot);

    assert_eq!(
        entry_status(&tx_pool, &id),
        Status::Proposed,
        "Gap in the proposed window must promote to Proposed"
    );
    assert!(
        notifies.contains(&Status::Proposed),
        "promotion must emit a Proposed notify"
    );
}

/// Pending short id enters the gap window → promote to Gap (existing path).
#[tokio::test]
async fn reorg_promotes_pending_to_gap_when_in_gap_window() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();

    let mut gap = HashSet::new();
    gap.insert(id.clone());
    let snapshot = snapshot_with_proposals(tx_pool.snapshot(), &store, gap, HashSet::new());
    run_mine_mode_reorg(&mut tx_pool, snapshot);

    assert_eq!(
        entry_status(&tx_pool, &id),
        Status::Gap,
        "Pending in the gap window must promote to Gap"
    );
}

/// Demotion is not mine-mode-only: a non-mining node must also drop stale
/// Gap so pool status matches the tip proposal windows.
#[tokio::test]
async fn reorg_demotes_stale_gap_even_without_mine_mode() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Gap,
        )
        .unwrap();

    let snapshot =
        snapshot_with_proposals(tx_pool.snapshot(), &store, HashSet::new(), HashSet::new());
    crate::process::reorg::update_tx_pool_for_reorg(
        &mut tx_pool,
        &LinkedHashSet::default(),
        &HashSet::default(),
        HashSet::default(),
        snapshot,
        false, // non-mine mode
    );

    assert_eq!(
        entry_status(&tx_pool, &id),
        Status::Pending,
        "stale Gap must demote even when block assembler is disabled"
    );
}

/// Pending short id with empty windows stays Pending (and proposable).
#[tokio::test]
async fn reorg_leaves_true_pending_proposable() {
    let h = harness(2).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;

    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    tx_pool
        .pool_map
        .add_entry(
            TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
            Status::Pending,
        )
        .unwrap();

    let snapshot =
        snapshot_with_proposals(tx_pool.snapshot(), &store, HashSet::new(), HashSet::new());
    run_mine_mode_reorg(&mut tx_pool, snapshot);

    assert_eq!(entry_status(&tx_pool, &id), Status::Pending);
    let proposals = tx_pool.get_proposals(10, &HashSet::new());
    assert!(proposals.contains(&id));
}
