use crate::callback::Callbacks;
use crate::component::orphan::OrphanPool;
use crate::component::tests::util::build_tx;
use crate::component::verify_queue::{Entry, VerifyQueue};
use crate::pool::TxPool;
use crate::service::{TxPoolService, TxVerificationResult};
use ckb_app_config::{NetworkConfig, TxPoolConfig};
use ckb_async_runtime::new_background_runtime;
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_db::RocksDB;
use ckb_fee_estimator::FeeEstimator;
use ckb_network::SessionId;
use ckb_network::network::TransportType;
use ckb_network::{Flags, NetworkService, NetworkState};
use ckb_snapshot::Snapshot;
use ckb_store::ChainDB;
use ckb_types::H256;
use ckb_types::U256;
use ckb_types::core::{FeeRate, TransactionBuilder};
use ckb_verification::cache::init_cache;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::select;
use tokio::sync::watch;
use tokio::sync::{RwLock, mpsc};
use tokio::time::sleep;

const MAX_TX_VERIFY_CYCLES: u64 = 70_000_000;
const UNUSED_SNAPSHOT_COLUMNS: u32 = 1;
#[tokio::test]
async fn verify_queue_basic() {
    let tx = TransactionBuilder::default().build();
    let entry = Entry {
        tx: tx.clone(),
        remote: None,
    };
    let tx2 = build_tx(vec![(&tx.hash(), 0)], 1);

    let id = tx.proposal_short_id();
    let (exit_tx, mut exit_rx) = watch::channel(());
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES);
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

    assert!(queue.add_tx(tx.clone(), false, None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(!queue.add_tx(tx.clone(), false, None).unwrap());

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));
    assert!(!queue.contains_key(&id));

    assert!(queue.add_tx(tx.clone(), false, None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert_eq!(queue.pop_front(false).as_ref(), Some(&entry));

    assert!(queue.add_tx(tx.clone(), false, None).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    assert!(queue.add_tx(tx2.clone(), false, None).unwrap());
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
    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES);
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
    assert!(queue.add_tx(tx0.clone(), false, remote(1001)).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    let tx1 = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(tx1.clone(), false, remote(MAX_TX_VERIFY_CYCLES + 1))
            .unwrap()
    );
    sleep(std::time::Duration::from_millis(100)).await;

    let tx2 = build_tx(vec![(&H256([2; 32]).into(), 0)], 1);
    assert!(queue.add_tx(tx2.clone(), false, remote(1001)).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;
    // now queue should be sorted by time (tx1, tx2)

    let tx3 = build_tx(vec![(&H256([3; 32]).into(), 0)], 1);
    assert!(queue.add_tx(tx3.clone(), false, remote(1001)).unwrap());
    sleep(std::time::Duration::from_millis(100)).await;

    let tx_size_sum = [&tx0, &tx1, &tx2, &tx3]
        .iter()
        .map(|tx| tx.data().serialized_size_in_block())
        .sum::<usize>();

    assert_eq!(queue.total_tx_size(), tx_size_sum);

    let tx_4_proposal = build_tx(vec![(&H256([4; 32]).into(), 0)], 1);
    assert!(
        queue
            .add_tx(tx_4_proposal.clone(), true, remote(2000000))
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
async fn verify_queue_remove() {
    let entry1 = Entry {
        tx: TransactionBuilder::default()
            .set_outputs_data(vec![Default::default()])
            .build(),
        remote: Some((1, SessionId::new(1))),
    };
    let entry1_id = entry1.tx.proposal_short_id();
    eprintln!("entry1_id: {:?}", entry1_id);
    let entry2 = Entry {
        tx: TransactionBuilder::default()
            .set_cell_deps(vec![Default::default(), Default::default()])
            .build(),
        remote: Some((2, SessionId::new(2))),
    };
    let entry2_id = entry2.tx.proposal_short_id();
    eprintln!("entry2_id: {:?}", entry2_id);
    let entry3 = Entry {
        tx: TransactionBuilder::default().build(),
        remote: None,
    };
    let entry3_id = entry3.tx.proposal_short_id();
    eprintln!("entry3_id: {:?}", entry3_id);

    let entry4 = Entry {
        tx: TransactionBuilder::default()
            .set_cell_deps(vec![
                Default::default(),
                Default::default(),
                Default::default(),
            ])
            .build(),
        remote: Some((4, SessionId::new(1))),
    };
    let entry4_id = entry4.tx.proposal_short_id();

    let mut queue = VerifyQueue::new(MAX_TX_VERIFY_CYCLES);

    assert!(
        queue
            .add_tx(entry1.tx.clone(), false, entry1.remote)
            .unwrap()
    );
    assert!(
        queue
            .add_tx(entry2.tx.clone(), false, entry2.remote)
            .unwrap()
    );
    assert!(
        queue
            .add_tx(entry3.tx.clone(), false, entry3.remote)
            .unwrap()
    );
    assert!(
        queue
            .add_tx(entry4.tx.clone(), false, entry4.remote)
            .unwrap()
    );
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

fn network(consensus: &Consensus) -> ckb_network::NetworkController {
    let handle = new_background_runtime();
    let tmp_dir = TempDir::new().expect("create temp dir");
    let config = NetworkConfig {
        max_peers: 19,
        max_outbound_peers: 5,
        path: tmp_dir.path().to_path_buf(),
        ping_interval_secs: 15,
        ping_timeout_secs: 20,
        connect_outbound_interval_secs: 1,
        discovery_local_address: true,
        bootnode_mode: true,
        reuse_port_on_linux: true,
        ..Default::default()
    };
    let network_state =
        Arc::new(NetworkState::from_config(config).expect("init test network state"));
    NetworkService::new(
        network_state,
        vec![],
        vec![],
        (consensus.identify_name(), "test".to_string(), Flags::all()),
        TransportType::Tcp,
    )
    .start(&handle)
    .expect("start test network service")
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

    (
        TxPoolService {
            tx_pool: Arc::new(RwLock::new(TxPool::new(config.clone(), snapshot))),
            orphan: Arc::new(RwLock::new(OrphanPool::new())),
            consensus: Arc::clone(&consensus),
            tx_pool_config: Arc::new(config.clone()),
            block_assembler: None,
            txs_verify_cache: Arc::new(RwLock::new(init_cache())),
            callbacks: Arc::new(Callbacks::new()),
            network: network(&consensus),
            tx_relay_sender,
            verify_queue: Arc::new(RwLock::new(VerifyQueue::new(config.max_tx_verify_cycles))),
            block_assembler_sender,
            fee_estimator: FeeEstimator::new_dummy(),
        },
        tx_relay_receiver,
    )
}

#[tokio::test]
async fn process_orphan_tx_keeps_high_cycle_orphan_when_verify_queue_is_full() {
    let service = service();
    let parent = build_tx(vec![], 1);
    let orphan = build_tx(vec![(&parent.hash(), 0)], 1);
    let orphan_id = orphan.proposal_short_id();

    service
        .add_orphan(orphan.clone(), 1.into(), MAX_TX_VERIFY_CYCLES + 1)
        .await;
    service
        .verify_queue
        .write()
        .await
        .set_total_tx_size_for_test(256_000_000 - 1);

    let service_clone = service.clone();
    let handle = tokio::spawn(async move {
        service_clone.process_orphan_tx(&parent).await;
    });
    assert!(
        handle.await.is_ok(),
        "full verify queue should not panic while requeueing a high-cycle orphan"
    );

    assert!(service.orphan.read().await.contains_key(&orphan_id));
    assert!(!service.verify_queue.read().await.contains_key(&orphan_id));
}

#[tokio::test]
async fn submit_remote_tx_notifies_relayer_when_verify_queue_is_full() {
    let (service, tx_relay_receiver) = service_with_relay_receiver();
    let tx = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    let tx_hash = tx.hash();

    service
        .verify_queue
        .write()
        .await
        .set_total_tx_size_for_test(256_000_000 - 1);

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
async fn notify_tx_notifies_relayer_when_verify_queue_is_full() {
    let (service, tx_relay_receiver) = service_with_relay_receiver();
    let tx = build_tx(vec![(&H256([1; 32]).into(), 0)], 1);
    let tx_hash = tx.hash();

    service
        .verify_queue
        .write()
        .await
        .set_total_tx_size_for_test(256_000_000 - 1);

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
