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
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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
    )
    .unwrap();
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

#[tokio::test]
async fn accepted_reorg_recovery_plan_is_parent_first_and_total() {
    let h = harness(1).workers(WorkerSet::None).build();
    let mut tx_pool = h.service.pool.tx_pool.write().await;
    let live = &h.out_points[0];
    let detached_parent = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let child = build_tx(vec![(&detached_parent.hash(), 0)], 1);
    let grandchild = build_tx(vec![(&child.hash(), 0)], 1);
    for tx in [child.clone(), grandchild.clone()] {
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
                Status::Proposed,
            )
            .unwrap();
    }

    let plan = crate::process::reorg::plan_accepted_recovery(
        &tx_pool,
        std::slice::from_ref(&detached_parent),
        2,
    );
    assert_eq!(
        plan.transactions_parent_first()
            .into_iter()
            .map(|tx| tx.hash())
            .collect::<Vec<_>>(),
        vec![child.hash(), grandchild.hash()]
    );
    let removed = crate::process::reorg::apply_accepted_recovery(&mut tx_pool, plan);
    assert_eq!(
        removed
            .iter()
            .map(|removed| removed.entry.transaction().hash())
            .collect::<Vec<_>>(),
        vec![grandchild.hash(), child.hash()],
        "total Apply removes children before parents"
    );
    assert_eq!(tx_pool.pool_map.len(), 0);
}

#[tokio::test]
async fn accepted_reorg_recovery_plan_reports_over_bound_fanout() {
    let h = harness(1).workers(WorkerSet::None).build();
    let mut tx_pool = h.service.pool.tx_pool.write().await;
    let live = &h.out_points[0];
    let detached_parent = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 3);
    for index in 0..3 {
        let child = build_tx(vec![(&detached_parent.hash(), index)], 1);
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(child, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
                Status::Pending,
            )
            .unwrap();
    }

    assert!(matches!(
        crate::process::reorg::plan_accepted_recovery(
            &tx_pool,
            std::slice::from_ref(&detached_parent),
            2,
        ),
        crate::process::reorg::AcceptedRecoveryPlan::OverBound
    ));
}

#[tokio::test]
async fn overlapping_detached_proposals_requeue_each_descendant_once() {
    let h = harness(1).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let mut tx_pool = service.pool.tx_pool.write().await;

    let parent = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let grandchild = build_tx(vec![(&child.hash(), 0)], 1);
    let ids = [
        parent.proposal_short_id(),
        child.proposal_short_id(),
        grandchild.proposal_short_id(),
    ];
    for tx in [parent, child, grandchild] {
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
                Status::Proposed,
            )
            .unwrap();
    }

    // Child-first order forces overlap: the child removes itself and the
    // grandchild, then the parent removes the remaining root. The batch
    // replay must still publish exactly one transition per entry.
    let detached = [ids[1].clone(), ids[0].clone()];
    let mut notify_events = Vec::new();
    tx_pool.remove_by_detached_proposal(detached.iter(), &mut notify_events);
    let notified: HashSet<_> = notify_events
        .iter()
        .map(|(entry, status)| {
            assert_eq!(*status, Status::Pending);
            entry.transaction().hash()
        })
        .collect();
    assert_eq!(notify_events.len(), 3);
    assert_eq!(notified.len(), 3, "no descendant may be notified twice");
    for id in ids {
        assert_eq!(entry_status(&tx_pool, &id), Status::Pending);
    }
}

#[tokio::test]
async fn reorg_publishes_only_the_final_status_after_multiple_transitions() {
    let h = harness(1).workers(WorkerSet::None).build();
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
            Status::Proposed,
        )
        .unwrap();
    let snapshot = snapshot_with_proposals(
        tx_pool.snapshot(),
        &store,
        HashSet::new(),
        HashSet::from([id.clone()]),
    );

    let outcome = crate::process::reorg::update_tx_pool_for_reorg(
        &mut tx_pool,
        &LinkedHashSet::default(),
        &HashSet::default(),
        HashSet::from([id.clone()]),
        snapshot,
        true,
    )
    .unwrap();

    assert!(outcome.reject_events.is_empty());
    assert_eq!(outcome.notify_events.len(), 1);
    assert_eq!(outcome.notify_events[0].1, Status::Proposed);
    assert_eq!(entry_status(&tx_pool, &id), Status::Proposed);
}

#[tokio::test]
async fn reorg_suppresses_intermediate_notify_when_entry_exits_terminally() {
    let h = harness(1).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;
    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;
    tx_pool.expiry = 0;

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let tx_hash = tx.hash();
    let id = tx.proposal_short_id();
    let mut entry = TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE);
    entry.timestamp = 0;
    tx_pool.pool_map.add_entry(entry, Status::Proposed).unwrap();
    let snapshot =
        snapshot_with_proposals(tx_pool.snapshot(), &store, HashSet::new(), HashSet::new());

    let outcome = crate::process::reorg::update_tx_pool_for_reorg(
        &mut tx_pool,
        &LinkedHashSet::default(),
        &HashSet::default(),
        HashSet::from([id.clone()]),
        snapshot,
        true,
    )
    .unwrap();

    assert!(outcome.notify_events.is_empty());
    assert_eq!(outcome.reject_events.len(), 1);
    assert_eq!(outcome.reject_events[0].0.transaction().hash(), tx_hash);
    assert!(tx_pool.pool_map.get_by_id(&id).is_none());
}

