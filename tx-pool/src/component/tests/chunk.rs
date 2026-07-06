use crate::callback::Callbacks;
use crate::component::orphan::OrphanPool;
use crate::component::pipeline_queue::PipelineQueue;
use crate::component::tests::util::build_tx;
use crate::component::verify_queue::VerifyQueue;
use crate::pool::TxPool;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::{TxPoolService, TxVerificationResult};
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
const UNUSED_SNAPSHOT_COLUMNS: u32 = 1;

fn test_snapshot() -> Arc<Snapshot> {
    use std::sync::OnceLock;
    static SNAPSHOT: OnceLock<Arc<Snapshot>> = OnceLock::new();
    Arc::<Snapshot>::clone(
        SNAPSHOT.get_or_init(|| snapshot(Arc::new(ConsensusBuilder::default().build()))),
    )
}

fn dummy_resolved_tx(
    tx: ckb_types::core::TransactionView,
    remote: Option<(u64, SessionId)>,
    is_proposal_tx: bool,
) -> ResolvedTx {
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    ResolvedTx {
        tx: tx.clone(),
        rtx,
        status: crate::process::TxStatus::Fresh,
        fee: Capacity::zero(),
        tx_size: tx.data().serialized_size_in_block(),
        pre_resolve_tip: Default::default(),
        snapshot: test_snapshot(),
        remote,
        is_proposal_tx,
    }
}
#[tokio::test]
async fn verify_queue_basic() {
    let tx = TransactionBuilder::default().build();
    let entry = dummy_resolved_tx(tx.clone(), None, false);
    let tx2 = build_tx(vec![(&tx.hash(), 0)], 1);

    let id = tx.proposal_short_id();
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::ArrivalTime);
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
            .add_tx(dummy_resolved_tx(tx.clone(), None, false))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        !queue
            .add_tx(dummy_resolved_tx(tx.clone(), None, false))
            .unwrap()
    );

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));

    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx.clone(), None, false))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));

    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx.clone(), None, false))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx2.clone(), None, false))
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::ArrivalTime);
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

    let remote = |cycles| Some((cycles, SessionId::default()));

    let tx0 = build_tx(vec![(&H256([0; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx0.clone(), remote(1001), false))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx1 = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(
                tx1.clone(),
                remote(MAX_TX_VERIFY_CYCLES + 1),
                false
            ))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx2 = build_tx(vec![(&H256([2; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx2.clone(), remote(1001), false))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by time (tx1, tx2)

    let tx3 = build_tx(vec![(&H256([3; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx3.clone(), remote(1001), false))
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
                remote(2000000),
                true
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
    let queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::ArrivalTime);
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::ArrivalTime);
    let remote = |cycles| Some((cycles, SessionId::default()));

    let tx0 = build_tx(vec![(&H256([0; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx0.clone(), remote(1001), false))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx1_proposal = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx1_proposal.clone(), remote(1001), true))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx2_proposal = build_tx(vec![(&H256([2; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx2_proposal.clone(), remote(1001), true))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx3 = build_tx(vec![(&H256([3; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(dummy_resolved_tx(tx3.clone(), remote(1001), false))
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
    let entry1 = dummy_resolved_tx(tx1.clone(), Some((1, SessionId::new(1))), false);
    let entry1_id = entry1.tx.proposal_short_id();
    eprintln!("entry1_id: {:?}", entry1_id);
    let tx2 = TransactionBuilder::default()
        .set_cell_deps(vec![Default::default(), Default::default()])
        .build();
    let entry2 = dummy_resolved_tx(tx2.clone(), Some((2, SessionId::new(2))), false);
    let entry2_id = entry2.tx.proposal_short_id();
    eprintln!("entry2_id: {:?}", entry2_id);
    let tx3 = TransactionBuilder::default().build();
    let entry3 = dummy_resolved_tx(tx3.clone(), None, false);
    let entry3_id = entry3.tx.proposal_short_id();
    eprintln!("entry3_id: {:?}", entry3_id);

    let tx4 = TransactionBuilder::default()
        .set_cell_deps(vec![
            Default::default(),
            Default::default(),
            Default::default(),
        ])
        .build();
    let entry4 = dummy_resolved_tx(tx4.clone(), Some((4, SessionId::new(1))), false);
    let entry4_id = entry4.tx.proposal_short_id();

    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::ArrivalTime);

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
    is_proposal_tx: bool,
) -> ResolvedTx {
    let rtx = Arc::new(ResolvedTransaction::dummy_resolve(tx.clone()));
    ResolvedTx {
        tx: tx.clone(),
        rtx,
        status: crate::process::TxStatus::Fresh,
        fee: Capacity::shannons(fee_shannons),
        tx_size: tx.data().serialized_size_in_block(),
        pre_resolve_tip: Default::default(),
        snapshot: test_snapshot(),
        remote: None,
        is_proposal_tx,
    }
}

#[tokio::test]
async fn verify_queue_peek_arrival_time_ordering() {
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::ArrivalTime);

    // Add txs with different fees — in arrival-time mode, fee should NOT matter.
    let tx_high_fee = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("high").pack()])
        .build();
    let rtx_high = dummy_resolved_tx_with_fee(tx_high_fee, 10_000, false);
    let id_high = rtx_high.tx.proposal_short_id();

    sleep(std::time::Duration::from_millis(5)).await;

    let tx_low_fee = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("low").pack()])
        .build();
    let rtx_low = dummy_resolved_tx_with_fee(tx_low_fee, 100, false);
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::FeeRate);

    // Add a low-fee tx first, then a high-fee tx.
    // tx_size is roughly the same for both, so fee_rate ∝ fee.
    let tx_low = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("low-fee").pack()])
        .build();
    let rtx_low = dummy_resolved_tx_with_fee(tx_low, 100, false);
    let _id_low = rtx_low.tx.proposal_short_id();

    let tx_high = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("high-fee").pack()])
        .build();
    let rtx_high = dummy_resolved_tx_with_fee(tx_high, 10_000, false);
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::FeeRate);

    // Add 3 txs with increasing fees.
    let fees = [500u64, 5000, 50_000];
    let mut ids = Vec::new();
    for (i, &fee) in fees.iter().enumerate() {
        let tx = TransactionBuilder::default()
            .set_outputs_data(vec![Bytes::from(format!("tx-{i}")).pack()])
            .build();
        let rtx = dummy_resolved_tx_with_fee(tx, fee, false);
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
async fn verify_queue_proposal_always_first_in_fee_rate_mode() {
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::FeeRate);

    // Add a high-fee non-proposal tx first.
    let tx_high = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("high-fee-normal").pack()])
        .build();
    let rtx_high = dummy_resolved_tx_with_fee(tx_high, 100_000, false);
    let id_high = rtx_high.tx.proposal_short_id();

    // Add a low-fee proposal tx.
    let tx_proposal = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("low-fee-proposal").pack()])
        .build();
    let rtx_proposal = dummy_resolved_tx_with_fee(tx_proposal, 1, true);
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES, VerifyOrdering::FeeRate);

    let tx1 = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("fee-1000").pack()])
        .build();
    let rtx1 = dummy_resolved_tx_with_fee(tx1, 1_000, false);
    let id1 = rtx1.tx.proposal_short_id();

    let tx2 = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("fee-5000").pack()])
        .build();
    let rtx2 = dummy_resolved_tx_with_fee(tx2, 5_000, false);
    let id2 = rtx2.tx.proposal_short_id();

    let tx3 = TransactionBuilder::default()
        .set_outputs_data(vec![Bytes::from("fee-3000").pack()])
        .build();
    let rtx3 = dummy_resolved_tx_with_fee(tx3, 3_000, false);
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

