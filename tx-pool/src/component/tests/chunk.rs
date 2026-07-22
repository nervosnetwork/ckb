use crate::callback::Callbacks;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::pool_map::Status;
use crate::component::tests::util::{TEST_MAX_VERIFY_QUEUE_TX_SIZE, build_tx};
use crate::component::verify_queue::VerifyQueue;
use crate::component::waiting_room::WaitingRoom;
use crate::pool::TxPool;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::{TxPoolService, TxVerificationResult};
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_db::RocksDB;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::SessionId;
use ckb_script::ChunkCommand;
use ckb_snapshot::Snapshot;
use ckb_store::ChainDB;
use ckb_types::H256;
use ckb_types::U256;
use ckb_types::bytes::Bytes;
use ckb_types::core::{
    Capacity, FeeRate, TransactionBuilder, cell::ResolvedTransaction, tx_pool::Reject,
};
use ckb_types::packed::Byte32;
use ckb_types::prelude::Pack;
use ckb_verification::cache::init_cache;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::select;
use tokio::sync::watch;
use tokio::sync::{RwLock, mpsc};
use tokio::time::sleep;

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
// Columns opened for the throwaway test store. `Snapshot::transaction_exists`
// reads COLUMN_TRANSACTION_INFO ("5"), which the orphan availability checks
// (handle_missing_input_orphan / process_orphan_tx) rely on, so the store
// must expose at least columns "0"..="5".
const UNUSED_SNAPSHOT_COLUMNS: u32 = 6;

fn test_snapshot() -> Arc<Snapshot> {
    use std::sync::OnceLock;
    static SNAPSHOT: OnceLock<Arc<Snapshot>> = OnceLock::new();
    Arc::<Snapshot>::clone(
        SNAPSHOT.get_or_init(|| snapshot(Arc::new(ConsensusBuilder::default().build()))),
    )
}

fn dummy_resolved_tx(
    tx: ckb_types::core::TransactionView,
    source: crate::tx_source::TxSource,
) -> ResolvedTx {
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    ResolvedTx {
        tx: tx.clone(),
        rtx,
        status: Status::Pending,
        fee: Capacity::zero(),
        tx_size: tx.data().serialized_size_in_block(),
        pre_resolve_tip: Default::default(),
        snapshot: test_snapshot(),
        source,
    }
}
#[tokio::test]
async fn verify_queue_popped_tx_stays_visible_until_finish() {
    let tx = TransactionBuilder::default().build();
    let id = tx.proposal_short_id();
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx.clone(), TxSource::Local))
            .unwrap()
    );

    let popped = queue.pop_front(false).expect("tx pops");
    assert_eq!(popped.tx.hash(), tx.hash());
    // No longer queued, but still in flight while the worker verifies it.
    assert!(!queue.contains_key(&id));
    assert!(queue.contains_or_active(&id));
    assert_eq!(
        queue.get_active_tx(&id).map(|r| r.tx.hash()),
        Some(tx.hash())
    );

    queue.finish(&id);
    assert!(!queue.contains_or_active(&id));
    assert!(queue.get_active_tx(&id).is_none());
}

#[tokio::test]
async fn verify_queue_basic() {
    let tx = TransactionBuilder::default().build();
    let entry = dummy_resolved_tx(tx.clone(), crate::tx_source::TxSource::Local);
    let tx2 = build_tx(vec![(&tx.hash(), 0)], 1);

    let id = tx.proposal_short_id();
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );
    let queue_rx = queue.subscribe();
    let count = tokio::spawn(async move {
        let mut count = 0;
        loop {
            select! {
                _ = queue_rx.notified() => {
                    count += 1;
                }
                _ = exit_rx.changed() => {
                    break;
                }
            }
        }
        count
    });

    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx.clone(),
                crate::tx_source::TxSource::Local
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        !queue
            .add_tx(dummy_resolved_tx(
                tx.clone(),
                crate::tx_source::TxSource::Local
            ))
            .unwrap()
    );

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));

    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx.clone(),
                crate::tx_source::TxSource::Local
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));

    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx.clone(),
                crate::tx_source::TxSource::Local
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx2.clone(),
                crate::tx_source::TxSource::Local
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    exit_tx.send(()).unwrap();
    let counts = count.await.unwrap();
    assert_eq!(counts, 4);

    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx);

    assert!(!queue.is_empty());
    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx2);

    assert!(queue.is_empty());

    queue.clear();
    assert!(!queue.contains_key(&id));
}

