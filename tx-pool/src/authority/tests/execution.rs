use super::*;
use crate::{
    authority::{model::Status, tests::common::*},
    callback::Callbacks,
    network::DummyTxPoolNetwork,
};
use ckb_types::packed::OutPoint;
use ckb_util::Mutex;
use ckb_verification::cache::init_cache;
use std::collections::BTreeSet;
use std::future::Future;

#[path = "stability.rs"]
mod stability;

fn fixture() -> (Arc<Pool>, RelaySink, RelayDrain, Handle) {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let store = store_with_pipeline_limit(chain_snapshot(), &config(), 64_000_000);
    let (pool, sink, drain) = Pool::with_store(
        config(),
        store,
        &handle,
        Arc::new(RwLock::new(init_cache())),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    (pool, sink, drain, handle)
}
fn bounded(tx: TransactionView) -> BoundedTransaction {
    BoundedTransaction::try_new(tx).unwrap()
}
fn fund(pool: &Pool, nonce: u32) -> TransactionView {
    let parent = funded_parent(nonce, 20_000_000_000);
    accept(&pool.store, parent.clone(), 1, 1, Status::Pending);
    let owner = pool.store.point(&parent.hash()).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    value.timestamp = ckb_systemtime::unix_time_as_millis();
    replace(&pool.store, owner, Phase::Accepted(value));
    funded_tx(OutPoint::new(parent.hash(), 0), 19_999_999_000)
}
fn endpoints(sink: RelaySink, callbacks: Callbacks) -> Endpoints {
    Endpoints::new(
        Arc::new(DummyTxPoolNetwork),
        sink,
        Arc::new(callbacks),
        None,
        FeeEstimator::new_dummy(),
    )
}
async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("bounded event completion")
}
async fn observe(pool: &Pool, hash: &Byte32, check: impl Fn(&Entry) -> bool) {
    within(async {
        loop {
            let changed = pool.store.changed.notified();
            // These observations follow worker phase transitions. A completed
            // Job releases active capacity after its new owner is committed;
            // Store.changed also preserves fault and lifecycle observation.
            let completed = pool.store.budget.changed.notified();
            tokio::pin!(changed, completed);
            changed.as_mut().enable();
            if pool.store.point(hash).1.as_deref().is_some_and(&check) {
                break;
            }
            assert!(!pool.is_faulted());
            tokio::select! { _ = changed => {}, _ = completed => {} }
        }
    })
    .await;
}
async fn shutdown(
    pool: &Pool,
    mut tasks: JoinSet<Result<(), Error>>,
    publisher: JoinHandle<Result<(), Error>>,
) {
    pool.stop();
    within(async {
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
    })
    .await;
    pool.close_outbox();
    within(publisher).await.unwrap().unwrap();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_observes_notice_refund_before_its_first_wait_poll() {
    let (pool, sink, drain, _) = fixture();
    let hash = tx(4110).hash();
    let effect = || {
        Effect::relay(TxVerificationResult::Reject {
            tx_hash: hash.clone(),
        })
    };
    let mut reservations: Vec<_> = (0..crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES)
        .map(|_| {
            pool.store
                .outbox
                .reserve(vec![effect()], Class::Remote)
                .unwrap()
                .unwrap()
        })
        .collect();
    let mut attempts = 0;
    let batch = within(pool.commit(|| {
        attempts += 1;
        if attempts == 1 {
            let refused = pool
                .store
                .outbox
                .reserve(vec![effect()], Class::Remote)
                .err()
                .expect("remote notice capacity is occupied");
            assert!(matches!(refused, Error::Full(FullReason::NoticeOutbox)));
            // The real refund broadcasts after refusal but before commit can
            // poll its wait future. No later event is needed to resume it.
            drop(reservations.pop().unwrap());
            return Err(refused);
        }
        let mut plan = Plan::new(pool.store.snapshot().0, Class::Remote, Default::default());
        plan.notify(effect());
        Ok(plan)
    }))
    .await
    .unwrap()
    .unwrap();
    assert_eq!(attempts, 2);
    drop(reservations);
    pool.close_outbox();
    within(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())))
        .await
        .unwrap();
    within(batch.wait(&pool.store.outbox)).await.unwrap();
    assert!(
        matches!(drain.try_recv(), Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == hash)
    );
    assert!(drain.try_recv().is_none());
    pool.stop();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn commit_observes_chain_release_before_its_first_wait_poll() {
    let (pool, _, _, _) = fixture();
    let candidate = entry(&pool.store, tx(4111), Source::Local);
    let mut pause = Some(pool.store.begin_chain().unwrap());
    let mut attempts = 0;
    within(pool.commit(|| {
        attempts += 1;
        let mut plan = Plan::new(pool.store.snapshot().0, Class::Trusted, Default::default());
        plan.edit(None, Some(Arc::clone(&candidate)), None)?;
        if attempts == 1 {
            let refused = pool
                .store
                .apply(plan)
                .err()
                .expect("chain pause excludes owner edits");
            assert!(matches!(refused, Error::Full(FullReason::ChainTransition)));
            assert!(pool.store.point(&candidate.hash()).1.is_none());
            drop(pause.take());
            return Err(refused);
        }
        Ok(plan)
    }))
    .await
    .unwrap();
    assert_eq!(attempts, 2);
    assert!(Arc::ptr_eq(
        &pool.store.point(&candidate.hash()).1.unwrap(),
        &candidate
    ));
    pool.stop();
    pool.close_outbox();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_pipeline_resolver_progresses_while_eight_remote_jobs_hold_their_memory() {
    let mut configuration = config();
    configuration.max_tx_verify_workers = 8;
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 64_000_000);
    let (pool, sink, _drain) = Pool::with_store(
        configuration,
        store,
        &handle,
        Arc::new(RwLock::new(init_cache())),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let transaction = fund(&pool, 4112);
    pool.submit_remote(bounded(transaction.clone()), 1, 1.into())
        .await
        .unwrap();
    let held_jobs: Vec<_> = [1, 1, 2, 2, 3, 3, 4, 4]
        .into_iter()
        .map(|peer| {
            pool.store
                .budget
                .active(ingress::remote_source(peer.into(), 1).unwrap())
                .unwrap()
        })
        .collect();
    let resolver = tokio::spawn(Arc::clone(&pool).worker(WorkStage::Resolve, 0));
    // The production resolver must publish Verify before any of the eight
    // active memory reservations are returned. No verification worker runs.
    observe(&pool, &transaction.hash(), |entry| {
        matches!(entry.phase, Phase::Verify(_))
    })
    .await;
    pool.stop();
    within(resolver).await.unwrap().unwrap();
    drop(held_jobs);
    pool.close_outbox();
    within(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())))
        .await
        .unwrap();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_receive_resolve_verify_commit_publish_and_join() {
    let (pool, sink, drain, handle) = fixture();
    let tx = fund(&pool, 4001);
    let cycles = pool
        .submit_local(bounded(tx.clone()), true)
        .await
        .unwrap()
        .unwrap()
        .cycles;
    assert!(pool.store.point(&tx.hash()).1.is_none());
    let (sent, mut received) = mpsc::unbounded_channel();
    let expected = tx.hash();
    let mut callbacks = Callbacks::new();
    let current = Arc::clone(&pool);
    callbacks.register_pending(Box::new(move |entry| {
        if entry.transaction.hash() == expected {
            // This point read would deadlock if publication retained its owner write guard.
            assert!(
                current
                    .store
                    .point(&expected)
                    .1
                    .unwrap()
                    .accepted()
                    .is_some()
            );
            assert!(crate::callback::in_callback());
            sent.send(entry.transaction.hash()).unwrap();
        }
    }));
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) = pool.start_background(&handle, endpoints(sink, callbacks), chain);
    pool.submit_remote(bounded(tx.clone()), cycles, PeerIndex::from(41))
        .await
        .unwrap();
    assert_eq!(within(received.recv()).await.unwrap(), tx.hash());
    observe(&pool, &tx.hash(), |entry| entry.accepted().is_some()).await;
    shutdown(&pool, tasks, publisher).await;
    let mut success = false;
    while let Some(result) = drain.try_recv() {
        if let TxVerificationResult::Ok {
            original_peer: Some(peer),
            tx_hash,
        } = result
        {
            assert_eq!(peer, PeerIndex::from(41));
            assert_eq!(tx_hash, tx.hash());
            success = true;
        }
    }
    assert!(success);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn suspended_workers_keep_verification_queued_and_dry_run_declines_without_waiting() {
    let (pool, sink, _, handle) = fixture();
    let tx = fund(&pool, 4002);
    let cycles = pool
        .submit_local(bounded(tx.clone()), true)
        .await
        .unwrap()
        .unwrap()
        .cycles;
    pool.verification.suspend().unwrap();
    let (chain_sender, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    pool.submit_remote(bounded(tx.clone()), cycles, PeerIndex::from(42))
        .await
        .unwrap();
    assert!(matches!(
        pool.store.point(&tx.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
    assert!(matches!(
        within(pool.submit_local(bounded(tx.clone()), true))
            .await
            .unwrap(),
        Err(Reject::Full(_))
    ));
    // The block consumer waits for this lane. Suspending computation must not
    // suspend reconciliation or the publication required to acknowledge it.
    let (view, snapshot) = pool.store.snapshot();
    let (responder, response) = ckb_channel::oneshot::channel();
    chain_sender
        .send(ChainControl::Reconcile(Request {
            responder,
            arguments: ChainReorgArgs::Detailed {
                detached_blocks: Default::default(),
                attached_blocks: Default::default(),
                snapshot,
            },
        }))
        .await
        .unwrap();
    within(tokio::task::spawn_blocking(move || {
        response.recv_timeout(Duration::from_secs(5))
    }))
    .await
    .unwrap()
    .unwrap();
    assert!(pool.store.snapshot().0 > view);
    assert_eq!(within(pool.pool_info()).await.unwrap().verify_queue_size, 1);
    pool.verification.resume().unwrap();
    observe(&pool, &tx.hash(), |entry| entry.accepted().is_some()).await;
    shutdown(&pool, tasks, publisher).await;
    assert!(pool.verification.resume().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_during_compute_capacity_wait_refunds_permit_and_stop_wakes_waiters() {
    let (pool, _, _, _) = fixture();
    let capacity = pool.cpu.available_permits();
    let occupied = Arc::clone(&pool.cpu)
        .try_acquire_many_owned(capacity as u32)
        .unwrap();
    let mut compute = Box::pin(pool.compute());
    assert!(futures_util::poll!(compute.as_mut()).is_pending());
    pool.verification.suspend().unwrap();
    drop(occupied);
    assert!(futures_util::poll!(compute.as_mut()).is_pending());
    assert_eq!(pool.cpu.available_permits(), capacity);
    assert!(matches!(
        pool.direct_capacity(false).await,
        Err(Error::Full(FullReason::Other("verification is suspended")))
    ));
    assert_eq!(pool.cpu.available_permits(), capacity);

    // Resume is already published before the suspended future is polled again.
    pool.verification.resume().unwrap();
    drop(within(compute).await.unwrap());
    pool.verification.suspend().unwrap();
    let mut compute = Box::pin(pool.compute());
    assert!(futures_util::poll!(compute.as_mut()).is_pending());
    pool.stop();
    assert!(matches!(within(compute).await, Err(Error::Closed)));
    assert!(pool.verification.resume().is_err());
    assert_eq!(pool.cpu.available_permits(), capacity);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suspended_computation_leaves_resolver_verifier_and_local_submission_unstarted() {
    let (pool, _, _, _) = fixture();
    let queued = entry(&pool.store, tx(4007), Source::Local);
    insert(&pool.store, Arc::clone(&queued));
    let local = fund(&pool, 4008);
    pool.verification.suspend().unwrap();
    let capacity = pool.cpu.available_permits();
    let mut resolver = Box::pin(Arc::clone(&pool).worker(WorkStage::Resolve, 0));
    let mut verifier = Box::pin(Arc::clone(&pool).worker(WorkStage::Verify, 0));
    let mut submission = Box::pin(pool.submit_local(bounded(local.clone()), false));
    // Poll the production futures through their first wait, with available CPU
    // capacity and queued work. Suspension, rather than scheduler timing, is
    // the only reason these computations cannot select or resolve a job.
    assert!(futures_util::poll!(resolver.as_mut()).is_pending());
    assert!(futures_util::poll!(verifier.as_mut()).is_pending());
    assert!(futures_util::poll!(submission.as_mut()).is_pending());
    assert_eq!(pool.cpu.available_permits(), capacity);
    assert!(Arc::ptr_eq(
        &pool.store.point(&queued.hash()).1.unwrap(),
        &queued
    ));
    assert!(pool.store.point(&local.hash()).1.is_none());
    assert_eq!(pool.pool_info().await.unwrap().verify_queue_size, 1);
    pool.stop();
    within(resolver).await.unwrap();
    within(verifier).await.unwrap();
    assert!(matches!(within(submission).await, Err(Error::Closed)));
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dry_run_under_active_pressure_returns_without_owner_or_publication_changes() {
    let (pool, _, _, _) = fixture();
    let tx = fund(&pool, 4003);
    let (cpu, memory) = pool.direct_capacity(true).await.unwrap();
    let all_cpu = Arc::clone(&pool.cpu)
        .try_acquire_many_owned(pool.cpu.available_permits() as u32)
        .unwrap();
    let before = pool.store.capture(false).2.len();
    assert!(matches!(
        within(pool.submit_local(bounded(tx), true)).await.unwrap(),
        Err(Reject::Full(_))
    ));
    assert_eq!(pool.store.capture(false).2.len(), before);
    assert!(!pool.is_faulted());
    drop((cpu, all_cpu, memory));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_reset_rebuilds_current_missing_levels_and_ignores_retired_owners() {
    let (pool, sink, drain, _) = fixture();
    let peer = PeerIndex::from(43);
    let missing = OutPoint::new(tx(4444).hash(), 0);
    let first = entry(
        &pool.store,
        tx(4004),
        ingress::remote_source(peer, 0).unwrap(),
    );
    insert(&pool.store, Arc::clone(&first));
    let first = replace(
        &pool.store,
        first,
        Phase::Waiting([DependencyKey::Cell(missing.clone())].into()),
    );
    let second = entry(
        &pool.store,
        tx(4005),
        ingress::remote_source(peer, 0).unwrap(),
    );
    insert(&pool.store, Arc::clone(&second));
    replace(
        &pool.store,
        second,
        Phase::Waiting([DependencyKey::Cell(missing.clone())].into()),
    );
    sink.publish(TxVerificationResult::GenerationReset);
    assert!(matches!(
        drain.try_recv(),
        Some(TxVerificationResult::GenerationReset)
    ));
    let mut remove = Plan::new(pool.store.snapshot().0, Class::Trusted, Default::default());
    remove.edit(Some(first), None, None).unwrap();
    pool.store.apply(remove).unwrap();
    let mut levels = Vec::new();
    for _ in 0..128 {
        if let Some(result) = drain.try_recv() {
            levels.push(result);
        }
    }
    assert_eq!(levels.len(), 1);
    assert!(
        matches!(&levels[0], TxVerificationResult::UnknownParents { peer: origin, parents } if *origin == peer && parents.len() == 1 && parents.contains(&missing.tx_hash()))
    );
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_joins_a_suspended_verification_queue_and_preserves_accepted_state() {
    let (pool, sink, _, handle) = fixture();
    let tx = fund(&pool, 4006);
    let cycles = pool
        .submit_local(bounded(tx.clone()), true)
        .await
        .unwrap()
        .unwrap()
        .cycles;
    pool.verification.suspend().unwrap();
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    pool.submit_remote(bounded(tx.clone()), cycles, PeerIndex::from(46))
        .await
        .unwrap();
    assert!(matches!(
        pool.store.point(&tx.hash()).1.unwrap().phase,
        Phase::Resolve
    ));
    shutdown(&pool, tasks, publisher).await;
    assert_eq!(pool.store.capture(true).2.len(), 1);
    assert!(pool.store.point(&tx.hash()).1.is_some());
}

#[cfg(feature = "internal")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn public_callback_queries_dry_runs_and_rejects_reentrant_mutation_then_service_joins() {
    let directory = tempfile::tempdir().unwrap();
    let mut configuration = config();
    configuration.persisted_data = directory.path().join("pool");
    configuration.recent_reject = Default::default();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (mut builder, controller, _relay) = crate::service::TxPoolServiceBuilder::new(
        configuration.clone(),
        chain_snapshot(),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    let transaction = fund(&pool, 4010);
    let dry = fund(&pool, 4011);
    let expected = transaction.hash();
    let read_controller = controller.clone();
    let (sent, mut received) = mpsc::unbounded_channel();
    builder.register_pending(Box::new(move |entry| {
        if entry.transaction.hash() == expected {
            assert!(read_controller.get_tx_pool_info().unwrap().pending_size >= 3);
            assert!(read_controller.test_accept_tx(dry.clone()).unwrap().is_ok());
            assert!(read_controller.remove_local_tx(expected.clone()).is_err());
            assert!(read_controller.update_ibd_state(true).is_err());
            assert!(read_controller.package_txs(None).is_ok());
            sent.send(()).unwrap();
        }
    }));
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    within(async {
        while !controller.service_started() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let submit = controller.clone();
    within(tokio::task::spawn_blocking(move || {
        submit.submit_local_tx(transaction)
    }))
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    within(received.recv()).await.unwrap();
    controller.stop();
    within(generation).await.unwrap();
    assert!(pool.persistence_eligible());
    assert!(!controller.service_started());
    let saved = crate::persisted::load_persistence_snapshot(&configuration).unwrap();
    assert_eq!(saved.accepted.len(), 3);
    assert!(saved.recovery.is_empty());
}

#[cfg(feature = "internal")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fatal_generation_keeps_previous_persistence_file_unchanged_on_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let mut configuration = config();
    configuration.persisted_data = directory.path().join("pool");
    configuration.recent_reject = Default::default();
    let writer = Arc::new(PersistenceWriter::default());
    writer
        .acquire()
        .await
        .write(
            &configuration.persisted_data,
            PersistenceSnapshot::default(),
        )
        .unwrap();
    let persisted = configuration.persisted_data.with_extension("v2");
    let before = std::fs::read(&persisted).unwrap();
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (builder, controller, _relay) = crate::service::TxPoolServiceBuilder::new(
        configuration,
        chain_snapshot(),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    fund(&pool, 4012);
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    within(async {
        while !controller.service_started() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    pool.fault();
    within(generation).await.unwrap();
    assert!(!pool.persistence_eligible());
    assert_eq!(std::fs::read(persisted).unwrap(), before);
}

#[cfg(feature = "internal")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn internal_insertion_preserves_supplied_metadata_and_duplicates_without_effects() {
    let (pool, _, _, _) = fixture();
    let mut entry =
        crate::TxEntry::dummy_resolve(tx(4200), 7, ckb_types::core::Capacity::shannons(100), 1234);
    entry.timestamp = 123;
    pool.plug(vec![entry.clone()], crate::PlugTarget::Proposed)
        .await
        .unwrap();
    let owner = pool.store.point(&entry.transaction().hash()).1.unwrap();
    let value = owner.accepted().unwrap();
    assert_eq!(value.timestamp, 123);
    assert_eq!(value.size, 1234);
    assert_eq!(value.cycles, 7);
    assert_eq!(value.status(&pool.store.snapshot().1), Status::Proposed);
    entry.fee = ckb_types::core::Capacity::shannons(200);
    pool.plug(vec![entry], crate::PlugTarget::Pending)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &pool.store.point(&owner.hash()).1.unwrap(),
        &owner
    ));
    pool.close_outbox();
    assert!(pool.store.outbox.drained());
}

#[cfg(feature = "internal")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn internal_insertion_cannot_replace_or_evict_an_existing_owner() {
    for replacement in [false, true] {
        let handle = Handle::new(tokio::runtime::Handle::current(), None);
        let configuration = TxPoolConfig {
            min_rbf_rate: ckb_types::core::FeeRate::from_u64(1),
            max_tx_pool_size: 1000,
            ..config()
        };
        let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 64_000_000);
        let (pool, _, _) = Pool::with_store(
            configuration,
            store,
            &handle,
            Arc::new(RwLock::new(init_cache())),
            None,
            None,
            FeeEstimator::new_dummy(),
        )
        .unwrap();
        let transaction = |nonce| {
            if replacement {
                spend(nonce, &[OutPoint::default()], &[])
            } else {
                tx(nonce)
            }
        };
        let size = if replacement { 100 } else { 750 };
        let first = crate::TxEntry::dummy_resolve(
            transaction(4201),
            1,
            ckb_types::core::Capacity::shannons(100),
            size,
        );
        let second = crate::TxEntry::dummy_resolve(
            transaction(4202),
            1,
            ckb_types::core::Capacity::shannons(1_000_000),
            size,
        );
        pool.plug(vec![first.clone()], crate::PlugTarget::Pending)
            .await
            .unwrap();
        let owner = pool.store.point(&first.transaction().hash()).1.unwrap();
        assert!(
            pool.plug(vec![second], crate::PlugTarget::Pending)
                .await
                .is_err()
        );
        assert!(Arc::ptr_eq(
            &pool.store.point(&owner.hash()).1.unwrap(),
            &owner
        ));
        assert_eq!(pool.store.capture(false).2.len(), 1);
        pool.close_outbox();
        assert!(pool.store.outbox.drained());
        assert!(!pool.is_faulted());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_orphan_waits_then_accepts_after_its_parent_completes_normal_verification() {
    let (pool, sink, _, handle) = fixture();
    let parent = fund(&pool, 9013);
    let child = funded_tx(OutPoint::new(parent.hash(), 0), 19_999_998_000);
    let cycles = pool
        .submit_local(bounded(parent.clone()), true)
        .await
        .unwrap()
        .unwrap()
        .cycles;
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    pool.submit_remote(bounded(child.clone()), cycles, 93.into())
        .await
        .unwrap();
    observe(&pool, &child.hash(), |entry| {
        matches!(entry.phase, Phase::Waiting(_))
    })
    .await;
    pool.submit_remote(bounded(parent.clone()), cycles, 94.into())
        .await
        .unwrap();
    observe(&pool, &parent.hash(), |entry| entry.accepted().is_some()).await;
    observe(&pool, &child.hash(), |entry| entry.accepted().is_some()).await;
    assert!(
        pool.store
            .point(&child.hash())
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .parents
            .contains(&parent.hash())
    );
    shutdown(&pool, tasks, publisher).await;
}

#[cfg(feature = "internal")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistence_replay_serves_a_callback_query_before_startup_can_complete() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = TxPoolConfig {
        persisted_data: directory.path().join("pool"),
        recent_reject: Default::default(),
        ..config()
    };
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let (mut builder, controller, _relay) = crate::service::TxPoolServiceBuilder::new(
        configuration.clone(),
        chain_snapshot(),
        None,
        Arc::new(RwLock::new(init_cache())),
        &handle,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let pool = builder.pool_for_test();
    let transaction = fund(&pool, 9014);
    Arc::new(PersistenceWriter::default())
        .acquire()
        .await
        .write(
            &configuration.persisted_data,
            PersistenceSnapshot {
                accepted: vec![transaction.clone()],
                recovery: Vec::new(),
            },
        )
        .unwrap();
    let target = transaction.hash();
    let query = controller.clone();
    let (sent, mut received) = mpsc::unbounded_channel();
    builder.register_pending(Box::new(move |entry| {
        if entry.transaction.hash() == target {
            assert!(!query.service_started());
            assert_eq!(query.get_tx_pool_info().unwrap().pending_size, 2);
            sent.send(()).unwrap();
        }
    }));
    let generation = builder.start_with_handle(DummyTxPoolNetwork);
    within(received.recv()).await.unwrap();
    within(async {
        while !controller.service_started() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    controller.stop();
    within(generation).await.unwrap();
    assert!(pool.persistence_eligible());
}

#[tokio::test]
async fn pool_configuration_rejects_a_current_thread_runtime_before_spawning() {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    assert!(matches!(
        Pool::new(
            config(),
            chain_snapshot(),
            &handle,
            Arc::new(RwLock::new(init_cache())),
            None,
            None,
            FeeEstimator::new_dummy()
        ),
        Err(Error::Full(FullReason::Other(
            "multi-thread runtime required"
        )))
    ));
}

#[test]
fn unusable_time_pipeline_and_per_job_policies_are_rejected_before_ownership() {
    let configs = [
        TxPoolConfig {
            tx_verify_cycles_per_ms: 0,
            ..config()
        },
        TxPoolConfig {
            min_tx_verify_time_ms: 0,
            ..config()
        },
        TxPoolConfig {
            min_tx_verify_time_ms: 2,
            max_tx_verify_time_ms: 1,
            ..config()
        },
        TxPoolConfig {
            max_tx_verify_initial_load_bytes: 0,
            ..config()
        },
        TxPoolConfig {
            max_tx_pool_size: usize::MAX,
            ..config()
        },
        TxPoolConfig {
            max_tx_verify_workers: usize::MAX,
            ..config()
        },
        TxPoolConfig {
            max_ancestors_count: 0,
            ..config()
        },
    ];
    let snapshot = chain_snapshot();
    for config in configs {
        assert!(Store::new(Arc::clone(&snapshot), &config).is_err());
    }
    assert!(
        crate::authority::budget::Limits::with_residency(
            &config(),
            snapshot.consensus(),
            crate::constants::ResidencyLimits {
                accepted: 32_000_000,
                pipeline: 1
            },
        )
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claimed_worker_retains_rejection_under_remote_notice_pressure_while_trusted_work_progresses_and_shutdown_joins()
 {
    let (pool, sink, drain, handle) = fixture();
    let trusted = fund(&pool, 4090);
    let remote = funded_tx(OutPoint::new(tx(4091).hash(), 0), 20_000_000_000);
    let source = ingress::remote_source(94.into(), 1).unwrap();
    pool.store
        .apply(ingress::prepare(&pool.store, Arc::new(remote.clone()), source).unwrap())
        .unwrap();
    let mut job = pool.store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    let owner = Arc::clone(&job.entry);
    let mut reservations = Vec::new();
    for _ in 0..crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES {
        reservations.push(
            pool.store
                .outbox
                .reserve(
                    vec![Effect::relay(TxVerificationResult::Reject {
                        tx_hash: tx(4092).hash(),
                    })],
                    Class::Remote,
                )
                .unwrap()
                .unwrap(),
        );
    }
    let settling = Arc::clone(&pool);
    let mut settlement = Box::pin(async move {
        settling
            .reject_job(
                &mut job,
                Reject::Full("retained result".into()),
                ReadSet::default(),
            )
            .await
    });
    assert!(
        settlement
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_pending()
    );
    assert!(Arc::ptr_eq(
        &pool.store.point(&remote.hash()).1.unwrap(),
        &owner
    ));
    assert!(matches!(
        pool.store.budget.active(source),
        Err(Error::Full(FullReason::Other("peer active work")))
    ));
    let settlement = tokio::spawn(settlement);
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    assert!(
        within(pool.submit_local(bounded(trusted.clone()), false))
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        pool.store
            .point(&trusted.hash())
            .1
            .unwrap()
            .accepted()
            .is_some()
    );
    assert!(!settlement.is_finished());
    pool.stop();
    assert!(!settlement.is_finished());
    assert!(Arc::ptr_eq(
        &pool.store.point(&remote.hash()).1.unwrap(),
        &owner
    ));
    drop(reservations);
    within(settlement).await.unwrap().unwrap();
    assert!(pool.store.point(&remote.hash()).1.is_none());
    assert!(pool.store.budget.active(source).is_ok());
    shutdown(&pool, tasks, publisher).await;
    let mut released = false;
    while let Some(result) = drain.try_recv() {
        if matches!(result, TxVerificationResult::Reject { tx_hash } if tx_hash == remote.hash()) {
            released = true;
        }
    }
    assert!(released);
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn one_runtime_worker_preserves_direct_verification_query_and_shutdown_progress() {
    let (pool, sink, _, handle) = fixture();
    let transaction = fund(&pool, 4093);
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    assert!(
        within(pool.submit_local(bounded(transaction.clone()), false))
            .await
            .unwrap()
            .is_ok()
    );
    assert!(
        within(pool.pool_ids())
            .await
            .unwrap()
            .pending
            .contains(&transaction.hash())
    );
    shutdown(&pool, tasks, publisher).await;
}

async fn control_progress_while_sync_computation_is_at_capacity() {
    let (pool, _, _, _) = fixture();
    let capacity = pool.cpu.available_permits();
    let (entered, mut received) = mpsc::unbounded_channel();
    let mut releases = Vec::new();
    let mut tasks = JoinSet::new();
    for _ in 0..capacity {
        let pool = Arc::clone(&pool);
        let entered = entered.clone();
        let (release, wait) = std::sync::mpsc::sync_channel(1);
        releases.push(release);
        tasks.spawn(async move {
            let permit = pool.compute().await.unwrap();
            pool.run_compute(&permit, || {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(10)).unwrap();
            });
        });
    }
    for _ in 0..capacity {
        within(received.recv()).await.unwrap();
    }
    assert_eq!(pool.cpu.available_permits(), 0);
    assert!(matches!(
        pool.direct_capacity(false).await,
        Err(Error::Full(FullReason::Other("active computation")))
    ));
    let control = Arc::clone(&pool);
    // Spawn onto a runtime worker: the test's own block_on thread is not the
    // spare executor whose progress this regression must establish.
    within(tokio::spawn(async move {
        control.pool_ids().await.unwrap();
        control.stop();
    }))
    .await
    .unwrap();
    assert!(pool.is_stopped());
    for release in releases {
        release.send(()).unwrap();
    }
    while let Some(result) = within(tasks.join_next()).await {
        result.unwrap();
    }
    assert_eq!(pool.cpu.available_permits(), capacity);
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_sync_compute_capacity_leaves_a_runtime_worker_for_query_and_stop() {
    control_progress_while_sync_computation_is_at_capacity().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn one_worker_sync_compute_hands_off_for_query_and_stop() {
    control_progress_while_sync_computation_is_at_capacity().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selected_resolution_requeues_once_when_only_the_view_changes() {
    let (pool, _, _, _) = fixture();
    let original = entry(&pool.store, tx(4094), Source::Local);
    insert(&pool.store, Arc::clone(&original));
    let mut selected = pool.store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    let mut lifecycle = Plan::new(pool.store.snapshot().0, Class::Critical, Default::default());
    lifecycle.reset(pool.store.snapshot().1, false);
    pool.store.apply(lifecycle).unwrap();
    assert!(selected.current().unwrap());
    assert!(pool.store.pop(WorkStage::Resolve, false).unwrap().is_none());
    let mut stale = Plan::new(selected.view, Class::Trusted, Default::default());
    stale
        .edit(Some(Arc::clone(&selected.entry)), None, None)
        .unwrap();
    assert!(matches!(pool.store.apply(stale), Err(Error::Stale)));
    within(pool.requeue(&mut selected)).await.unwrap();
    let successor = pool.store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    assert!(!Arc::ptr_eq(&successor.entry, &original));
    assert_eq!(successor.view, pool.store.snapshot().0);
    assert!(pool.store.pop(WorkStage::Resolve, false).unwrap().is_none());
    pool.store
        .apply(chain::clear(&pool.store, None, false).unwrap())
        .unwrap();
    drop((selected, successor));
    assert!(!pool.is_faulted());
    assert!(!pool.store.budget.faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn legal_ingress_pressure_releases_remote_filter_without_ban_or_generation_loss() {
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let configuration = config();
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 1_000_000);
    let (pool, sink, drain) = Pool::with_store(
        configuration,
        store,
        &handle,
        Arc::new(RwLock::new(init_cache())),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let transaction = funded_tx(OutPoint::new(tx(4200).hash(), 0), 20_000_000_000)
        .as_advanced_builder()
        .set_inputs(
            (0..128)
                .map(|n| {
                    ckb_types::packed::CellInput::new(OutPoint::new(tx(4201 + n).hash(), 0), 0)
                })
                .collect(),
        )
        .build();
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    within(pool.submit_remote(bounded(transaction.clone()), 1, 97.into()))
        .await
        .unwrap();
    assert!(pool.store.point(&transaction.hash()).1.is_none());
    assert!(!pool.store.peer_banned(97.into()));
    assert!(
        matches!(drain.try_recv(), Some(TxVerificationResult::Reject { tx_hash }) if tx_hash == transaction.hash())
    );
    assert!(!pool.is_faulted());
    shutdown(&pool, tasks, publisher).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn committed_chain_reconciliation_publishes_detached_uncle_candidates() {
    use ckb_app_config::BlockAssemblerConfig;
    use ckb_jsonrpc_types::ScriptHashType;
    use ckb_types::{core::BlockBuilder, h256};
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let snapshot = template_snapshot_with_child(Some(200));
    let assembler = BlockAssembler::new(
        BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".into(),
            update_interval_millis: 800,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
            notify_auth_token: None,
        },
        Arc::clone(&snapshot),
    )
    .unwrap();
    let store = store_with_pipeline_limit(Arc::clone(&snapshot), &config(), 64_000_000);
    let (pool, sink, _drain) = Pool::with_store(
        config(),
        store,
        &handle,
        Arc::new(RwLock::new(init_cache())),
        Some(assembler),
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let (_chain, chain) = mpsc::channel(2);
    let (tasks, publisher) =
        pool.start_background(&handle, endpoints(sink, Callbacks::new()), chain);
    let detached = BlockBuilder::default()
        .number(1)
        .parent_hash(snapshot.consensus().genesis_block().hash())
        .timestamp(201)
        .compact_target(snapshot.tip_header().compact_target())
        .epoch(snapshot.epoch_ext().number_with_fraction(1))
        .build();
    let command = ChainReorgArgs::Detailed {
        detached_blocks: [detached.clone()].into(),
        attached_blocks: Default::default(),
        snapshot,
    };
    pool.reconcile(&command).await.unwrap();
    let template = pool.template.as_ref().unwrap();
    within(async {
        loop {
            let output = template.read().await.unwrap();
            if output
                .uncles
                .iter()
                .any(|uncle| uncle.hash == detached.hash().into())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(!pool.is_faulted());
    shutdown(&pool, tasks, publisher).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_parent_wait_and_request_commit_together_after_notice_pressure() {
    let (pool, sink, drain, _) = fixture();
    let missing = OutPoint::new(tx(4100).hash(), 0);
    let transaction = funded_tx(missing.clone(), 20_000_000_000);
    let source = ingress::remote_source(196.into(), 1).unwrap();
    pool.store
        .apply(ingress::prepare(&pool.store, Arc::new(transaction.clone()), source).unwrap())
        .unwrap();
    let mut job = pool.store.pop(WorkStage::Resolve, false).unwrap().unwrap();
    let working = Arc::clone(&job.entry);
    let reservations: Vec<_> = (0..crate::constants::EFFECT_JOURNAL_REMOTE_MAX_BATCHES)
        .map(|_| {
            pool.store
                .outbox
                .reserve(
                    vec![Effect::relay(TxVerificationResult::Reject {
                        tx_hash: tx(4101).hash(),
                    })],
                    Class::Remote,
                )
                .unwrap()
                .unwrap()
        })
        .collect();
    let cpu = Arc::clone(&pool.cpu).acquire_owned().await.unwrap();
    let mut settle = Box::pin(pool.resolve_job(&mut job, cpu));
    assert!(
        settle
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
            .is_pending()
    );
    assert!(Arc::ptr_eq(
        &working,
        &pool.store.point(&transaction.hash()).1.unwrap()
    ));
    assert!(drain.try_recv().is_none());
    assert!(matches!(
        pool.store.budget.active(source),
        Err(Error::Full(FullReason::Other("peer active work")))
    ));
    drop(reservations);
    within(settle).await.unwrap();
    let waiting = pool.store.point(&transaction.hash()).1.unwrap();
    assert!(
        matches!(&waiting.phase, Phase::Waiting(keys) if keys == &std::collections::BTreeSet::from([DependencyKey::Cell(missing.clone())]))
    );
    drop(job); // The worker releases its active frame after the terminal returns.
    assert!(pool.store.budget.active(source).is_ok());
    pool.close_outbox();
    within(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())))
        .await
        .unwrap();
    assert!(
        matches!(drain.try_recv(), Some(TxVerificationResult::UnknownParents { peer, parents }) if peer == 196.into() && parents == std::collections::HashSet::from([missing.tx_hash()]))
    );
    assert!(drain.try_recv().is_none());
    pool.stop();
    assert!(!pool.is_faulted());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_non_contextual_rejection_preserves_empty_membership_for_send_and_dry_run() {
    let (pool, sink, _, _) = fixture();
    let publisher =
        tokio::spawn(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())));
    let invalid = funded_tx(OutPoint::new(tx(4102).hash(), 0), 20_000_000_000)
        .as_advanced_builder()
        .version(1u32)
        .build();
    // Resource refusal can precede validation in a nonwaiting dry-run. Control
    // that premise explicitly; idle background workers also acquire this permit
    // briefly while looking for work, so they do not belong in this path test.
    let occupied = pool.compute().await.unwrap();
    let refused = within(pool.submit_local(bounded(invalid.clone()), true))
        .await
        .unwrap();
    assert!(matches!(refused, Err(Reject::Full(ref reason)) if reason == "active computation"));
    assert!(pool.store.capture(false).2.is_empty());
    drop(occupied);
    for dry_run in [true, false] {
        let result = within(pool.submit_local(bounded(invalid.clone()), dry_run))
            .await
            .unwrap();
        assert!(
            matches!(result, Err(Reject::Verification(_))),
            "dry_run={dry_run}, actual={result:?}"
        );
        assert!(pool.store.capture(false).2.is_empty());
        assert!(pool.store.budget.active(Source::Local).is_ok());
    }
    pool.stop();
    pool.close_outbox();
    within(publisher).await.unwrap().unwrap();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_limit_diagnostics_do_not_wait_for_capacity_events() {
    let (pool, _, _, _) = fixture();
    for reason in [
        "notice outbox",
        "chain transition",
        "pipeline",
        "replacement history",
    ] {
        let mut attempts = 0;
        let result = within(pool.commit(|| {
            attempts += 1;
            Err(Error::Full(reason.into()))
        }))
        .await;
        assert!(matches!(result, Err(Error::Full(FullReason::Other(value))) if value == reason));
        assert_eq!(attempts, 1);
    }
    pool.stop();
    pool.close_outbox();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_replacement_history_reenters_through_verified_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = TxPoolConfig {
        persisted_data: directory.path().join("pool"),
        min_rbf_rate: ckb_types::core::FeeRate::from_u64(1000),
        ..config()
    };
    let handle = Handle::new(tokio::runtime::Handle::current(), None);
    let store = store_with_pipeline_limit(chain_snapshot(), &configuration, 64_000_000);
    let (pool, sink, _) = Pool::with_store(
        configuration.clone(),
        store,
        &handle,
        Arc::new(RwLock::new(init_cache())),
        None,
        None,
        FeeEstimator::new_dummy(),
    )
    .unwrap();
    let transaction = fund(&pool, 6403);
    let replacement = funded_tx(transaction.input_pts_iter().next().unwrap(), 19_999_990_000);
    let publisher =
        tokio::spawn(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())));
    within(pool.submit_local(bounded(transaction.clone()), false))
        .await
        .unwrap()
        .unwrap();
    within(pool.submit_local(bounded(replacement.clone()), false))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        pool.store.point(&transaction.hash()).1.unwrap().phase,
        Phase::Replaced { .. }
    ));
    within(pool.save()).await.unwrap();
    let saved = crate::persisted::load_persistence_snapshot(&configuration).unwrap();
    assert!(
        saved
            .accepted
            .iter()
            .any(|tx| tx.hash() == replacement.hash())
    );
    assert!(
        !saved
            .accepted
            .iter()
            .any(|tx| tx.hash() == transaction.hash())
    );
    assert_eq!(saved.recovery.len(), 1);
    assert_eq!(saved.recovery[0].hash(), transaction.hash());
    pool.stop();
    pool.close_outbox();
    within(publisher).await.unwrap().unwrap();
    assert!(pool.persistence_eligible());

    let (restored, sink, _, _) = fixture();
    // Restore the fixture funding output, then check both outcomes for the
    // saved recovery body. No historical acceptance is installed.
    fund(&restored, 6403);
    let publisher =
        tokio::spawn(Arc::clone(&restored.store.outbox).run(endpoints(sink, Callbacks::new())));
    let blocked = within(
        restored.replay(
            PersistenceSnapshot {
                accepted: vec![replacement.clone()],
                recovery: saved.recovery.clone(),
            }
            .prepare_replay()
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(blocked, (1, 1));
    assert!(restored.store.point(&transaction.hash()).1.is_none());
    assert!(
        within(restored.remove_local(&replacement.hash()))
            .await
            .unwrap()
            .unwrap()
    );
    let result = within(
        restored.replay(
            PersistenceSnapshot {
                accepted: Vec::new(),
                recovery: saved.recovery,
            }
            .prepare_replay()
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(result, (1, 0));
    let accepted = restored.store.point(&transaction.hash()).1.unwrap();
    assert!(accepted.accepted().unwrap().cycles > 0);
    restored.stop();
    restored.close_outbox();
    within(publisher).await.unwrap().unwrap();
    assert!(restored.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_history_pressure_preserves_local_and_owned_job_admission() {
    for (local, stop) in [(false, false), (true, false), (false, true), (true, true)] {
        let handle = Handle::new(tokio::runtime::Handle::current(), None);
        let configuration = TxPoolConfig {
            max_tx_verify_workers: 1,
            min_rbf_rate: ckb_types::core::FeeRate::from_u64(1_000),
            ..config()
        };
        let store = store_with_pipeline_limit(
            chain_snapshot(),
            &configuration,
            if local { 64_000 } else { 96_000 },
        );
        let (pool, sink, _drain) = Pool::with_store(
            configuration,
            store,
            &handle,
            Arc::new(RwLock::new(init_cache())),
            None,
            None,
            FeeEstimator::new_dummy(),
        )
        .unwrap();
        let transaction = fund(&pool, 7300);
        let incumbent = accept(&pool.store, transaction.clone(), 1_000, 1, Status::Pending);
        let replacement = funded_tx(transaction.input_pts_iter().next().unwrap(), 19_999_990_000);
        let source = if local {
            Source::Local
        } else {
            Source::Remote {
                peer: 7300.into(),
                deadline: Instant::now() + Duration::from_secs(60),
                cycles: Some(1),
            }
        };
        let candidate = entry(&pool.store, replacement.clone(), source);
        let verified = verified(&pool.store, &candidate, 10_000, 1, Status::Pending);
        let (with_history, reject) =
            membership::admission(&pool.store, &candidate, None, &verified, &pool.config, true)
                .unwrap();
        assert!(reject.is_none());
        assert!(matches!(
            pool.store.apply(with_history),
            Err(Error::Full(FullReason::History))
        ));
        assert!(pool.store.point(&incumbent).1.unwrap().accepted().is_some());
        assert!(pool.store.point(&candidate.hash()).1.is_none());

        let planned_before = pool
            .store
            .admission_attempts
            .load(std::sync::atomic::Ordering::Acquire);
        if stop {
            let stopping = Arc::downgrade(&pool);
            let target = candidate.hash();
            *pool.store.commit_observer.lock() = Some(Arc::new(move |plan, locked| {
                if locked
                    && plan.edits().get(&target).is_some_and(|edit| {
                        edit.after
                            .as_ref()
                            .is_some_and(|owner| owner.accepted().is_some())
                    })
                {
                    stopping.upgrade().unwrap().stop();
                }
            }));
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        let victim = incumbent.clone();
        let mut callbacks = Callbacks::new();
        callbacks.register_reject(Box::new(move |entry, _| {
            if entry.transaction.hash() == victim {
                recorded.lock().push("rejected");
            }
        }));
        let recorded = Arc::clone(&events);
        let admitted = candidate.hash();
        callbacks.register_pending(Box::new(move |entry| {
            if entry.transaction.hash() == admitted {
                recorded.lock().push("accepted");
            }
        }));
        let publisher =
            tokio::spawn(Arc::clone(&pool.store.outbox).run(endpoints(sink, callbacks)));
        if local {
            let result = within(pool.submit_local(bounded(replacement), false)).await;
            if stop {
                assert!(matches!(result, Err(Error::Closed)));
            } else {
                result.unwrap().unwrap();
            }
        } else {
            let queued = candidate.with_phase(Phase::Verify(Arc::clone(verified.resolved())));
            insert(&pool.store, Arc::clone(&queued));
            let mut job = pool.store.pop(WorkStage::Verify, false).unwrap().unwrap();
            assert!(Arc::ptr_eq(&job.entry, &queued));
            within(pool.accept_job(&mut job, &verified)).await.unwrap();
        }
        assert_eq!(
            pool.store
                .admission_attempts
                .load(std::sync::atomic::Ordering::Acquire)
                - planned_before,
            1,
            "local={local}: optional history pressure must reuse the membership decision"
        );
        if stop {
            assert!(pool.store.point(&incumbent).1.unwrap().accepted().is_some());
            let queued = pool.store.point(&candidate.hash()).1;
            if local {
                assert!(queued.is_none());
            } else {
                assert!(matches!(queued.unwrap().phase, Phase::Resolve));
            }
        } else {
            assert!(
                pool.store
                    .point(&candidate.hash())
                    .1
                    .unwrap()
                    .accepted()
                    .is_some()
            );
            assert!(pool.store.point(&incumbent).1.is_none());
        }
        assert!(!pool.is_faulted());
        pool.stop();
        pool.close_outbox();
        within(publisher).await.unwrap().unwrap();
        assert_eq!(
            *events.lock(),
            if stop {
                vec![]
            } else {
                vec!["rejected", "accepted"]
            }
        );
        assert!(pool.persistence_eligible());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_queries_expose_acceptance_and_count_remaining_verification_work() {
    let (pool, sink, _, _) = fixture();
    let candidate = entry(&pool.store, tx(7350), Source::Local);
    insert(&pool.store, Arc::clone(&candidate));
    assert_eq!(
        pool.transaction_status(&candidate.hash()).unwrap(),
        (TxStatus::Unknown, None)
    );
    let unresolved = pool.transaction(&candidate.hash()).await.unwrap();
    assert_eq!(unresolved.tx_status, TxStatus::Unknown);
    assert!(
        unresolved.transaction.is_none() && unresolved.cycles.is_none() && unresolved.fee.is_none()
    );
    assert_eq!(pool.pool_info().await.unwrap().verify_queue_size, 1);

    let missing = OutPoint::new(tx(7351).hash(), 0);
    let waiting = replace(
        &pool.store,
        candidate,
        Phase::Waiting([DependencyKey::Cell(missing)].into()),
    );
    assert_eq!(
        pool.transaction_status(&waiting.hash()).unwrap(),
        (TxStatus::Unknown, None)
    );
    assert!(
        pool.transaction(&waiting.hash())
            .await
            .unwrap()
            .transaction
            .is_none()
    );
    let summary = pool.pool_info().await.unwrap();
    assert_eq!(summary.orphan_size, 1);
    assert_eq!(summary.verify_queue_size, 0);
    let candidate = replace(&pool.store, waiting, Phase::Resolve);

    let verified = verified(&pool.store, &candidate, 1, 7, Status::Pending);
    let queued = replace(
        &pool.store,
        candidate,
        Phase::Verify(Arc::clone(verified.resolved())),
    );
    assert_eq!(pool.pool_info().await.unwrap().verify_queue_size, 1);
    let mut job = pool.store.pop(WorkStage::Verify, false).unwrap().unwrap();
    assert!(Arc::ptr_eq(&job.entry, &queued));
    assert_eq!(pool.pool_info().await.unwrap().verify_queue_size, 0);
    assert_eq!(
        pool.transaction_status(&queued.hash()).unwrap(),
        (TxStatus::Unknown, None)
    );
    assert!(
        pool.transaction(&queued.hash())
            .await
            .unwrap()
            .transaction
            .is_none()
    );
    pool.accept_job(&mut job, &verified).await.unwrap();
    drop(job);
    assert_eq!(
        pool.transaction_status(&queued.hash()).unwrap(),
        (TxStatus::Pending, Some(7))
    );
    let accepted = pool.transaction(&queued.hash()).await.unwrap();
    assert_eq!(accepted.transaction.unwrap().hash(), queued.hash());
    assert_eq!(accepted.cycles, Some(7));
    assert_eq!(accepted.fee, Some(ckb_types::core::Capacity::shannons(1)));
    let summary = pool.pool_info().await.unwrap();
    assert_eq!(summary.pending_size, 1);
    assert_eq!(summary.verify_queue_size, 0);
    pool.stop();
    pool.close_outbox();
    within(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())))
        .await
        .unwrap();
    assert!(pool.persistence_eligible());
}

#[test]
fn maintenance_expires_accepted_work_while_wake_pages_remain() {
    let fixture_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (pool, sink, _, _) = fixture_runtime.block_on(async { fixture() });
    // Construct through the supported production runtime, then drive only this
    // maintenance future with a controlled clock. No worker/VM task is started.
    let clock_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    clock_runtime.block_on(async {
        let parent = output_tx(6124);
        let ready = DependencyKey::Cell(OutPoint::new(parent.hash(), 0));
        let missing = DependencyKey::Cell(OutPoint::new(output_tx(6125).hash(), 0));
        for nonce in 6200..6360 {
            let owner = entry(
                &pool.store,
                tx(nonce),
                Source::Remote {
                    peer: 1.into(),
                    deadline: Instant::now() + Duration::from_secs(60),
                    cycles: Some(1),
                },
            )
            .with_phase(Phase::Waiting(BTreeSet::from([
                ready.clone(),
                missing.clone(),
            ])));
            insert(&pool.store, owner);
        }
        accept(&pool.store, parent.clone(), 1, 1, Status::Pending);
        let owner = pool.store.point(&parent.hash()).1.unwrap();
        let mut value = owner.accepted().unwrap().clone();
        value.timestamp = ckb_systemtime::unix_time_as_millis();
        replace(&pool.store, owner, Phase::Accepted(value));
        let expired = accept(&pool.store, tx(6126), 1, 1, Status::Pending);

        let maintenance = Arc::clone(&pool).maintain();
        tokio::pin!(maintenance);
        // Register the timer, make its deadline ready, then resume the same future.
        // Five pages leave unvisited work after these two cooperative polls.
        assert!(futures_util::poll!(maintenance.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(futures_util::poll!(maintenance.as_mut()).is_pending());
        assert!(pool.store.point(&expired).1.is_none());
        assert!(pool.store.point(&parent.hash()).1.is_some());
        assert!(
            !pool
                .store
                .wake_page(&mut None)
                .expect("wake work still pending at the expiry turn")
                .hashes
                .is_empty()
        );
        pool.stop();
        within(maintenance).await.unwrap();
        pool.close_outbox();
        within(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())))
            .await
            .unwrap();
        assert!(pool.persistence_eligible());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_removal_completes_the_accepted_closure_beyond_an_expiry_page() {
    let (pool, sink, _, _) = fixture();
    let root_hash = accept(&pool.store, output_tx(7400), 1, 1, Status::Pending);
    let root = pool.store.point(&root_hash).1.unwrap();
    let dependency = OutPoint::new(root_hash.clone(), 0);
    for nonce in 0..crate::constants::MAX_POOL_MUTATION_CANDIDATES {
        accept(
            &pool.store,
            spend(
                7401 + u32::try_from(nonce).unwrap(),
                &[],
                std::slice::from_ref(&dependency),
            ),
            1,
            1,
            Status::Pending,
        );
    }
    let unrelated = accept(&pool.store, output_tx(7600), 1, 1, Status::Pending);
    let prepared = membership::removal(&pool.store, &root, &pool.config, None).unwrap();
    assert_eq!(
        prepared.edits().len(),
        crate::constants::MAX_POOL_MUTATION_CANDIDATES + 1
    );
    assert!(!prepared.edits().contains_key(&unrelated));
    let expiry =
        membership::removal(&pool.store, &root, &pool.config, Some(Reject::Expiry(0))).unwrap();
    assert_eq!(
        expiry.edits().len(),
        crate::constants::MAX_POOL_MUTATION_CANDIDATES
    );
    assert!(!expiry.edits().contains_key(&root_hash));
    drop(expiry);
    let late = accept(
        &pool.store,
        spend(7601, &[], &[dependency]),
        1,
        1,
        Status::Pending,
    );
    assert!(matches!(pool.store.apply(prepared), Err(Error::Stale)));
    assert!(pool.store.point(&root_hash).1.is_some());
    assert!(pool.store.point(&late).1.is_some());

    let publisher =
        tokio::spawn(Arc::clone(&pool.store.outbox).run(endpoints(sink, Callbacks::new())));
    assert!(
        within(pool.remove_local(&root_hash))
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        !within(pool.remove_local(&root_hash))
            .await
            .unwrap()
            .unwrap()
    );
    let remaining = pool.store.capture(true).2;
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].hash(), unrelated);
    pool.stop();
    pool.close_outbox();
    within(publisher).await.unwrap().unwrap();
    assert!(pool.persistence_eligible());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_rejection_releases_work_before_a_blocked_callback_and_preserves_join() {
    for cancel_publisher in [false, true] {
        let (pool, sink, drain, _) = fixture();
        let victim = accept(&pool.store, output_tx(8120), 1, 1, Status::Pending);
        let independent = fund(&pool, 8121);
        let local = fund(&pool, 8122);
        let (entered, mut events) = mpsc::unbounded_channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let waiting = std::sync::Mutex::new(waiting);
        let blocked = victim.clone();
        let mut callbacks = Callbacks::new();
        callbacks.register_reject(Box::new(move |entry, _| {
            if entry.transaction.hash() == blocked {
                entered.send(()).unwrap();
                waiting
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap();
            }
        }));
        // Preaccepted rejection does not itself have an accepted-entry
        // callback. An earlier real expiry callback blocks its FIFO publisher.
        let head = pool
            .store
            .apply(
                membership::removal(
                    &pool.store,
                    &pool.store.point(&victim).1.unwrap(),
                    &pool.config,
                    Some(Reject::Expiry(0)),
                )
                .unwrap(),
            )
            .unwrap()
            .unwrap();
        let publisher =
            tokio::spawn(Arc::clone(&pool.store.outbox).run(endpoints(sink, callbacks)));
        within(events.recv()).await.unwrap();
        assert!(!publisher.is_finished());
        let mut published = Box::pin(head.wait(&pool.store.outbox));
        assert!(futures_util::poll!(published.as_mut()).is_pending());
        drop(published);

        let source = ingress::remote_source(8120.into(), 1).unwrap();
        let rejected = entry(
            &pool.store,
            funded_tx(OutPoint::new(tx(8123).hash(), 0), 20_000_000_000)
                .as_advanced_builder()
                .header_dep(tx(8124).hash())
                .build(),
            source,
        );
        assert!(matches!(
            jobs::resolve(&pool.store, &rejected, &pool.config),
            Ok(Resolution::Rejected(Reject::Resolve(_), _))
        ));
        insert(&pool.store, Arc::clone(&rejected));
        // Only one worker runs, so subsequent Resolve progress must come from
        // the worker that rejected this transaction, not another verifier.
        let worker = tokio::spawn(Arc::clone(&pool).worker(WorkStage::Resolve, 0));
        within(async {
            loop {
                let changed = pool.store.budget.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if pool.store.point(&rejected.hash()).1.is_none()
                    && let Ok(memory) = pool.store.budget.active(source)
                {
                    drop(memory);
                    break;
                }
                assert!(!pool.is_faulted());
                changed.await;
            }
        })
        .await;
        assert!(pool.store.outbox.pending_reject(&rejected.hash()).is_some());
        assert!(!publisher.is_finished());
        pool.submit_remote(bounded(independent.clone()), 1, 8120.into())
            .await
            .unwrap();
        observe(&pool, &independent.hash(), |entry| {
            matches!(entry.phase, Phase::Verify(_))
        })
        .await;

        let local = if cancel_publisher {
            None
        } else {
            let submitting = Arc::clone(&pool);
            let expected = local.hash();
            let response =
                tokio::spawn(async move { submitting.submit_local(bounded(local), false).await });
            observe(&pool, &expected, |entry| entry.accepted().is_some()).await;
            // A committed direct local request still waits for its effects.
            assert!(!response.is_finished());
            Some(response)
        };
        pool.stop();
        within(worker).await.unwrap().unwrap();
        assert!(pool.verification.resume().is_err());
        assert!(!publisher.is_finished());
        assert!(!pool.persistence_eligible());
        if let Some(response) = &local {
            assert!(!response.is_finished());
        }
        if cancel_publisher {
            publisher.abort();
            assert!(!publisher.is_finished());
        } else {
            pool.close_outbox();
        }
        // Stop joined the background worker while the callback was blocked;
        // publication still owns its batch and cannot join until release.
        release.send(()).unwrap();
        let result = within(publisher).await;
        if cancel_publisher {
            assert!(result.unwrap_err().is_cancelled());
            assert!(pool.is_faulted());
            assert!(matches!(pool.open(), Err(Error::Fault(_))));
            pool.close_outbox();
            assert!(!pool.persistence_eligible());
        } else {
            result.unwrap().unwrap();
            assert!(within(local.unwrap()).await.unwrap().unwrap().is_ok());
            assert!(pool.persistence_eligible());
            assert!(!pool.is_faulted());
        }
        within(head.wait(&pool.store.outbox)).await.unwrap();
        assert!(pool.store.budget.active(source).is_ok());
        let mut saw_rejection = false;
        while let Some(result) = drain.try_recv() {
            if matches!(result, TxVerificationResult::Reject { tx_hash } if tx_hash == rejected.hash())
            {
                saw_rejection = true;
            }
        }
        assert!(saw_rejection);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn admitted_ingress_batch_returns_a_poll_boundary_and_preserves_the_stopped_prefix() {
    for proposal in [false, true] {
        let (pool, _, _, _) = fixture();
        let transactions: Vec<_> = (8200..8712)
            .map(|nonce| funded_tx(OutPoint::new(tx(nonce).hash(), 0), 20_000_000_000))
            .collect();
        let hashes: Vec<_> = transactions.iter().map(TransactionView::hash).collect();
        let offered = transactions.len();
        let (message, response) = if proposal {
            (
                crate::service::Message::NotifyTxs(crate::service::Notify::new(
                    crate::service::NotifyTxBatch::try_new(transactions).unwrap(),
                )),
                None,
            )
        } else {
            let batch = crate::service::RemoteTxSubmissionBatch::try_new(
                transactions.into_iter().map(|tx| (tx, 1)).collect(),
                62.into(),
            )
            .unwrap();
            let (sender, response) = tokio::sync::oneshot::channel();
            (
                crate::service::Message::SubmitRemoteTxBatch(Request::call(batch, sender)),
                Some(response),
            )
        };
        let (observed, first_poll) = tokio::sync::oneshot::channel();
        let (resume, resumed) = tokio::sync::oneshot::channel();
        let running = Arc::clone(&pool);
        // A real spawned Tokio handler has a cooperative budget. No pool
        // workers or publisher can create unrelated awaits for these fresh,
        // valid first installations; the public message bounds remain intact.
        let handler = tokio::spawn(async move {
            let mut processing = Box::pin(crate::service::process(Arc::clone(&running), message));
            let first = futures_util::poll!(processing.as_mut());
            let installed = running.store.capture(false).2.len();
            observed.send((first.is_pending(), installed)).unwrap();
            match first {
                std::task::Poll::Ready(result) => result,
                std::task::Poll::Pending => {
                    // This test-only acknowledgement fixes Stop between two
                    // production polls without assuming executor queue order.
                    resumed.await.unwrap();
                    processing.await
                }
            }
        });
        let (pending, installed) = within(first_poll).await.unwrap();
        assert!(pending, "the admitted batch ran to completion in one poll");
        assert!(installed > 0 && installed < offered);
        for (index, hash) in hashes.iter().enumerate() {
            assert_eq!(pool.store.point(hash).1.is_some(), index < installed);
        }
        pool.stop();
        resume.send(()).unwrap();
        within(handler).await.unwrap().unwrap();
        if let Some(response) = response {
            let (reported, completed, error) = within(response).await.unwrap().into_parts();
            assert_eq!(reported, offered);
            assert_eq!(completed, installed);
            assert!(error.is_some());
        }
        assert_eq!(pool.store.capture(false).2.len(), installed);
        assert!(pool.verification.resume().is_err());
        pool.close_outbox();
        assert!(pool.persistence_eligible());
        assert!(!pool.is_faulted());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn public_relay_batch_drain_keeps_raw_reset_order_before_waiter_reconstruction() {
    for limit in [0, 1, 2, 3, usize::MAX] {
        let (pool, sink, drain, _) = fixture();
        let missing = tx(8130).hash();
        let waiter = entry(&pool.store, tx(8131), remote(61, 1)).with_phase(Phase::Waiting(
            [DependencyKey::Cell(OutPoint::new(missing.clone(), 0))].into(),
        ));
        insert(&pool.store, waiter);
        sink.publish(TxVerificationResult::GenerationReset);
        sink.publish(TxVerificationResult::Reject {
            tx_hash: tx(8132).hash(),
        });
        let receiver = crate::service::TxVerificationResultReceiver::from_authority(drain);
        let mut results = receiver.drain(limit);
        assert_eq!(results.len(), limit.min(3));
        results.extend(receiver.drain(16));
        let [reset, rejected, rebuilt]: [TxVerificationResult; 3] = results.try_into().unwrap();
        assert!(matches!(reset, TxVerificationResult::GenerationReset));
        assert!(
            matches!(rejected, TxVerificationResult::Reject { tx_hash } if tx_hash == tx(8132).hash())
        );
        assert!(
            matches!(rebuilt, TxVerificationResult::UnknownParents { peer, parents } if peer == PeerIndex::from(61) && parents == [missing].into_iter().collect())
        );
        assert!(receiver.drain(16).is_empty());
        assert!(receiver.try_recv().is_none());
        assert!(!pool.is_faulted());
    }
}