fn service_with_relay_receiver() -> (TxPoolService, ckb_channel::Receiver<TxVerificationResult>) {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let snapshot = snapshot(Arc::clone(&consensus));
    let config = tx_pool_config();
    let (tx_relay_sender, tx_relay_receiver) = ckb_channel::bounded(16);
    let (block_assembler_sender, _) = mpsc::channel(1);
    let max_workers = config.max_tx_verify_workers.max(1);
    let pre_check_workers =
        max_workers.min(std::thread::available_parallelism().map_or(4, |n| n.get()));
    let pre_check_cancel = ckb_stop_handler::CancellationToken::new();
    let pre_check_queue = Arc::new(crate::component::pre_check_queue::PreCheckQueue::new(
        pre_check_cancel,
    ));
    let (deferred_sender, mut deferred_receiver) = mpsc::channel(1024);
    let (_chunk_tx, chunk_rx) = watch::channel(ChunkCommand::Resume);

    let ordered_resolve_queue = Arc::new(RwLock::new(
        crate::component::ordered_resolve_queue::OrderedResolveQueue::new(),
    ));
    let service = TxPoolService {
        tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snapshot))),
        orphan: Arc::new(RwLock::new(OrphanPool::new())),
        consensus: Arc::clone(&consensus),
        tx_pool_config: Arc::new(config.clone()),
        block_assembler: None,
        txs_verify_cache: Arc::new(RwLock::new(init_cache())),
        callbacks: Arc::new(Callbacks::new()),
        network: dummy_network(),
        tx_relay_sender,
        ordered_resolve_queue: Arc::clone(&ordered_resolve_queue),
        verify_queue: Arc::new(RwLock::new(VerifyQueue::new(
            config.max_tx_verify_cycles,
            config.verify_ordering,
        ))),
        block_assembler_sender,
        fee_estimator: FeeEstimator::new_dummy(),
        recent_reject: None,
        pre_check_queue: Arc::clone(&pre_check_queue),
        chunk_rx,
        rbf_candidates: Arc::new(RwLock::new(
            crate::component::rbf_candidates::RbfCandidates::new(),
        )),
        deferred_sender,
    };

    {
        for _ in 0..pre_check_workers {
            let svc = service.clone();
            let queue = Arc::clone(&pre_check_queue);
            tokio::spawn(async move {
                while let Some(job) = queue.pop().await {
                    let _ = svc
                        .classify_and_enqueue_tx(job.tx, job.is_proposal_tx, job.remote)
                        .await;
                }
            });
        }
    }

    // Drain deferred tasks (RBF recovery + verify cache updates) for tests.
    {
        let ordered = Arc::clone(&ordered_resolve_queue);
        let txs_verify_cache = Arc::clone(&service.txs_verify_cache);
        tokio::spawn(async move {
            while let Some(task) = deferred_receiver.recv().await {
                match task {
                    crate::service::DeferredTask::RecoverTxs(txs) => {
                        let mut queue = ordered.write().await;
                        for tx in txs {
                            let _ = queue.add_tx(crate::resolved_tx::ResolveJob {
                                tx,
                                remote: None,
                                is_proposal_tx: false,
                                attempts: 0,
                            });
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

    (service, tx_relay_receiver)
}

/// Seed `parent` into the ordered resolve queue and inflate the queue's
/// `total_tx_size` so the next `add_tx` call will be rejected with
/// `Reject::Full`.  This avoids hitting the chain snapshot (which in tests
/// only has one column) when classifying dependent transactions.
async fn seed_parent_and_nearly_fill_queue(
    service: &TxPoolService,
    parent: ckb_types::core::TransactionView,
) {
    let mut ordered = service.ordered_resolve_queue.write().await;
    ordered.set_total_tx_size_for_test(256_000_000 - 1_000);
    ordered
        .add_tx(ResolveJob {
            tx: parent,
            remote: None,
            is_proposal_tx: false,
            attempts: 0,
        })
        .unwrap();
    ordered.set_total_tx_size_for_test(256_000_000 - 1);
}

#[tokio::test]
async fn process_orphan_tx_keeps_high_cycle_orphan_when_ordered_resolve_queue_is_full() {
    let service = service();
    let parent = build_tx(vec![], 1);
    let orphan = build_tx(vec![(&parent.hash(), 0)], 1);
    let orphan_id = orphan.proposal_short_id();

    seed_parent_and_nearly_fill_queue(&service, parent.clone()).await;

    service
        .add_orphan(orphan.clone(), 1.into(), MAX_TX_VERIFY_CYCLES + 1)
        .await;

    let service_clone = service.clone();
    let handle = tokio::spawn(async move {
        service_clone.process_orphan_tx(&parent).await;
    });
    assert!(
        handle.await.is_ok(),
        "full resolve queue should not panic while requeueing a high-cycle orphan"
    );

    assert!(service.orphan.read().await.contains_key(&orphan_id));
    assert!(
        !service
            .ordered_resolve_queue
            .read()
            .await
            .contains_key(&orphan_id)
    );
}

#[tokio::test]
async fn submit_remote_tx_notifies_relayer_when_ordered_resolve_queue_is_full() {
    let (service, tx_relay_receiver) = service_with_relay_receiver();
    let parent = build_tx(vec![], 1);
    let tx = build_tx(vec![(&parent.hash(), 0)], 1);
    let tx_hash = tx.hash();

    seed_parent_and_nearly_fill_queue(&service, parent).await;

    let ret = service
        .submit_remote_tx(tx, MAX_TX_VERIFY_CYCLES, 1.into())
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
    let (service, tx_relay_receiver) = service_with_relay_receiver();
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
    let (service, receiver) = service_with_relay_receiver();
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
            recent_reject: Some(Arc::new(recent_reject)),
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
    let recent_reject = service.recent_reject.as_ref().expect("recent_reject set");
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
    let recent_reject = service.recent_reject.as_ref().expect("recent_reject set");
    assert!(
        recent_reject.get(&tx_hash).unwrap().is_some(),
        "declared-wrong-cycles reject should be recorded"
    );
}

#[tokio::test]
async fn handle_missing_input_orphan_notifies_relayer_once() {
    let (service, tx_relay_receiver) = service_with_relay_receiver();
    let parent = build_tx(vec![], 1);
    let parent_hash = parent.hash();
    let orphan = build_tx(vec![(&parent_hash, 0)], 1);
    let orphan_id = orphan.proposal_short_id();
    let parents: std::collections::HashSet<Byte32> = std::iter::once(parent_hash).collect();

    // First call should add to orphan pool and notify relayer.
    service
        .handle_missing_input_orphan(orphan.clone(), 1.into(), 100, parents.clone())
        .await;

    assert!(service.orphan.read().await.contains_key(&orphan_id));
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
        .handle_missing_input_orphan(orphan, 2.into(), 100, parents)
        .await;

    assert!(
        tx_relay_receiver.try_recv().is_err(),
        "duplicate orphan should not trigger second UnknownParents"
    );
}