#[tokio::test]
async fn test_verify_different_cycles() {
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );
    let queue_rx = queue.subscribe();
    let count = tokio::spawn(async move {
        let mut count = 0;
        loop {
            select! {
                _ = queue_rx.notified() => {
                    count += 1;
                }
                _ = exit_rx.changed() => {
                    break;
                }
            }
        }
        count
    });

    let tx0 = build_tx(vec![(&H256([0; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx0.clone(),
                crate::tx_source::TxSource::Remote {
                    cycles: 1001,
                    peer: SessionId::default(),
                },
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx1 = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx1.clone(),
                crate::tx_source::TxSource::Remote {
                    cycles: MAX_TX_VERIFY_CYCLES + 1,
                    peer: SessionId::default(),
                },
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx2 = build_tx(vec![(&H256([2; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx2.clone(),
                crate::tx_source::TxSource::Remote {
                    cycles: 1001,
                    peer: SessionId::default(),
                },
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by time (tx1, tx2)

    let tx3 = build_tx(vec![(&H256([3; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx3.clone(),
                crate::tx_source::TxSource::Remote {
                    cycles: 1001,
                    peer: SessionId::default(),
                },
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx_size_sum = [&tx0, &tx1, &tx2, &tx3]
        .iter()
        .map(|tx| tx.data().serialized_size_in_block())
        .sum::<usize>();

    assert_eq!(queue.total_tx_size(), tx_size_sum);

    let tx_4_proposal = build_tx(vec![(&H256([4; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx_4_proposal.clone(),
                crate::tx_source::TxSource::Proposal,
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    // first should pop the proposal tx
    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx_4_proposal);

    // tx0 should be the first tx in the queue
    let cur = queue.pop_front(true);
    assert_eq!(cur.unwrap().tx, tx0);

    let cur = queue.pop_front(true);
    assert_eq!(cur.unwrap().tx, tx2);

    let cur = queue.pop_front(true);
    assert_eq!(cur.unwrap().tx, tx3);

    // now there is no small cycle tx
    let cur = queue.pop_front(true);
    assert!(cur.is_none());

    // pop the tx with the large cycle
    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx1);

    let cur = queue.pop_front(false);
    assert!(cur.is_none());

    exit_tx.send(()).unwrap();
    let counts = count.await.unwrap();
    assert_eq!(counts, 5);
    assert_eq!(queue.total_tx_size(), 0);
}

#[tokio::test]
async fn verify_queue_renotify_does_not_store_permit() {
    let queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );
    let queue_rx = queue.subscribe();

    queue.re_notify();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), queue_rx.notified())
            .await
            .is_err()
    );

    let queue_rx = queue.subscribe();
    let waiter = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_millis(500), queue_rx.notified())
            .await
            .is_ok()
    });
    sleep(std::time::Duration::from_millis(10)).await;

    queue.re_notify();
    assert!(waiter.await.unwrap());
}

#[tokio::test]
async fn verify_queue_pops_proposals_by_arrival_order() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );
    let tx0 = build_tx(vec![(&H256([0; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx0.clone(),
                crate::tx_source::TxSource::Remote {
                    cycles: 1001,
                    peer: SessionId::default(),
                },
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx1_proposal = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx1_proposal.clone(),
                crate::tx_source::TxSource::Proposal,
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx2_proposal = build_tx(vec![(&H256([2; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx2_proposal.clone(),
                crate::tx_source::TxSource::Proposal,
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx3 = build_tx(vec![(&H256([3; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx3.clone(),
                crate::tx_source::TxSource::Remote {
                    cycles: 1001,
                    peer: SessionId::default(),
                },
            ))
            .unwrap()
    );

    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx1_proposal);

    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx2_proposal);

    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx0);

    let cur = queue.pop_front(false);
    assert_eq!(cur.unwrap().tx, tx3);
}

#[tokio::test]
async fn verify_queue_remove() {
    let tx1 = TransactionBuilder::default()
        .set_outputs_data(vec![Default::default()])
        .build();
    let entry1 = dummy_resolved_tx(
        tx1.clone(),
        crate::tx_source::TxSource::Remote {
            cycles: 1,
            peer: SessionId::new(1),
        },
    );
    let entry1_id = entry1.tx.proposal_short_id();
    eprintln!("entry1_id: {:?}", entry1_id);
    let tx2 = TransactionBuilder::default()
        .set_cell_deps(vec![Default::default(), Default::default()])
        .build();
    let entry2 = dummy_resolved_tx(
        tx2.clone(),
        crate::tx_source::TxSource::Remote {
            cycles: 2,
            peer: SessionId::new(2),
        },
    );
    let entry2_id = entry2.tx.proposal_short_id();
    eprintln!("entry2_id: {:?}", entry2_id);
    let tx3 = TransactionBuilder::default().build();
    let entry3 = dummy_resolved_tx(tx3.clone(), crate::tx_source::TxSource::Local);
    let entry3_id = entry3.tx.proposal_short_id();
    eprintln!("entry3_id: {:?}", entry3_id);

    let tx4 = TransactionBuilder::default()
        .set_cell_deps(vec![
            Default::default(),
            Default::default(),
            Default::default(),
        ])
        .build();
    let entry4 = dummy_resolved_tx(
        tx4.clone(),
        crate::tx_source::TxSource::Remote {
            cycles: 4,
            peer: SessionId::new(1),
        },
    );
    let entry4_id = entry4.tx.proposal_short_id();

    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    assert!(queue.add_tx(entry1.clone()).unwrap());
    assert!(queue.add_tx(entry2.clone()).unwrap());
    assert!(queue.add_tx(entry3.clone()).unwrap());
    assert!(queue.add_tx(entry4.clone()).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(queue.contains_key(&entry1_id));
    assert!(queue.contains_key(&entry2_id));
    assert!(queue.contains_key(&entry3_id));
    assert!(queue.contains_key(&entry4_id));

    queue.remove_txs_by_peer(&SessionId::new(1));

    assert!(!queue.contains_key(&entry1_id));
    assert!(!queue.contains_key(&entry4_id));
    assert!(queue.contains_key(&entry2_id));
    assert!(queue.contains_key(&entry3_id));
}

/// Helper: create a ResolvedTx with a specific fee (shannons).
fn dummy_resolved_tx_with_fee(
    tx: ckb_types::core::TransactionView,
    fee_shannons: u64,
    source: crate::tx_source::TxSource,
) -> ResolvedTx {
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    ResolvedTx {
        tx: tx.clone(),
        rtx,
        status: Status::Pending,
        fee: Capacity::shannons(fee_shannons),
        tx_size: tx.data().serialized_size_in_block(),
        pre_resolve_tip: Default::default(),
        snapshot: test_snapshot(),
        source,
    }
}

#[tokio::test]
async fn verify_queue_peek_arrival_time_ordering() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::ArrivalTime,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    // Add txs with different fees — in arrival-time mode, fee should NOT matter.
    let tx_high_fee = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("high").pack()])
        .build();
    let rtx_high =
        dummy_resolved_tx_with_fee(tx_high_fee, 10_000, crate::tx_source::TxSource::Local);
    let id_high = rtx_high.tx.proposal_short_id();

    sleep(std::time::Duration::from_millis(5)).await;

    let tx_low_fee = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("low").pack()])
        .build();
    let rtx_low = dummy_resolved_tx_with_fee(tx_low_fee, 100, crate::tx_source::TxSource::Local);
    let _id_low = rtx_low.tx.proposal_short_id();

    queue.add_tx(rtx_high).unwrap();
    queue.add_tx(rtx_low).unwrap();

    // Arrival time mode: first added (high fee, but that's irrelevant) is peeked first.
    let peeked = queue.peek(false).unwrap();
    assert_eq!(
        peeked, id_high,
        "arrival-time mode should return oldest first"
    );
}

#[tokio::test]
async fn verify_queue_peek_fee_rate_ordering() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::FeeRate,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    // Add a low-fee tx first, then a high-fee tx.
    // tx_size is roughly the same for both, so fee_rate ∝ fee.
    let tx_low = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("low-fee").pack()])
        .build();
    let rtx_low = dummy_resolved_tx_with_fee(tx_low, 100, crate::tx_source::TxSource::Local);
    let _id_low = rtx_low.tx.proposal_short_id();

    let tx_high = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("high-fee").pack()])
        .build();
    let rtx_high = dummy_resolved_tx_with_fee(tx_high, 10_000, crate::tx_source::TxSource::Local);
    let id_high = rtx_high.tx.proposal_short_id();

    queue.add_tx(rtx_low).unwrap();
    queue.add_tx(rtx_high).unwrap();

    // Fee-rate mode: highest fee rate should be peeked first regardless of arrival order.
    let peeked = queue.peek(false).unwrap();
    assert_eq!(
        peeked, id_high,
        "fee-rate mode should return highest fee rate first"
    );
}

#[tokio::test]
async fn verify_queue_fee_rate_pop_order() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::FeeRate,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    // Add 3 txs with increasing fees.
    let fees = [500u64, 5000, 50_000];
    let mut ids = Vec::new();
    for (i, &fee) in fees.iter().enumerate() {
        let tx = TransactionBuilder::default()
            .set_outputs_data(vec![Bytes::from(format!("tx-{i}")).pack()])
            .build();
        let rtx = dummy_resolved_tx_with_fee(tx, fee, crate::tx_source::TxSource::Local);
        ids.push(rtx.tx.proposal_short_id());
        queue.add_tx(rtx).unwrap();
    }

    // Pop order should be: highest fee first → lowest fee last.
    let e1 = queue.pop_front(false).unwrap();
    assert_eq!(
        e1.tx.proposal_short_id(),
        ids[2],
        "first pop should be highest fee"
    );

    let e2 = queue.pop_front(false).unwrap();
    assert_eq!(
        e2.tx.proposal_short_id(),
        ids[1],
        "second pop should be middle fee"
    );

    let e3 = queue.pop_front(false).unwrap();
    assert_eq!(
        e3.tx.proposal_short_id(),
        ids[0],
        "third pop should be lowest fee"
    );

    assert!(queue.pop_front(false).is_none());
}

#[tokio::test]
async fn verify_queue_proposal_count_tracks_insert_promote_and_remove() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::FeeRate,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    let tx_normal = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("normal").pack()])
        .build();
    queue
        .add_tx(dummy_resolved_tx_with_fee(
            tx_normal.clone(),
            100,
            crate::tx_source::TxSource::Local,
        ))
        .unwrap();
    assert_eq!(queue.proposal_count(), 0);

    let tx_proposal = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("proposal").pack()])
        .build();
    queue
        .add_tx(dummy_resolved_tx_with_fee(
            tx_proposal,
            1,
            crate::tx_source::TxSource::Proposal,
        ))
        .unwrap();
    assert_eq!(queue.proposal_count(), 1);

    // Promoting the normal tx in place bumps the count; re-promoting an
    // already-proposal entry must not double count.
    queue
        .add_tx(dummy_resolved_tx_with_fee(
            tx_normal.clone(),
            100,
            crate::tx_source::TxSource::Proposal,
        ))
        .unwrap();
    assert_eq!(queue.proposal_count(), 2);
    queue
        .add_tx(dummy_resolved_tx_with_fee(
            tx_normal,
            100,
            crate::tx_source::TxSource::Proposal,
        ))
        .unwrap();
    assert_eq!(queue.proposal_count(), 2);

    // Popping both proposals drains the count to zero, re-enabling the
    // scan-skip fast path in `peek_by_fee_rate`.
    assert!(queue.pop_front(false).is_some());
    assert_eq!(queue.proposal_count(), 1);
    assert!(queue.pop_front(false).is_some());
    assert_eq!(queue.proposal_count(), 0);
}

#[tokio::test]
async fn verify_queue_proposal_always_first_in_fee_rate_mode() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::FeeRate,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    // Add a high-fee non-proposal tx first.
    let tx_high = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("high-fee-normal").pack()])
        .build();
    let rtx_high = dummy_resolved_tx_with_fee(tx_high, 100_000, crate::tx_source::TxSource::Local);
    let id_high = rtx_high.tx.proposal_short_id();

    // Add a low-fee proposal tx.
    let tx_proposal = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("low-fee-proposal").pack()])
        .build();
    let rtx_proposal =
        dummy_resolved_tx_with_fee(tx_proposal, 1, crate::tx_source::TxSource::Proposal);
    let id_proposal = rtx_proposal.tx.proposal_short_id();

    queue.add_tx(rtx_high).unwrap();
    queue.add_tx(rtx_proposal).unwrap();

    // Proposal tx should be peeked first even though it has much lower fee.
    let peeked = queue.peek(false).unwrap();
    assert_eq!(
        peeked, id_proposal,
        "proposal tx should always be prioritised over non-proposal"
    );

    // After popping the proposal, the high-fee tx should be next.
    queue.pop_front(false).unwrap();
    let next = queue.peek(false).unwrap();
    assert_eq!(next, id_high);
}

