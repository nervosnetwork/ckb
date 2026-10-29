//! The real builder owns pre-start requests until it starts or is dropped.

use crate::{
    network::DummyTxPoolNetwork,
    persisted::{PersistenceSnapshot, write_snapshot},
    service::{
        BoundedTransaction, TxPoolController, TxPoolServiceBuilder, TxVerificationResultReceiver,
    },
    test_support::genesis_snapshot,
};
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::core::{BlockBuilder, BlockView, TransactionBuilder};
use ckb_verification::cache::init_cache;
use std::{collections::VecDeque, future::Future, path::Path, sync::Arc, time::Duration};
use tokio::{sync::RwLock, task::JoinHandle};

fn service(
    directory: &Path,
) -> (
    TxPoolServiceBuilder,
    TxPoolController,
    TxVerificationResultReceiver,
) {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    TxPoolServiceBuilder::new(
        TxPoolConfig {
            max_tx_verify_workers: 0,
            persisted_data: directory.join("pool"),
            recent_reject: Default::default(),
            ..TxPoolConfig::default()
        },
        genesis_snapshot(),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap()
}

fn next_snapshot() -> Arc<Snapshot> {
    let previous = genesis_snapshot();
    let tip = BlockBuilder::default()
        .number(1)
        .epoch(previous.epoch_ext().number_with_fraction(1))
        .parent_hash(previous.tip_hash())
        .build()
        .header();
    Arc::new(Snapshot::new(
        tip,
        previous.total_difficulty().clone(),
        previous.epoch_ext().clone(),
        MockStore::default().store().get_snapshot(),
        Default::default(),
        previous.cloned_consensus(),
    ))
}

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("lifecycle event completes")
}

