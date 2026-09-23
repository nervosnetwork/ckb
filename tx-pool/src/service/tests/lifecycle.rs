//! The real builder owns pre-start requests until it starts or is dropped.

use crate::{
    network::DummyTxPoolNetwork,
    service::{TxPoolController, TxPoolServiceBuilder, TxVerificationResultReceiver},
    test_support::genesis_snapshot,
};
use ckb_app_config::TxPoolConfig;
use ckb_async_runtime::Handle;
use ckb_error::AnyError;
use ckb_fee_estimator::FeeEstimator;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::core::BlockBuilder;
use ckb_verification::cache::init_cache;
use std::{future::Future, path::Path, sync::Arc, time::Duration};
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

async fn queue_reorg(
    controller: &TxPoolController,
    snapshot: Arc<Snapshot>,
) -> JoinHandle<Result<(), AnyError>> {
    assert!(!controller.service_started());
    assert!(controller.accepts_chain_updates());
    let client = controller.clone();
    let request = tokio::task::spawn_blocking(move || {
        client.update_tx_pool_for_reorg(Default::default(), Default::default(), snapshot)
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
    let request = queue_reorg(&controller, snapshot).await;

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
    let request = queue_reorg(&controller, snapshot).await;
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
    let request = queue_reorg(&controller, next_snapshot()).await;

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
