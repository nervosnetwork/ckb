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
        while !controller.gate.is_closed() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(!pool.config.persisted_data.with_extension("v2").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_joins_a_request_waiting_for_verification_resume() {
    let directory = tempfile::tempdir().unwrap();
    let (builder, controller, _relay) = service(directory.path());
    let pool = builder.pool_for_test();
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    started(&controller).await;
    controller.suspend_chunk_process().unwrap();

    let submitter = controller.clone();
    let mut submission = tokio::spawn(async move {
        submitter
            .submit_local_tx_async(TransactionBuilder::default().build())
            .await
    });
    within(async {
        while controller.gate.ordinary_available() == crate::service::DEFAULT_CHANNEL_SIZE {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Reserved reads stay responsive while the local submission awaits resume.
    let reader = controller.clone();
    within(tokio::task::spawn_blocking(move || {
        reader.get_tx_pool_info().unwrap()
    }))
    .await
    .unwrap();
    assert!(futures_util::poll!(&mut submission).is_pending());

    controller.stop();
    within(generation).await.unwrap();
    assert!(within(submission).await.unwrap().is_err());
    assert!(!pool.is_faulted());
    assert!(!controller.service_started());
    assert!(controller.gate.is_closed());
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
        assert!(controller.gate.is_closed());
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
    assert!(controller.gate.is_closed());
    assert!(retained.upgrade().is_none(), "rejected payload is released");
    assert!(controller.continue_chunk_process().is_err());
    assert!(!pool.persistence_eligible());
    assert_eq!(std::fs::read(persisted).unwrap(), before);

    let retry = tokio::task::spawn_blocking(move || {
        controller.update_tx_pool_for_reorg(Default::default(), Default::default(), next_snapshot())
    });
    assert!(within(retry).await.unwrap().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_batch_admitted_work_runs_when_response_unpolled_dropped_or_stopped_mid_batch() {
    use crate::authority::service::Error;
    use crate::service::DEFAULT_CHANNEL_SIZE;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let make_batch = |base: u32, count: u32| -> Vec<(ckb_types::core::TransactionView, u64)> {
        (0..count)
            .map(|offset| {
                (
                    TransactionBuilder::default()
                        .input(ckb_types::packed::CellInput::new(
                            ckb_types::packed::OutPoint::new(
                                ckb_types::packed::Byte32::zero(),
                                base + offset,
                            ),
                            0,
                        ))
                        .output(ckb_types::packed::CellOutput::default())
                        .output_data(ckb_types::packed::Bytes::default())
                        .build(),
                    0,
                )
            })
            .collect()
    };

    // 1. Holding the response future unpolled must not block execution, pin
    //    permits, or stall graceful shutdown & persistence.
    {
        let directory = tempfile::tempdir().unwrap();
        let (builder, controller, _relay) = service(directory.path());
        let pool = builder.pool_for_test();
        let generation = builder.start_with_handle(DummyTxPoolNetwork);
        started(&controller).await;

        let batch = make_batch(10_000, 40);
        let expected: BTreeSet<_> = batch.iter().map(|(tx, _)| tx.hash()).collect();
        let unpolled_response = controller
            .submit_remote_txs(batch, 1.into())
            .expect("batch is admitted");

        within(async {
            while controller.gate.ordinary_available() != DEFAULT_CHANNEL_SIZE {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert_eq!(pool.owner_hashes(), expected);

        // Shut down while the caller STILL holds the response future unpolled.
        controller.stop();
        within(generation).await.unwrap();
        assert!(!pool.is_faulted());
        assert!(pool.persistence_eligible());
        assert!(pool.config.persisted_data.with_extension("v2").exists());
        drop(unpolled_response);
    }

    // 2. Dropping the response future immediately after admission must not
    //    cancel the service-owned batch.
    {
        let directory = tempfile::tempdir().unwrap();
        let (builder, controller, _relay) = service(directory.path());
        let pool = builder.pool_for_test();
        let generation = builder.start_with_handle(DummyTxPoolNetwork);
        started(&controller).await;

        let batch = make_batch(20_000, 40);
        let expected: BTreeSet<_> = batch.iter().map(|(tx, _)| tx.hash()).collect();
        let dropped_response = controller
            .submit_remote_txs(batch, 2.into())
            .expect("batch is admitted");
        drop(dropped_response);

        within(async {
            while controller.gate.ordinary_available() != DEFAULT_CHANNEL_SIZE {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert_eq!(pool.owner_hashes(), expected);

        controller.stop();
        within(generation).await.unwrap();
        assert!(!pool.is_faulted());
        assert!(pool.persistence_eligible());
    }

    // 3. Shutdown mid-batch drains promptly and reports the exact prefix of
    //    transactions processed before the service stopped.
    {
        let directory = tempfile::tempdir().unwrap();
        let (builder, controller, _relay) = service(directory.path());
        let pool = builder.pool_for_test();
        let generation = builder.start_with_handle(DummyTxPoolNetwork);
        started(&controller).await;

        let batch = make_batch(30_000, 64);
        let first_chunk: BTreeSet<_> = batch[..32].iter().map(|(tx, _)| tx.hash()).collect();
        let second_chunk: BTreeSet<_> = batch[32..].iter().map(|(tx, _)| tx.hash()).collect();

        let commits = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(ckb_util::parking_lot::Mutex::new(release_rx));
        pool.set_before_commit_hook(Some(Arc::new(move || {
            if commits.fetch_add(1, Ordering::SeqCst) == 0 {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .recv_timeout(Duration::from_secs(10))
                    .expect("test releases chunk 1 commit");
            }
        })));

        let response = controller
            .submit_remote_txs(batch, 3.into())
            .expect("batch is admitted");

        // Wait until chunk 1 (32 txs) reaches commit, then stop the service so
        // chunk 1 completes while chunk 2 observes `Error::Closed`.
        within(tokio::task::spawn_blocking(move || {
            entered_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("chunk 1 reaches before_commit");
        }))
        .await
        .unwrap();
        controller.stop();
        release_tx.send(()).unwrap();

        let outcome = within(response).await.unwrap();
        within(generation).await.unwrap();
        pool.set_before_commit_hook(None);

        let (offered, completed, error) = outcome.into_parts();
        assert_eq!(offered, 64);
        assert_eq!(completed, 32);
        assert!(matches!(
            error.as_ref().and_then(|err| err.downcast_ref::<Error>()),
            Some(Error::Closed)
        ));
        let stored = pool.owner_hashes();
        assert_eq!(stored, first_chunk);
        assert!(stored.is_disjoint(&second_chunk));
        assert!(!pool.is_faulted());
        assert!(pool.persistence_eligible());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_replay_callback_reads_succeed_while_external_read_slots_are_saturated() {
    use crate::authority::tests::common::{chain_store, funded_parent, funded_tx};
    use crate::service::READ_CHANNEL_SIZE;
    use ckb_store::attach_block_cell;
    use ckb_test_chain_utils::always_success_consensus;
    use ckb_types::packed::OutPoint;

    let directory = tempfile::tempdir().unwrap();
    let consensus = Arc::new(always_success_consensus());
    let (mock_store, base_snapshot) = chain_store(Arc::clone(&consensus));
    let funding = funded_parent(50_000, 20_000_000_000);
    let funding_block = BlockBuilder::default()
        .number(1)
        .epoch(base_snapshot.epoch_ext().number_with_fraction(1))
        .parent_hash(base_snapshot.tip_hash())
        .transaction(funding.clone())
        .build();
    {
        let db_txn = mock_store.store().begin_transaction();
        db_txn.insert_block(&funding_block).unwrap();
        db_txn.attach_block(&funding_block).unwrap();
        attach_block_cell(&db_txn, &funding_block).unwrap();
        db_txn.commit().unwrap();
    }
    let snapshot = Arc::new(Snapshot::new(
        funding_block.header(),
        base_snapshot.total_difficulty().clone(),
        base_snapshot.epoch_ext().clone(),
        mock_store.store().get_snapshot(),
        Default::default(),
        consensus,
    ));
    let replay_tx = funded_tx(OutPoint::new(funding.hash(), 0), 19_999_999_000);
    let persisted_path = directory.path().join("pool");
    write_snapshot(
        &persisted_path,
        PersistenceSnapshot {
            accepted: vec![replay_tx.clone()],
            recovery: Vec::new(),
        },
    )
    .unwrap();

    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (mut builder, controller, _relay) = TxPoolServiceBuilder::new(
        TxPoolConfig {
            max_tx_verify_workers: 1,
            persisted_data: persisted_path,
            recent_reject: Default::default(),
            ..TxPoolConfig::default()
        },
        snapshot,
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();

    let (cb_tx, cb_rx) = tokio::sync::oneshot::channel();
    let cb_tx = Arc::new(ckb_util::parking_lot::Mutex::new(Some(cb_tx)));
    let cb_controller = controller.clone();
    builder.register_pending(Box::new(move |entry| {
        if entry.transaction.hash() == replay_tx.hash()
            && let Some(sender) = cb_tx.lock().take()
        {
            let prestart = !cb_controller.service_started();
            let info = cb_controller.get_tx_pool_info();
            let _ = sender.send((prestart, info));
        }
    }));

    // Saturate all external read slots with pre-start queries awaiting readiness.
    let mut external_readers = Vec::with_capacity(READ_CHANNEL_SIZE);
    for _ in 0..READ_CHANNEL_SIZE {
        let client = controller.clone();
        external_readers.push(tokio::task::spawn_blocking(move || {
            client.get_tx_pool_info()
        }));
    }
    within(async {
        while controller.gate.read_available() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;

    // An additional external pre-start read must fail fast (bounded waiting).
    let overflow_err = controller
        .get_tx_pool_info()
        .expect_err("external pre-start reads stay bounded by READ_CHANNEL_SIZE");
    assert!(
        overflow_err.to_string().contains("no available capacity"),
        "{overflow_err}"
    );

    // Start the service: replay callback must succeed while all 16 external
    // read slots are occupied, and then gate.open() releases all 16 waiters.
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    let (was_prestart, callback_info) = within(cb_rx).await.unwrap();
    assert!(
        was_prestart,
        "replay callback must execute before external readiness opens"
    );
    let callback_info = callback_info.expect("replay callback read must succeed");
    assert_eq!(callback_info.pending_size, 1);

    for reader in external_readers {
        let info = within(reader).await.unwrap().unwrap();
        assert_eq!(info.pending_size, 1);
    }

    controller.stop();
    within(generation).await.unwrap();
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hot_block_template_respects_ordinary_capacity_drain_deadline_and_callback_isolation() {
    use crate::authority::tests::common::{template_config, template_snapshot_with_child};
    use crate::service::{DEFAULT_CHANNEL_SIZE, READ_CHANNEL_SIZE};

    let directory = tempfile::tempdir().unwrap();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (builder, controller, _relay) = TxPoolServiceBuilder::new(
        TxPoolConfig {
            max_tx_verify_workers: 1,
            persisted_data: directory.path().join("pool"),
            recent_reject: Default::default(),
            ..TxPoolConfig::default()
        },
        template_snapshot_with_child(None),
        Some(template_config(100)),
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    started(&controller).await;

    // Warm the block template cache so subsequent requests take the hot path.
    let warm_client = controller.clone();
    within(tokio::task::spawn_blocking(move || {
        warm_client
            .get_block_template(None, None, None)
            .unwrap()
            .unwrap()
    }))
    .await
    .unwrap();

    // 1. Saturate ordinary admission slots: hot template hit must be rejected.
    let mut ordinary_permits = Vec::with_capacity(DEFAULT_CHANNEL_SIZE);
    while let Some(permit) = controller.gate.try_reserve_ordinary() {
        ordinary_permits.push(permit);
    }
    assert_eq!(controller.gate.ordinary_available(), 0);
    let full_err = controller
        .get_block_template(None, None, None)
        .expect_err("hot template hit must obey ordinary capacity limits");
    assert!(
        full_err.to_string().contains("no available capacity"),
        "{full_err}"
    );

    // 2. Even with both ordinary and external read lanes saturated, a nested
    //    callback retains its template read capability.
    let mut read_permits = Vec::with_capacity(READ_CHANNEL_SIZE);
    while let Some(permit) = controller.gate.try_reserve_read() {
        read_permits.push(permit);
    }
    let cb_client = controller.clone();
    let cb_template = within(tokio::task::spawn_blocking(move || {
        crate::callback::with_callback_context(|| cb_client.get_block_template(None, None, None))
    }))
    .await
    .unwrap()
    .expect("callback template read bypasses saturated ordinary/external lanes")
    .expect("cached template is valid");
    assert!(u64::from(cb_template.number) >= 1);
    drop(read_permits);
    drop(ordinary_permits);

    // 3. Hot template hit must be tracked by the shutdown drain counter.
    let write_lock = pool.lock_state_write_for_test();
    let hot_client = controller.clone();
    let hot_call =
        tokio::task::spawn_blocking(move || hot_client.get_block_template(None, None, None));
    within(async {
        while controller.gate.inflight() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(controller.gate.inflight(), 1);
    assert_eq!(
        controller.gate.ordinary_available(),
        DEFAULT_CHANNEL_SIZE - 1
    );
    drop(write_lock);
    within(hot_call).await.unwrap().unwrap().unwrap();
    assert_eq!(controller.gate.inflight(), 0);

    // 4. An already-expired deadline on the hot path returns timeout error, and
    //    a deadline expiring while waiting to validate the cache also times out
    //    promptly via the controller before the write lock is released.
    let expired = pool.current_block_template(tokio::time::Instant::now());
    assert!(
        matches!(expired, Err(crate::authority::service::Error::Full(_))),
        "expired deadline must reject even a hot cached template"
    );
    let write_lock = pool.lock_state_write_for_test();
    let timed_client = controller.clone();
    let started_at = std::time::Instant::now();
    let timeout_during_validation = within(tokio::task::spawn_blocking(move || {
        timed_client.get_block_template_at(
            tokio::time::Instant::now() + std::time::Duration::from_millis(40),
        )
    }))
    .await
    .unwrap();
    let elapsed = started_at.elapsed();
    drop(write_lock);
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "controller must time out at deadline before write lock release, elapsed={elapsed:?}"
    );
    assert!(
        matches!(timeout_during_validation, Ok(Err(_))),
        "deadline expiring during cache validation must return timeout error: {timeout_during_validation:?}"
    );

    // 5. After stop/drain, hot template returns channel closed.
    controller.stop();
    within(generation).await.unwrap();
    let closed_err = controller
        .get_block_template(None, None, None)
        .expect_err("closed gate rejects hot template reads");
    assert!(
        closed_err.to_string().contains("channel closed"),
        "{closed_err}"
    );
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hot_template_response_construction_enforces_deadline_and_reclaims_permit() {
    use crate::authority::tests::common::{chain_store, funded_tx, template_config};
    use crate::service::{DEFAULT_CHANNEL_SIZE, READ_CHANNEL_SIZE};
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_proposal_table::ProposalView;
    use ckb_test_chain_utils::{always_success_cell, always_success_consensus};
    use ckb_types::{
        U256,
        bytes::Bytes,
        core::{Capacity, FeeRate},
        packed::{CellOutput, OutPoint},
        prelude::*,
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    const TEMPLATE_TX_COUNT: usize = 1_500;
    const OUTPUT_CAPACITY: u64 = 20_000_000_000;
    const SPEND_CAPACITY: u64 = 19_000_000_000;

    // Build a real consensus + genesis containing 1,500 independent always-success
    // funding cells and mark all 1,500 child transactions as proposed so the hot
    // template packs all 1,500 transactions cleanly.
    let (_, _, always_success_script) = always_success_cell();
    let funding_cell = CellOutput::new_builder()
        .capacity(Capacity::shannons(OUTPUT_CAPACITY))
        .lock(always_success_script.clone())
        .build();
    let mut funding_builder = TransactionBuilder::default();
    for _ in 0..TEMPLATE_TX_COUNT {
        funding_builder = funding_builder
            .output(funding_cell.clone())
            .output_data(Bytes::new().pack());
    }
    let funding_tx = funding_builder.build();
    let genesis = always_success_consensus()
        .genesis_block()
        .as_advanced_builder()
        .transaction(funding_tx.clone())
        .build();
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .genesis_block(genesis)
            .max_block_bytes(2_000_000)
            .max_block_cycles(100_000_000_000)
            .build(),
    );
    let (mock_store, base_snapshot) = chain_store(Arc::clone(&consensus));

    let txs: Vec<_> = (0..TEMPLATE_TX_COUNT as u32)
        .map(|idx| funded_tx(OutPoint::new(funding_tx.hash(), idx), SPEND_CAPACITY))
        .collect();
    let proposed_ids: HashSet<_> = txs.iter().map(|tx| tx.proposal_short_id()).collect();
    let proposals = ProposalView::new(HashSet::new(), proposed_ids);
    let snapshot = Arc::new(Snapshot::new(
        base_snapshot.tip_header().clone(),
        U256::zero(),
        base_snapshot.epoch_ext().clone(),
        mock_store.store().get_snapshot(),
        proposals,
        consensus,
    ));

    let directory = tempfile::tempdir().unwrap();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (builder, controller, _relay) = TxPoolServiceBuilder::new(
        TxPoolConfig {
            max_tx_verify_workers: 2,
            min_fee_rate: FeeRate::zero(),
            max_tx_pool_size: 16_000_000,
            persisted_data: directory.path().join("pool"),
            recent_reject: Default::default(),
            ..TxPoolConfig::default()
        },
        snapshot,
        Some(template_config(50)),
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    started(&controller).await;

    // Submit all 1,500 transactions via the public controller API.
    for tx in &txs {
        within(controller.submit_local_tx_async(tx.clone()))
            .await
            .unwrap()
            .unwrap();
    }

    // Warm the block template cache until it contains all 1,500 transactions.
    let warmed = within(async {
        loop {
            let client = controller.clone();
            let tpl = tokio::task::spawn_blocking(move || {
                client
                    .get_block_template(None, None, None)
                    .unwrap()
                    .unwrap()
            })
            .await
            .unwrap();
            if tpl.transactions.len() == TEMPLATE_TX_COUNT {
                break tpl;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert_eq!(warmed.transactions.len(), TEMPLATE_TX_COUNT);
    let expected_hashes: HashSet<_> = txs.iter().map(|tx| tx.hash().unpack()).collect();

    // Pause response construction on the first call AFTER the 1,500-tx hot
    // template passes cache validation, using explicit sync channels rather
    // than microsecond timing assumptions.
    let armed = Arc::new(AtomicBool::new(true));
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let release_rx = Arc::new(ckb_util::parking_lot::Mutex::new(release_rx));
    let (resumed_tx, resumed_rx) = std::sync::mpsc::channel::<()>();
    pool.set_template_response_hook(Some(Arc::new({
        let armed = Arc::clone(&armed);
        let release_rx = Arc::clone(&release_rx);
        move || {
            if armed.swap(false, Ordering::SeqCst) {
                let _ = entered_tx.send(());
                let _ = release_rx.lock().recv();
                let _ = resumed_tx.send(());
            }
        }
    })));

    // 1. Start public get_block_template_at with a short deadline and wait for
    //    confirmation that cache validation passed and response construction is
    //    now paused at our sync signal.
    let timed_client = controller.clone();
    let timed_call = tokio::task::spawn_blocking(move || {
        timed_client.get_block_template_at(tokio::time::Instant::now() + Duration::from_millis(80))
    });
    within(tokio::task::spawn_blocking(move || {
        entered_rx
            .recv()
            .expect("hot 1,500-tx template must pass validation and enter response construction");
    }))
    .await
    .unwrap();
    assert_eq!(controller.gate.inflight(), 1);
    assert_eq!(
        controller.gate.ordinary_available(),
        DEFAULT_CHANNEL_SIZE - 1
    );

    // 2. BEFORE releasing response construction, the caller must independently
    //    time out with the original timeout error.
    let timed_result = within(timed_call).await.unwrap();
    let timeout_err = timed_result
        .expect("public controller must deliver inner timeout result")
        .expect_err("paused response construction must time out at caller deadline");
    assert!(
        timeout_err.to_string().contains("template refresh timeout"),
        "expected original template refresh timeout error, got: {timeout_err}"
    );

    // 3. While the late construction task is still paused after caller timeout,
    //    the executor must keep holding its admission/drain permit, AND the
    //    CallbackRead lane must remain isolated even if ordinary/read lanes fill.
    assert_eq!(
        controller.gate.inflight(),
        1,
        "executor must retain drain permit until late construction finishes"
    );
    assert_eq!(
        controller.gate.ordinary_available(),
        DEFAULT_CHANNEL_SIZE - 1,
        "executor must retain ordinary admission permit until late construction finishes"
    );
    let mut ordinary_permits = Vec::with_capacity(DEFAULT_CHANNEL_SIZE - 1);
    while let Some(permit) = controller.gate.try_reserve_ordinary() {
        ordinary_permits.push(permit);
    }
    assert_eq!(ordinary_permits.len(), DEFAULT_CHANNEL_SIZE - 1);
    let mut read_permits = Vec::with_capacity(READ_CHANNEL_SIZE);
    while let Some(permit) = controller.gate.try_reserve_read() {
        read_permits.push(permit);
    }
    let cb_client = controller.clone();
    let cb_tpl = within(tokio::task::spawn_blocking(move || {
        crate::callback::with_callback_context(|| cb_client.get_block_template(None, None, None))
    }))
    .await
    .unwrap()
    .expect("callback template read stays isolated while late ordinary build runs")
    .expect("1,500-tx hot template remains valid");
    assert_eq!(cb_tpl.transactions.len(), TEMPLATE_TX_COUNT);
    drop(read_permits);
    drop(ordinary_permits);

    // 4. Release the paused response construction, confirm it resumes, discards
    //    the late template cleanly, and reclaims the admission/drain permit.
    release_tx.send(()).unwrap();
    within(tokio::task::spawn_blocking(move || {
        resumed_rx
            .recv()
            .expect("released response construction must run to completion");
    }))
    .await
    .unwrap();
    within(async {
        while controller.gate.inflight() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(controller.gate.inflight(), 0);
    assert_eq!(controller.gate.ordinary_available(), DEFAULT_CHANNEL_SIZE);
    assert!(!pool.is_faulted());

    // 5. With sufficient budget, the exact same 1,500-tx hot template returns
    //    complete contents normally via the public API.
    pool.set_template_response_hook(None);
    let ok_client = controller.clone();
    let full_tpl = within(tokio::task::spawn_blocking(move || {
        ok_client.get_block_template(None, None, None)
    }))
    .await
    .unwrap()
    .expect("sufficient budget succeeds")
    .expect("1,500-tx template returns cleanly");
    assert_eq!(full_tpl.transactions.len(), TEMPLATE_TX_COUNT);
    let actual_hashes: HashSet<_> = full_tpl
        .transactions
        .iter()
        .map(|tx| tx.hash.clone())
        .collect();
    assert_eq!(actual_hashes, expected_hashes);

    controller.stop();
    within(generation).await.unwrap();
    assert!(!pool.is_faulted());
}