#[tokio::test]
async fn verify_queue_fee_rate_remove_and_repeek() {
    let mut queue = VerifyQueue::new(
        MAX_TX_VERIFY_CYCLES,
        VerifyOrdering::FeeRate,
        TEST_MAX_VERIFY_QUEUE_TX_SIZE,
    );

    let tx1 = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("fee-1000").pack()])
        .build();
    let rtx1 = dummy_resolved_tx_with_fee(tx1, 1_000, crate::tx_source::TxSource::Local);
    let id1 = rtx1.tx.proposal_short_id();

    let tx2 = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("fee-5000").pack()])
        .build();
    let rtx2 = dummy_resolved_tx_with_fee(tx2, 5_000, crate::tx_source::TxSource::Local);
    let id2 = rtx2.tx.proposal_short_id();

    let tx3 = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("fee-3000").pack()])
        .build();
    let rtx3 = dummy_resolved_tx_with_fee(tx3, 3_000, crate::tx_source::TxSource::Local);
    let id3 = rtx3.tx.proposal_short_id();

    queue.add_tx(rtx1).unwrap();
    queue.add_tx(rtx2).unwrap();
    queue.add_tx(rtx3).unwrap();

    // Highest fee should be first.
    assert_eq!(queue.peek(false).unwrap(), id2);

    // Remove the highest fee tx.
    queue.remove_tx(&id2);

    // Now the second highest (id3, fee=3000) should be first.
    assert_eq!(queue.peek(false).unwrap(), id3);

    // Remove the lowest fee tx.
    queue.remove_tx(&id1);

    // id3 should still be first.
    assert_eq!(queue.peek(false).unwrap(), id3);

    // Pop should return id3.
    let popped = queue.pop_front(false).unwrap();
    assert_eq!(popped.tx.proposal_short_id(), id3);

    // Queue should be empty.
    assert!(queue.is_empty());
    assert!(queue.peek(false).is_none());
}