/// Reorg expiry is a graph mutation, not a set of independent timestamp
/// removals. A fresh child of an expired parent must leave in the same cascade
/// because the parent's outputs disappear with it.
#[tokio::test]
async fn reorg_expiry_cascades_from_expired_parent_to_fresh_child() {
    let h = harness(1).workers(WorkerSet::None).build();
    let service = h.service;
    let live = h.out_points[0].clone();
    let store = h.store;
    let mut tx_pool = service.pool.tx_pool.write().await;
    tx_pool.onchain_reconcile_done = true;
    tx_pool.expiry = 60_000;

    let parent = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let ids = [parent.proposal_short_id(), child.proposal_short_id()];
    let hashes = HashSet::from([parent.hash(), child.hash()]);
    let mut parent_entry =
        TxEntry::dummy_resolve(parent, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE);
    parent_entry.timestamp = 0;
    let mut child_entry =
        TxEntry::dummy_resolve(child, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE);
    child_entry.timestamp = ckb_systemtime::unix_time_as_millis();
    tx_pool
        .pool_map
        .add_entry(parent_entry, Status::Pending)
        .unwrap();
    tx_pool
        .pool_map
        .add_entry(child_entry, Status::Pending)
        .unwrap();

    let snapshot =
        snapshot_with_proposals(tx_pool.snapshot(), &store, HashSet::new(), HashSet::new());
    let outcome = crate::process::reorg::update_tx_pool_for_reorg(
        &mut tx_pool,
        &LinkedHashSet::default(),
        &HashSet::default(),
        HashSet::new(),
        snapshot,
        true,
    )
    .unwrap();

    assert!(outcome.notify_events.is_empty());
    assert_eq!(outcome.reject_events.len(), 2);
    assert_eq!(
        outcome
            .reject_events
            .iter()
            .map(|(entry, _)| entry.transaction().hash())
            .collect::<HashSet<_>>(),
        hashes
    );
    assert!(
        ids.iter()
            .all(|id| tx_pool.pool_map.get_by_id(id).is_none())
    );
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
    let proposals = tx_pool.package_proposals(10);
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
    let proposals = tx_pool.package_proposals(10);
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
    )
    .unwrap();

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
    let proposals = tx_pool.package_proposals(10);
    assert!(proposals.contains(&id));
}

/// Bug #53: a status-transition failure leaves the entry live in its old
/// status. The reorg delta must remain unacknowledged (production retries the
/// retained head), and replaying it must converge with exactly one real status
/// notification and no false rejection.
#[tokio::test]
async fn reorg_status_transition_failure_has_no_false_reject_and_replay_converges() {
    let mut h = harness(2).workers(WorkerSet::None).build();
    let live = h.out_points[0].clone();

    let pending_calls = Arc::new(AtomicUsize::new(0));
    let reject_calls = Arc::new(AtomicUsize::new(0));
    let mut callbacks = crate::callback::Callbacks::new();
    let pending_calls_cb = Arc::clone(&pending_calls);
    let pool_for_callback = Arc::clone(&h.service.pool.tx_pool);
    callbacks.register_pending(Box::new(move |_| {
        assert!(
            pool_for_callback.try_read().is_ok(),
            "reorg callbacks must publish after the authoritative pool slice"
        );
        pending_calls_cb.fetch_add(1, Ordering::SeqCst);
    }));
    let reject_calls_cb = Arc::clone(&reject_calls);
    callbacks.register_reject(Box::new(move |_, _| {
        reject_calls_cb.fetch_add(1, Ordering::SeqCst);
    }));
    h.service.relay.callbacks = Arc::new(callbacks);

    let tx = build_tx(vec![(&live.tx_hash(), live.index().unpack())], 1);
    let id = tx.proposal_short_id();
    let (snapshot, original_tip) = {
        let mut tx_pool = h.service.pool.tx_pool.write().await;
        let original_tip = tx_pool.snapshot().tip_hash();
        tx_pool.onchain_reconcile_done = true;
        tx_pool
            .pool_map
            .add_entry(
                TxEntry::dummy_resolve(tx, MOCK_CYCLES, Capacity::shannons(1_000), MOCK_SIZE),
                Status::Gap,
            )
            .unwrap();
        tx_pool.fail_next_status_transition = true;
        (
            snapshot_with_proposals(tx_pool.snapshot(), &h.store, HashSet::new(), HashSet::new()),
            original_tip,
        )
    };

    let run = |service: crate::service::TxPoolService, snapshot: Arc<Snapshot>| async move {
        service
            .update_tx_pool_for_reorg(
                Default::default(),
                Default::default(),
                HashSet::new(),
                snapshot,
            )
            .await
    };

    assert!(run(h.service.clone(), Arc::clone(&snapshot)).await.is_err());
    h.service.relay.effects.wait_idle().await;
    let (status_after_failure, tip_after_failure) = {
        let tx_pool = h.service.pool.tx_pool.read().await;
        (entry_status(&tx_pool, &id), tx_pool.snapshot().tip_hash())
    };
    assert_eq!(
        status_after_failure,
        Status::Gap,
        "failed transition must leave the live entry in its old status"
    );
    assert_eq!(
        tip_after_failure, original_tip,
        "a retryable preflight error must not expose the new snapshot before the reorg slice commits"
    );
    assert_eq!(pending_calls.load(Ordering::SeqCst), 0);
    assert_eq!(reject_calls.load(Ordering::SeqCst), 0);

    run(h.service.clone(), snapshot).await.unwrap();
    h.service.relay.effects.wait_idle().await;
    let status_after_replay = {
        let tx_pool = h.service.pool.tx_pool.read().await;
        entry_status(&tx_pool, &id)
    };
    assert_eq!(
        status_after_replay,
        Status::Pending,
        "replaying the authoritative state must converge"
    );
    assert_eq!(pending_calls.load(Ordering::SeqCst), 1);
    assert_eq!(reject_calls.load(Ordering::SeqCst), 0);
}