async fn started(controller: &TxPoolController) {
    within(async {
        while !controller.service_started() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Exercise both lanes before abandoning their owner.
    let client = controller.clone();
    within(tokio::task::spawn_blocking(move || {
        client.get_tx_pool_info().unwrap();
        assert!(
            client
                .submit_local_tx(TransactionBuilder::default().build())
                .unwrap()
                .is_err()
        );
    }))
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn aborting_the_builder_cancels_workers_and_releases_publication_endpoints() {
    let directory = tempfile::tempdir().unwrap();
    let (mut builder, controller, _relay) = service(directory.path());
    let pool = builder.pool_for_test();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    builder.register_pending(Box::new(move |_| {
        let _ = &release;
    }));
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    started(&controller).await;

    generation.abort();
    assert!(within(generation).await.unwrap_err().is_cancelled());
    assert!(!controller.service_started());
    assert!(pool.is_stopped());
    assert!(pool.is_faulted());
    assert!(within(released).await.is_err(), "the endpoint was released");
    within(async {
        while !controller.sender.is_closed() || !controller.query_sender.is_closed() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(!pool.config.persisted_data.with_extension("v2").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn stopping_joins_a_request_waiting_for_verification_resume() {
    use crate::service::{Message, Request};
    let directory = tempfile::tempdir().unwrap();
    let (builder, controller, _relay) = service(directory.path());
    let pool = builder.pool_for_test();
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    started(&controller).await;
    controller.suspend_chunk_process().unwrap();
    let (responder, response) = ckb_channel::oneshot::channel();
    controller
        .sender
        .try_send(Message::SubmitLocalTx(Request::call(
            BoundedTransaction::try_new(TransactionBuilder::default().build()).unwrap(),
            responder,
        )))
        .unwrap();
    // A later message on the same FIFO lane proves the earlier request has
    // been taken by a worker. Its result must still await verification resume.
    let (responder, barrier) = ckb_channel::oneshot::channel();
    controller
        .sender
        .try_send(Message::GetTxPoolInfo(Request::call((), responder)))
        .unwrap();
    within(tokio::task::spawn_blocking(move || {
        barrier.recv().unwrap().unwrap()
    }))
    .await
    .unwrap();
    assert!(matches!(
        response.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));

    controller.stop();
    within(generation).await.unwrap();
    assert!(response.recv().unwrap().is_err());
    assert!(!pool.is_faulted());
    assert!(!controller.service_started());
}

#[test]
fn runtime_drop_releases_idle_request_workers() {
    idle_runtime_shutdown(None);
}

#[test]
fn idle_request_workers_do_not_exhaust_runtime_blocking_capacity() {
    idle_runtime_shutdown(Some(1));
}

fn idle_runtime_shutdown(blocking_threads: Option<usize>) {
    let (finished, completion) = std::sync::mpsc::channel();
    let owner = std::thread::spawn(move || {
        let directory = tempfile::tempdir().unwrap();
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.worker_threads(1).enable_all();
        if let Some(threads) = blocking_threads {
            builder.max_blocking_threads(threads);
        }
        let runtime = builder.build().unwrap();
        let (controller, retained) = runtime.block_on(async {
            let (builder, controller, _relay) = service(directory.path());
            let retained = Arc::downgrade(&builder.pool_for_test());
            drop(builder.start_with_handle(DummyTxPoolNetwork));
            started(&controller).await;
            (controller, retained)
        });
        drop(runtime);
        assert!(retained.upgrade().is_none());
        assert!(!controller.service_started());
        assert!(controller.sender.is_closed());
        assert!(controller.query_sender.is_closed());
        finished.send(()).unwrap();
    });
    completion
        .recv_timeout(Duration::from_secs(10))
        .expect("runtime drop joins idle request workers");
    owner.join().unwrap();
}

async fn queue_reorg(
    controller: &TxPoolController,
    attached_blocks: VecDeque<BlockView>,
    snapshot: Arc<Snapshot>,
) -> JoinHandle<Result<(), AnyError>> {
    assert!(!controller.service_started());
    assert!(controller.accepts_chain_updates());
    let client = controller.clone();
    let request = tokio::task::spawn_blocking(move || {
        client.update_tx_pool_for_reorg(Default::default(), attached_blocks, snapshot)
    });
    // Observe the public sender owning the sole lane slot before changing
    // its receiver's lifecycle. Elapsed time does not establish admission.
    within(async {
        while controller.chain_control_sender.capacity() != 0 {
            assert!(!request.is_finished(), "pre-start reorg must wait");
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(!request.is_finished());
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builder_start_completes_a_prestart_chain_transition() {
    let directory = tempfile::tempdir().unwrap();
    let (builder, controller, _relay) = service(directory.path());
    let pool = builder.pool_for_test();
    let snapshot = next_snapshot();
    let expected_tip = snapshot.tip_hash();
    assert_ne!(pool.pool_info().await.unwrap().tip_hash, expected_tip);
    let request = queue_reorg(&controller, Default::default(), snapshot).await;

    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    within(request).await.unwrap().unwrap();
    assert_eq!(pool.pool_info().await.unwrap().tip_hash, expected_tip);

    controller.stop();
    within(generation).await.unwrap();
    assert!(!pool.is_faulted());
    assert!(!controller.service_started());
    assert!(!controller.accepts_chain_updates());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_an_unstarted_builder_releases_its_chain_request() {
    let directory = tempfile::tempdir().unwrap();
    let (builder, controller, _relay) = service(directory.path());
    let snapshot = next_snapshot();
    let retained = Arc::downgrade(&snapshot);
    let request = queue_reorg(&controller, Default::default(), snapshot).await;
    assert!(retained.upgrade().is_some());

    drop(builder);
    assert!(within(request).await.unwrap().is_err());
    assert!(!controller.accepts_chain_updates());
    assert!(
        retained.upgrade().is_none(),
        "abandoned payload is released"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_before_start_closes_the_chain_lane_and_prevents_resume() {
    let directory = tempfile::tempdir().unwrap();
    let (builder, controller, _relay) = service(directory.path());
    let pool = builder.pool_for_test();
    let request = queue_reorg(&controller, Default::default(), next_snapshot()).await;

    controller.stop();
    assert!(
        controller.accepts_chain_updates(),
        "the unstarted builder still owns its receiver after cancellation"
    );
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    within(generation).await.unwrap();
    // The chain consumer can finish the admitted command before shutdown or
    // close it without a response. Either outcome must release its caller.
    let _outcome = within(request).await.unwrap();
    assert!(pool.is_stopped());
    assert!(!pool.is_faulted());
    assert!(!controller.service_started());
    assert!(!controller.accepts_chain_updates());
    assert!(controller.continue_chunk_process().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_failure_during_builder_replay_releases_request_and_preserves_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let (builder, controller, _relay) = service(directory.path());
    let pool = builder.pool_for_test();
    let transaction = TransactionBuilder::default().build();
    assert!(BoundedTransaction::try_new(transaction.clone()).is_ok());
    write_snapshot(
        &pool.config.persisted_data,
        PersistenceSnapshot {
            accepted: vec![transaction],
            recovery: Vec::new(),
        },
    )
    .unwrap();
    let persisted = pool.config.persisted_data.with_extension("v2");
    let before = std::fs::read(&persisted).unwrap();
    // A nonempty bounded replay must acquire computation capacity before even
    // checking validity. Suspension prevents successful startup, including a
    // transient ready state that a final service_started() check would miss.
    controller.suspend_chunk_process().unwrap();

    // Deliberately violate the caller contract to exercise the real structural
    // error gate and its cleanup; a valid chain update does not have this shape.
    let snapshot = next_snapshot();
    let attached = BlockBuilder::default().build();
    assert_ne!(attached.hash(), snapshot.tip_hash());
    assert_ne!(
        pool.pool_info().await.unwrap().tip_hash,
        snapshot.tip_hash()
    );
    let retained = Arc::downgrade(&snapshot);
    let request = queue_reorg(&controller, VecDeque::from([attached]), snapshot).await;
    assert!(retained.upgrade().is_some());

    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    assert!(within(request).await.unwrap().is_err());
    within(generation).await.unwrap();

    assert!(pool.is_faulted());
    assert!(pool.is_stopped());
    assert!(!controller.service_started());
    assert!(!controller.accepts_chain_updates());
    assert!(controller.sender.is_closed());
    assert!(controller.query_sender.is_closed());
    assert!(retained.upgrade().is_none(), "rejected payload is released");
    assert!(controller.continue_chunk_process().is_err());
    assert!(!pool.persistence_eligible());
    assert_eq!(std::fs::read(persisted).unwrap(), before);

    let retry = tokio::task::spawn_blocking(move || {
        controller.update_tx_pool_for_reorg(Default::default(), Default::default(), next_snapshot())
    });
    assert!(within(retry).await.unwrap().is_err());
}