fn tx_pool_config() -> TxPoolConfig {
    TxPoolConfig {
        max_tx_pool_size: 180_000_000,
        min_fee_rate: FeeRate::zero(),
        min_rbf_rate: FeeRate::zero(),
        max_tx_verify_cycles: MAX_TX_VERIFY_CYCLES,
        max_tx_verify_workers: 1,
        max_ancestors_count: 125,
        keep_rejected_tx_hashes_days: 1,
        keep_rejected_tx_hashes_count: 1000,
        persisted_data: Default::default(),
        recent_reject: Default::default(),
        expiry_hours: 24,
        verify_ordering: VerifyOrdering::ArrivalTime,
        max_verify_queue_tx_size: 256_000_000,
    }
}

fn snapshot(consensus: Arc<Consensus>) -> Arc<Snapshot> {
    let tmp_dir = TempDir::new().expect("create temp dir");
    let store = ChainDB::new(
        RocksDB::open_in(&tmp_dir, UNUSED_SNAPSHOT_COLUMNS),
        Default::default(),
    );
    Arc::new(Snapshot::new(
        consensus.genesis_block().header(),
        U256::zero(),
        Default::default(),
        store.get_snapshot(),
        Default::default(),
        consensus,
    ))
}

pub(crate) fn dummy_network() -> crate::network::TxPoolNetworkHandle {
    Arc::new(crate::network::DummyTxPoolNetwork)
}

fn service() -> TxPoolService {
    service_with_relay_receiver().0
}

fn service_with_relay_receiver() -> (
    TxPoolService,
    ckb_channel::Receiver<TxVerificationResult>,
    watch::Sender<ChunkCommand>,
) {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let snapshot = snapshot(Arc::clone(&consensus));
    let config = tx_pool_config();
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(16);
    let (block_assembler_sender, _) = mpsc::channel(1);
    let max_workers = config.max_tx_verify_workers.max(1);
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let queues = Arc::new(crate::component::pipeline_queues::PipelineQueues {
        ordered_resolve_queue: RwLock::new(
            crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
        ),
        verify_queue: RwLock::new(VerifyQueue::new(
            config.max_tx_verify_cycles,
            config.verify_ordering,
            config.verify_queue_tx_size_budget(),
        )),
        pre_check_queue: crate::component::pre_check_queue::PreCheckQueue::new(pre_check_cancel),
        rbf_candidates: RwLock::new(crate::component::rbf_candidates::RbfCandidates::new()),
    });
    let service = TxPoolService {
        pool: crate::service::PoolCore {
            tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snapshot))),
            consensus: Arc::clone(&consensus),
            tx_pool_config: Arc::new(config),
        },
        pipeline: crate::service::PipelineState {
            queues: Arc::clone(&queues),
            waiting_room: Arc::new(RwLock::new(WaitingRoom::new())),
            chunk_rx,
            deferred_sender,
        },
        relay: crate::service::RelayState {
            network: dummy_network(),
            tx_relay_sender,
            block_assembler_sender,
            callbacks: Arc::new(Callbacks::new()),
            banned_peers: Default::default(),
        },
        aux: crate::service::AuxServices {
            txs_verify_cache: Arc::new(RwLock::new(init_cache())),
            recent_reject: None,
            fee_estimator: FeeEstimator::new_dummy(),
        },
        block_assembler: None,
        recovery_lock: Arc::new(tokio::sync::Mutex::new(())),
    };

    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            tokio::spawn(crate::service::workers::run_pre_check_worker_loop(svc));
        }
    }

    // Drain deferred tasks (RBF recovery + verify cache updates) for tests.
    {
        let queues = Arc::clone(&queues);
        let txs_verify_cache = Arc::clone(&service.aux.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = queues.ordered_resolve_queue.write().await;
                        for (tx, source) in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob::new(tx, source));
                        }
                    }
                    crate::service::DeferredTask::CacheUpdate { wtx_hash, verified } => {
                        let mut guard = txs_verify_cache.write().await;
                        guard.put(wtx_hash, verified);
                    }
                }
            }
        });
    }

    (service, tx_relay_receiver, chunk_tx)
}

/// Seed `parent` into the ordered resolve queue and inflate the queue's
/// `total_tx_size` so the next `add_tx` call will be rejected with
/// `Reject::Full`.  This avoids hitting the chain snapshot (which in tests
/// only has one column) when classifying dependent transactions.
async fn seed_parent_and_nearly_fill_queue(
    service: &TxPoolService,
    parent: ckb_types::core::TransactionView,
) {
    let mut ordered = service.pipeline.queues.ordered_resolve_queue.write().await;
    ordered.set_total_tx_size_for_test(crate::constants::MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE - 1_000);
    ordered
        .add_tx(ResolveJob::new(parent, TxSource::Local))
        .unwrap();
    ordered.set_total_tx_size_for_test(crate::constants::MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE - 1);
}

#[tokio::test]
async fn process_orphan_tx_keeps_high_cycle_orphan_when_ordered_resolve_queue_is_full() {
    let service = service();
    let parent = build_tx(vec![], 1);
    let orphan = build_tx(vec![(&parent.hash(), 0)], 1);
    let orphan_id = orphan.proposal_short_id();

    seed_parent_and_nearly_fill_queue(&service, parent.clone()).await;

    service
        .add_orphan(
            orphan.clone(),
            TxSource::Remote {
                cycles: MAX_TX_VERIFY_CYCLES + 1,
                peer: 1.into(),
            },
        )
        .await;

    let service_clone = service.clone();
    let handle = tokio::spawn(async move {
        service_clone.process_orphan_tx(&parent).await;
    });
    assert!(
        handle.await.is_ok(),
        "full resolve queue should not panic while requeueing a high-cycle orphan"
    );

    assert!(
        service
            .pipeline
            .waiting_room
            .read()
            .await
            .contains_key(&orphan_id)
    );
    assert!(
        !service
            .pipeline
            .queues
            .ordered_resolve_queue
            .read()
            .await
            .contains_key(&orphan_id)
    );
}

#[tokio::test]
async fn submit_remote_tx_notifies_relayer_when_ordered_resolve_queue_is_full() {
    let (service, tx_relay_receiver, _chunk_tx) = service_with_relay_receiver();
    let parent = build_tx(vec![], 1);
    let tx = build_tx(vec![(&parent.hash(), 0)], 1);
    let tx_hash = tx.hash();

    seed_parent_and_nearly_fill_queue(&service, parent).await;

    let ret = service
        .submit_remote_tx(
            tx,
            TxSource::Remote {
                cycles: MAX_TX_VERIFY_CYCLES,
                peer: 1.into(),
            },
        )
        .await;

    assert!(matches!(ret, Err(crate::error::Reject::Full(_))));
    match tx_relay_receiver
        .try_recv()
        .expect("expected reject notification")
    {
        TxVerificationResult::Reject { tx_hash: rejected } => {
            assert_eq!(rejected, tx_hash);
        }
        _ => panic!("expected reject notification"),
    }
}

#[tokio::test]
async fn notify_tx_notifies_relayer_when_ordered_resolve_queue_is_full() {
    let (service, tx_relay_receiver, _chunk_tx) = service_with_relay_receiver();
    let parent = build_tx(vec![], 1);
    let tx = build_tx(vec![(&parent.hash(), 0)], 1);
    let tx_hash = tx.hash();

    seed_parent_and_nearly_fill_queue(&service, parent).await;

    let ret = service.notify_tx(tx).await;

    assert!(matches!(ret, Err(crate::error::Reject::Full(_))));
    match tx_relay_receiver
        .try_recv()
        .expect("expected reject notification")
    {
        TxVerificationResult::Reject { tx_hash: rejected } => {
            assert_eq!(rejected, tx_hash);
        }
        _ => panic!("expected reject notification"),
    }
}

/// Build a service with a real `RecentReject` database so that
/// `handle_remote_reject` recording paths can be tested.
fn service_with_recent_reject() -> (TxPoolService, ckb_channel::Receiver<TxVerificationResult>) {
    let (service, receiver, _chunk_tx) = service_with_relay_receiver();
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let recent_reject = crate::component::recent_reject::RecentReject::build(
        tmp_dir.path(),
        2,   // shard_num
        100, // count_limit
        -1,  // ttl
    )
    .expect("create test recent_reject");
    // Keep the temp dir alive for the lifetime of the service by leaking it.
    // Tests are short-lived, so this is acceptable.
    let _ = Box::leak(Box::new(tmp_dir));
    (
        TxPoolService {
            aux: crate::service::AuxServices {
                recent_reject: Some(Arc::new(recent_reject)),
                ..service.aux.clone()
            },
            ..service
        },
        receiver,
    )
}

#[tokio::test]
async fn handle_remote_reject_records_reject_and_rejects_relay() {
    let (service, tx_relay_receiver) = service_with_recent_reject();
    let tx = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let tx_hash = tx.hash();
    // Malformed tx: should be recorded and should ban the peer, but not relayed.
    let reject = Reject::Malformed("bad tx".to_string(), Default::default());

    service
        .handle_remote_reject(&tx_hash, &reject, 1.into())
        .await;

    // No relay reject for malformed tx.
    assert!(
        tx_relay_receiver.try_recv().is_err(),
        "malformed reject should not be relayed"
    );
    // Recorded in recent_reject.
    let recent_reject = service
        .aux
        .recent_reject
        .as_ref()
        .expect("recent_reject set");
    assert!(
        recent_reject.get(&tx_hash).unwrap().is_some(),
        "malformed reject should be recorded"
    );
}

#[tokio::test]
async fn handle_remote_reject_relays_allowed_rejects() {
    let (service, tx_relay_receiver) = service_with_recent_reject();
    let tx = build_tx(vec![(&Byte32::zero(), 0)], 1);
    let tx_hash = tx.hash();
    // DeclaredWrongCycles: malformed but allowed to be relayed with correct cycles.
    let reject = Reject::DeclaredWrongCycles(100, 200);

    service
        .handle_remote_reject(&tx_hash, &reject, 1.into())
        .await;

    match tx_relay_receiver.try_recv().expect("expected reject relay") {
        TxVerificationResult::Reject { tx_hash: rejected } => {
            assert_eq!(rejected, tx_hash);
        }
        _ => panic!("expected Reject relay"),
    }
    // Also recorded.
    let recent_reject = service
        .aux
        .recent_reject
        .as_ref()
        .expect("recent_reject set");
    assert!(
        recent_reject.get(&tx_hash).unwrap().is_some(),
        "declared-wrong-cycles reject should be recorded"
    );
}

#[tokio::test]
async fn handle_missing_input_orphan_notifies_relayer_once() {
    let (service, tx_relay_receiver, _chunk_tx) = service_with_relay_receiver();
    let parent = build_tx(vec![], 1);
    let parent_hash = parent.hash();
    let orphan = build_tx(vec![(&parent_hash, 0)], 1);
    let orphan_id = orphan.proposal_short_id();
    let parents: std::collections::HashSet<Byte32> = std::iter::once(parent_hash).collect();

    // First call should add to orphan pool and notify relayer.
    service
        .handle_missing_input_orphan(
            orphan.clone(),
            TxSource::Remote {
                cycles: 100,
                peer: 1.into(),
            },
            parents.clone(),
        )
        .await;

    assert!(
        service
            .pipeline
            .waiting_room
            .read()
            .await
            .contains_key(&orphan_id)
    );
    match tx_relay_receiver
        .try_recv()
        .expect("expected UnknownParents on first add")
    {
        TxVerificationResult::UnknownParents { peer, parents: p } => {
            assert_eq!(peer, 1.into());
            assert_eq!(p, parents);
        }
        _ => panic!("expected UnknownParents"),
    }

    // Second call for the same tx is a duplicate; orphan pool already contains
    // it, so the relayer must not receive another UnknownParents notification.
    service
        .handle_missing_input_orphan(
            orphan,
            TxSource::Remote {
                cycles: 100,
                peer: 2.into(),
            },
            parents,
        )
        .await;

    assert!(
        tx_relay_receiver.try_recv().is_err(),
        "duplicate orphan should not trigger second UnknownParents"
    );
}

/// The pre-check worker must close the loop with the relayer when the
/// ordered resolve queue rejects a dependent job with `Full`: previously
/// the worker discarded the classification result entirely, so the peer's
/// filter entry would wait forever (no notification, no record, no orphan).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pre_check_worker_notifies_relayer_when_ordered_resolve_queue_is_full() {
    let (service, tx_relay_receiver, _chunk_tx) = service_with_relay_receiver();
    let parent = build_tx(vec![], 1);
    let tx = build_tx(vec![(&parent.hash(), 0)], 1);
    let tx_hash = tx.hash();

    seed_parent_and_nearly_fill_queue(&service, parent).await;

    // The dependent job goes through the pre-check worker pool (spawned by
    // `service_with_relay_receiver`); the ordered queue rejects it with
    // `Full`, and the worker must notify the relayer.
    service
        .pipeline
        .queues
        .pre_check_queue
        .push(crate::component::pre_check_queue::PreCheckJob {
            tx: tx.clone(),
            source: TxSource::Remote {
                cycles: MAX_TX_VERIFY_CYCLES,
                peer: 1.into(),
            },
        })
        .unwrap();

    let notified = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(TxVerificationResult::Reject { tx_hash: rejected }) =
                tx_relay_receiver.try_recv()
                && rejected == tx_hash
            {
                break;
            }
            sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        notified.is_ok(),
        "pre-check worker must notify the relayer on ordered-queue Full"
    );
}

/// A job from a peer banned *after* the job entered the pipeline must be
/// dropped by the worker instead of flowing into the pool: queue-level
/// removal only covers queued jobs, not popped ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn banned_peer_job_is_dropped_by_pre_check_worker() {
    let (service, tx_relay_receiver, _chunk_tx) = service_with_relay_receiver();
    let peer: ckb_network::PeerIndex = 7.into();
    let tx = build_tx(vec![], 1);
    let tx_hash = tx.hash();

    assert!(!service.is_recently_banned(TxSource::Remote { cycles: 0, peer }));
    service.ban_malformed(peer, "test ban".to_string()).await;
    assert!(service.is_recently_banned(TxSource::Remote { cycles: 0, peer }));

    // The job enters the pre-check queue after the ban: the worker pops it,
    // sees the ban, and drops it terminally (relayer hears Reject).
    service
        .pipeline
        .queues
        .pre_check_queue
        .push(crate::component::pre_check_queue::PreCheckJob {
            tx: tx.clone(),
            source: TxSource::Remote {
                cycles: MAX_TX_VERIFY_CYCLES,
                peer,
            },
        })
        .unwrap();

    let notified = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(TxVerificationResult::Reject { tx_hash: rejected }) =
                tx_relay_receiver.try_recv()
                && rejected == tx_hash
            {
                break;
            }
            sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        notified.is_ok(),
        "banned peer's in-flight job must be dropped with a Reject notification"
    );
}

/// A parent parked in the waiting room (orphan or RBF-held) must count as
/// in flight for the local-orphan retry heuristic: otherwise the child
/// burns its small immediate-retry budget while the parent is merely parked.
#[tokio::test]
async fn parent_parked_in_waiting_room_counts_as_in_flight() {
    let service = service();
    let parent = build_tx(vec![], 1);
    let parent_hash = parent.hash();

    assert!(service.add_orphan(parent.clone(), TxSource::Local).await);

    let parents: std::collections::HashSet<Byte32> = [parent_hash].into_iter().collect();
    assert!(
        service.all_missing_parents_in_flight(&parents).await,
        "a parent parked in the waiting room must count as in flight"
    );
}

/// Bounded recovery retries must end in a *terminal* outcome, not a silent
/// drop: recovered txs lost their conflict-cache handle when the recovery
/// began, so an exhausted recovery must still notify the relayer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recover_gives_up_terminally_after_bounded_retries() {
    let (service, tx_relay_receiver, _chunk_tx) = service_with_relay_receiver();
    // Permanently full ordered queue.
    {
        let mut queue = service.pipeline.queues.ordered_resolve_queue.write().await;
        queue.set_total_tx_size_for_test(crate::constants::MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE);
    }

    let tx = build_tx(vec![], 1);
    let tx_hash = tx.hash();
    let queues = Arc::clone(&service.pipeline.queues);
    let relay = service.relay.clone();
    let cancel = ckb_stop_handler::CancellationToken::new();

    let start = std::time::Instant::now();
    crate::process::recover::enqueue_recover_txs(
        queues,
        vec![(
            tx,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
        )],
        &cancel,
        &relay,
    )
    .await;
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(1_500),
        "the bounded retries must actually back off before giving up"
    );

    match tx_relay_receiver.try_recv() {
        Ok(TxVerificationResult::Reject { tx_hash: rejected }) => {
            assert_eq!(rejected, tx_hash);
        }
        other => panic!(
            "exhausted recovery must end with a terminal Reject notification, got {:?}",
            other
        ),
    }
}

/// Administrative removal must clear a double-parked transaction from
/// *both* waiting rooms (pipeline-side and pool-side): previously the
/// pipeline-side hit returned early, leaving the pool-side copy to linger
/// until budget eviction.
#[tokio::test]
async fn remove_tx_clears_double_parked_transaction_from_both_rooms() {
    let service = service();
    let tx = build_tx(vec![], 1);
    let id = tx.proposal_short_id();

    // Double-park: pipeline-side (orphan) and pool-side (InputsBlocked).
    assert!(service.add_orphan(tx.clone(), TxSource::Local).await);
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool.record_conflict(tx.clone(), TxSource::Local);
    }

    assert!(matches!(
        service.remove_tx(tx.hash()).await,
        crate::service::RemoveTxOutcome::Removed
    ));
    assert!(
        !service.pipeline.waiting_room.read().await.contains_key(&id),
        "pipeline-side entry must be removed"
    );
    assert!(
        service
            .pool
            .tx_pool
            .read()
            .await
            .waiting_room
            .get(&id)
            .is_none(),
        "pool-side entry must be removed too"
    );
}

/// An expired parent must cascade to its descendants: its children can
/// never resolve once the parent's outputs die, so leaving them would make
/// them zombies until their own expiry (and the template builder would
/// filter them every round).
#[tokio::test]
async fn remove_expired_cascades_to_descendants() {
    let service = service();
    let parent = build_tx(vec![], 1);
    let child = build_tx(vec![(&parent.hash(), 0)], 1);
    let parent_id = parent.proposal_short_id();
    let child_id = child.proposal_short_id();

    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        // The parent is already expired (timestamp 1); the child is fresh.
        tx_pool
            .pool_map
            .add_entry(
                crate::component::entry::TxEntry::new_with_timestamp(
                    Arc::new(ckb_types::core::cell::ResolvedTransaction::dummy_resolve(
                        parent.clone(),
                    )),
                    0,
                    Capacity::shannons(1),
                    100,
                    1,
                ),
                Status::Pending,
            )
            .unwrap();
        tx_pool
            .pool_map
            .add_entry(
                crate::component::entry::TxEntry::dummy_resolve(
                    child.clone(),
                    0,
                    Capacity::shannons(1),
                    100,
                ),
                Status::Pending,
            )
            .unwrap();
    }

    let mut events = Vec::new();
    {
        let mut tx_pool = service.pool.tx_pool.write().await;
        tx_pool.remove_expired(&mut events);
    }

    let removed: std::collections::HashSet<_> = events
        .iter()
        .map(|(entry, _)| entry.proposal_short_id())
        .collect();
    assert!(removed.contains(&parent_id));
    assert!(
        removed.contains(&child_id),
        "an expired parent must cascade to its child"
    );
    assert!(
        events
            .iter()
            .all(|(_, reject)| matches!(reject, crate::error::Reject::Expiry(_))),
        "every cascaded entry gets its own Expiry reject"
    );
    {
        let tx_pool = service.pool.tx_pool.read().await;
        assert!(tx_pool.pool_map.get_by_id(&parent_id).is_none());
        assert!(tx_pool.pool_map.get_by_id(&child_id).is_none());
    }
}

/// A dropped `VerifyMgr` (e.g. after a manager-level panic, before the
/// monitor respawns a new one) must cancel its whole worker generation:
/// detached workers of the old generation must not keep draining the
/// queue alongside the respawned generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_verify_mgr_cancels_its_worker_generation() {
    use ckb_stop_handler::CancellationToken;

    let (service, _relay, _chunk_tx) = service_with_relay_receiver();
    let (chunk_tx, _chunk_rx) = watch::channel(ChunkCommand::Resume);
    let parent = CancellationToken::new();
    let mut mgr = crate::verify_mgr::VerifyMgr::new(
        service.clone(),
        chunk_tx.subscribe(),
        parent.child_token(),
    );
    let handle = tokio::spawn(async move { mgr.run().await });

    let park = |id: u8| {
        let service = service.clone();
        async move {
            let tx = build_tx(vec![(&Byte32::new([id; 32]), 0)], 1);
            let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();
            let resolved = ResolvedTx {
                tx: tx.clone(),
                rtx: Arc::new(ckb_types::core::cell::ResolvedTransaction::dummy_resolve(
                    tx.clone(),
                )),
                status: Status::Pending,
                fee: Capacity::zero(),
                tx_size: tx.data().serialized_size_in_block(),
                pre_resolve_tip: Default::default(),
                snapshot,
                source: TxSource::Local,
            };
            let mut verify = service.pipeline.queues.verify_queue.write().await;
            verify.add_tx(resolved).unwrap();
        }
    };

    // The live generation drains a queued job.
    park(0x51).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if service.pipeline.queues.verify_queue.read().await.is_empty() {
                break;
            }
            sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("live workers must drain the queue");

    // Simulate a manager-level panic: the manager task dies and its
    // VerifyMgr is dropped. The whole generation must shut down.
    handle.abort();
    let _ = handle.await;

    park(0x52).await;
    sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !service.pipeline.queues.verify_queue.read().await.is_empty(),
        "workers of a dropped generation must not keep running"
    );
}

/// `save_pool` must wait for the reorg recovery lock, so it can never
/// persist a half-updated pool mid-reorg.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn save_pool_waits_for_recovery_lock() {
    let service = service();
    let guard = service.recovery_lock.lock().await;

    let svc = service.clone();
    let save = tokio::spawn(async move { svc.save_pool().await });
    sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !save.is_finished(),
        "save_pool must block while the recovery lock is held"
    );

    drop(guard);
    tokio::time::timeout(std::time::Duration::from_secs(5), save)
        .await
        .expect("save_pool must complete once the recovery lock is released")
        .expect("save task joins");
}
