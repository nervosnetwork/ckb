use super::*;

/// Resolution parks on the exact `Unknown` out-point reported by the cell
/// provider. Once that edge becomes available, a fresh resolution can discover
/// the next missing edge without guessing or registering unrelated parents.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_job_registers_the_exact_unknown_outpoint() {
    use crate::component::pre_pool::DependencyKey;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::resolve_mgr::ResolveStageResult;
    use crate::resolved_tx::ResolveJob;
    use ckb_types::packed::Byte32;

    let h = harness(0).workers(WorkerSet::None).build();
    let first_parent = Byte32::new([0x41; 32]);
    let second_parent = Byte32::new([0x42; 32]);
    let tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::new(first_parent.clone(), 0), 0))
        .input(CellInput::new(OutPoint::new(second_parent.clone(), 0), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(1_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();

    let result = crate::resolve_mgr::resolve_job(
        &h.service,
        ResolveJob::new_at(
            tx,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            0,
        ),
    )
    .await;
    assert!(matches!(
        result,
        ResolveStageResult::Orphan(dependencies)
            if dependencies == std::collections::BTreeSet::from([
                DependencyKey::Cell(OutPoint::new(first_parent, 0)),
            ])
    ));

    h.cancel.cancel();
}

/// A parent can commit after a child resolver observed `Unknown` but before
/// it registers the wait. The atomic TxPool -> coordinator settlement must
/// requeue the child instead of installing a waiter after the only wake edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_commit_before_wait_registration_requeues_child() {
    use crate::component::pre_pool::{DependencyKey, PrePoolLocation, ResolveLane};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::pipeline_ops::ParentWaitOutcome;

    let h = harness(1).workers(WorkerSet::None).build();
    let parent = build_tx(&h.out_points[0], 4_000);
    let child = build_tx(&OutPoint::new(parent.hash(), 0), 3_000);
    let child_hash = child.hash();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .kernel
        .admit_transaction(
            child,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
            ResolveLane::Ordered,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap();

    h.service
        .process_tx(parent.clone(), TxSource::Local)
        .await
        .expect("parent commits before child waiter registration");
    assert!(matches!(
        h.service
            .settle_raw_parent_wait(
                &lease,
                std::collections::BTreeSet::from([DependencyKey::Cell(OutPoint::new(
                    parent.hash(),
                    0,
                ))]),
                h.service
                    .reserve_effects(TxPoolService::unknown_parents_effect_bytes(1))
                    .await
                    .unwrap(),
            )
            .await
            .unwrap(),
        ParentWaitOutcome::Requeued
    ));
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.view(&child_hash).unwrap().location),
        PrePoolLocation::ResolveQueued
    );

    h.cancel.cancel();
}

/// A remote transaction becomes externally observable as `UnknownParents`
/// only through the same coordinator transition that installs its durable
/// parent wait. The missing hash intentionally differs from the raw direct
/// parent: dep-group expansion can discover it only during resolution, and
/// that ordinary input must extend the charged canonical graph rather than
/// reaching fail-stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_parent_wait_and_unknown_parents_effect_are_one_transition() {
    use crate::component::pre_pool::{DependencyKey, PrePoolLocation, ResolveLane};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;
    use crate::service::pipeline_ops::ParentWaitOutcome;
    use ckb_types::packed::Byte32;
    use std::collections::HashSet;

    let h = harness(0).workers(WorkerSet::None).build();
    let direct_parent = Byte32::new([42; 32]);
    let discovered_parent = Byte32::new([43; 32]);
    let child = build_tx(&OutPoint::new(direct_parent.clone(), 0), 3_000);
    let child_hash = child.hash();
    let peer = 7.into();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .kernel
        .admit_transaction(
            child,
            TxSource::Remote { cycles: 0, peer },
            epoch,
            ResolveLane::Ordered,
        )
        .unwrap();
    let mut expected_dependencies = h
        .service
        .pipeline
        .kernel
        .read(|coordinator| coordinator.view(&child_hash).unwrap().dependencies);
    assert!(expected_dependencies.contains(&direct_parent));
    expected_dependencies.insert(discovered_parent.clone());
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap();

    let outcome = h
        .service
        .settle_raw_parent_wait(
            &lease,
            std::collections::BTreeSet::from([DependencyKey::Cell(OutPoint::new(
                discovered_parent.clone(),
                0,
            ))]),
            h.service
                .reserve_effects(TxPoolService::unknown_parents_effect_bytes(1))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, ParentWaitOutcome::Parked));
    assert_eq!(
        h.service.pipeline.kernel.read(|coordinator| {
            let view = coordinator.view(&child_hash).unwrap();
            assert_eq!(view.dependencies, expected_dependencies);
            view.location
        }),
        PrePoolLocation::Wait(crate::component::pre_pool::WaitReason::Missing),
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("parent request is published from the journal");
    match relayed {
        TxVerificationResult::UnknownParents {
            peer: relayed_peer,
            parents,
        } => {
            assert_eq!(relayed_peer, peer);
            assert_eq!(parents, HashSet::from([discovered_parent]));
        }
        other => panic!("unexpected relay result: {other:?}"),
    }

    h.cancel.cancel();
}

/// Administrative removal deletes an accepted root and every accepted
/// descendant. Coordinator consumers of any member of that closure must be
/// demoted before the pool mutation, not only consumers of the root.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_pool_closure_demotes_consumers_of_removed_descendants() {
    use crate::component::entry::TxEntry;
    use crate::component::pre_pool::{PrePoolLocation, ResolveLane};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::RemoveTxOutcome;

    let h = harness(1).workers(WorkerSet::None).build();
    let root = build_tx(&h.out_points[0], 4_000);
    let child = build_tx(&OutPoint::new(root.hash(), 0), 3_000);
    let consumer = build_tx(&OutPoint::new(child.hash(), 0), 2_000);
    let root_id = root.proposal_short_id();
    let child_id = child.proposal_short_id();
    let consumer_hash = consumer.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        for tx in [root.clone(), child.clone()] {
            pool.pool_map
                .add_entry(
                    TxEntry::dummy_resolve(tx, 0, Capacity::zero(), 100),
                    Status::Pending,
                )
                .unwrap();
        }
    }
    h.service
        .pipeline
        .kernel
        .admit_transaction(
            consumer,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            h.service.current_pipeline_epoch().unwrap(),
            ResolveLane::Ordered,
        )
        .unwrap();

    assert_eq!(
        h.service.remove_tx(root.hash()).await,
        RemoveTxOutcome::Removed
    );
    let pool = h.service.pool.tx_pool.read().await;
    assert!(pool.get_tx_from_pool(&root_id).is_none());
    assert!(pool.get_tx_from_pool(&child_id).is_none());
    drop(pool);
    assert_eq!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.view(&consumer_hash).unwrap().location),
        PrePoolLocation::Wait(crate::component::pre_pool::WaitReason::Missing)
    );

    h.cancel.cancel();
}

/// `clear_pool` advances the pipeline epoch immediately, then waits behind an
/// in-flight reorg's whole recovery slice. Even if that reorg subsequently
/// re-adds a detached transaction, clear must run last and return with neither
/// accepted nor coordinator ownership left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clear_during_reorg_recovery_owns_the_final_empty_state() {
    use crate::callback::Callbacks;
    use crate::component::tests::harness::{WorkerSet, harness};
    use std::collections::{HashSet, VecDeque};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let id = tx.proposal_short_id();
    let detached = BlockBuilder::default()
        .transaction(TransactionBuilder::default().build())
        .transaction(tx)
        .build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();

    let pending_calls = Arc::new(AtomicUsize::new(0));
    let pending_calls_cb = Arc::clone(&pending_calls);
    let mut callbacks = Callbacks::new();
    callbacks.register_pending(Box::new(move |_| {
        pending_calls_cb.fetch_add(1, Ordering::SeqCst);
    }));
    h.service.relay.callbacks = Arc::new(callbacks);

    // Hold TxPool so the reorg deterministically acquires recovery_lock and
    // pauses before its first authoritative slice.
    let pool_guard = h.service.pool.tx_pool.write().await;
    let reorg_service = h.service.clone();
    let reorg_snapshot = Arc::clone(&snapshot);
    let reorg = tokio::spawn(async move {
        reorg_service
            .update_tx_pool_for_reorg(
                VecDeque::from([detached]),
                VecDeque::new(),
                HashSet::new(),
                reorg_snapshot,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match h.service.recovery_lock.try_lock() {
                Ok(guard) => drop(guard),
                Err(_) => break,
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reorg acquires recovery serialization before clear starts");

    let old_epoch = h.service.current_pipeline_epoch().unwrap();
    let mut clear_service = h.service.clone();
    let clear = tokio::spawn(async move { clear_service.clear_pool(snapshot).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while h.service.current_pipeline_epoch().unwrap() == old_epoch {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("clear establishes its epoch barrier while reorg is in flight");

    drop(pool_guard);
    tokio::time::timeout(Duration::from_secs(10), reorg)
        .await
        .expect("reorg recovery completes")
        .expect("reorg task joins")
        .expect("detached transaction is recoverable");
    tokio::time::timeout(Duration::from_secs(10), clear)
        .await
        .expect("clear runs after recovery")
        .expect("clear task joins");
    h.service.relay.effects.wait_idle().await;

    assert_eq!(
        pending_calls.load(Ordering::SeqCst),
        1,
        "the in-flight reorg really re-added its detached transaction before clear"
    );
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .get_pool_entry(&id)
            .is_none(),
        "clear is the final accepted-pool state"
    );
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash)),
        "clear is also the final pre-pool state"
    );

    h.cancel.cancel();
}

/// Successful dep-group expansion must add every live member to the canonical
/// coordinator graph, not only members that happened to be missing. A later
/// RBF removal of such a member must demote the already-resolved consumer in
/// the same pool/coordinator transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_rbf_commit_demotes_consumer_of_live_expanded_dep_group_member() {
    use crate::component::pre_pool::{PrePoolLocation, ResolveLane};
    use crate::component::tests::harness::{WorkerSet, harness};
    use ckb_types::core::DepType;
    use ckb_types::packed::OutPointVec;

    let h = harness(3).rbf(true).workers(WorkerSet::None).build();
    let original = build_tx(&h.out_points[0], 4_000);
    h.service
        .process_tx(original.clone(), TxSource::Local)
        .await
        .expect("original enters the accepted pool");

    let group_data = Into::<OutPointVec>::into(vec![OutPoint::new(original.hash(), 0)]).as_bytes();
    let group = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(h.out_points[1].clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(group_data.pack())
        .build();
    h.service
        .process_tx(group.clone(), TxSource::Local)
        .await
        .expect("dep-group cell enters the accepted pool");

    let consumer = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(group.hash(), 0))
                .dep_type(DepType::DepGroup)
                .build(),
        )
        .input(CellInput::new(h.out_points[2].clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(3_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let consumer_hash = consumer.hash();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .kernel
        .admit_transaction(
            consumer.clone(),
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
            ResolveLane::Ordered,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .kernel
        .checkout_resolve(ResolveLane::Ordered)
        .unwrap();
    h.service.process_pipeline_raw_lease(lease).await;
    let resolved_view = h
        .service
        .pipeline
        .kernel
        .read(|coordinator| coordinator.view(&consumer_hash).unwrap());
    assert!(resolved_view.dependencies.contains(&original.hash()));
    assert!(resolved_view.dependencies.contains(&group.hash()));

    let replacement = build_tx(&h.out_points[0], 3_000);
    h.service
        .process_tx(replacement.clone(), TxSource::Local)
        .await
        .expect("higher-fee replacement commits");
    let pool = h.service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&original.proposal_short_id())
            .is_none()
    );
    assert!(
        pool.get_tx_from_pool(&replacement.proposal_short_id())
            .is_some()
    );
    drop(pool);
    let view = h
        .service
        .pipeline
        .kernel
        .read(|coordinator| coordinator.view(&consumer_hash).unwrap());
    assert_eq!(
        view.location,
        PrePoolLocation::Wait(crate::component::pre_pool::WaitReason::Missing),
        "an accepted-pool removal invalidates the old resolution snapshot and waits for an exact availability edge"
    );

    h.cancel.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_preserves_order_for_dependent_txs() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let issue_out_point = &issue_out_points[0];

    // tx_a creates an output; tx_b spends it.
    let tx_a = build_tx(issue_out_point, 4_000);
    let tx_a_output = OutPoint::new(tx_a.hash(), 0);
    let tx_b = build_tx(&tx_a_output, 3_000);

    // Submit B first, then A. Because resolve is ordered, A should be resolved
    // and submitted before B is re-resolved against the pool.
    let tx_a_cycles = measured_cycles(&service, tx_a.clone()).await;
    // tx_b spends tx_a's output, so it cannot be measured until tx_a is in the
    // pool.  For the always-success script the verification cost is identical
    // for both transactions, so we reuse tx_a's cycle count for tx_b.
    submit_remote(&service, tx_b.clone(), tx_a_cycles, 1.into())
        .await
        .unwrap();

    submit_remote(&service, tx_a.clone(), tx_a_cycles, 1.into())
        .await
        .unwrap();

    if wait_for_pending(&service, 2, Duration::from_secs(10))
        .await
        .is_err()
    {
        let pool = service.pool.tx_pool.read().await;
        let (state, wait_state) = service.pipeline.kernel.read(|kernel| {
            (
                kernel
                    .hashes()
                    .into_iter()
                    .map(|hash| (hash.clone(), kernel.view(&hash)))
                    .collect::<Vec<_>>(),
                kernel.debug_wait_state(),
            )
        });
        panic!(
            "pipeline should process dependent txs in time: pending={}, pre_pool={state:?}, {wait_state}",
            pool.pool_map.pending_size()
        );
    }

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, 2);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rejects_conflicting_double_spend() {
    // Two remote txs spend the same chain output concurrently.
    // The pool must accept exactly one and reject the other; it must never
    // end up with both or panic.
    let tx_count = 2;
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(tx_count, 4);

    let shared_input = issue_out_points.first().expect("at least one issue out");
    let tx_a = build_tx(shared_input, 4_000);
    let id_a = tx_a.proposal_short_id();
    // tx_b spends the same input but pays to a different output so it has a
    // different hash.
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(shared_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_990).unwrap())
                .lock(Script::default())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let id_b = tx_b.proposal_short_id();

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    let service_a = service.clone();
    let service_b = service.clone();
    let handle_a =
        tokio::spawn(async move { submit_remote(&service_a, tx_a, cycles_a, 1.into()).await });
    let handle_b =
        tokio::spawn(async move { submit_remote(&service_b, tx_b, cycles_b, 1.into()).await });

    let (res_a, res_b) = tokio::join!(handle_a, handle_b);
    let _ = res_a.expect("task a should not panic");
    let _ = res_b.expect("task b should not panic");

    // Exactly one lands in the accepted pool. The loser remains non-executable
    // in the bounded conflict-history class until the winning input is freed.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, conflicts) = {
                let pool = service.pool.tx_pool.read().await;
                let conflicts = service
                    .pipeline
                    .kernel
                    .read(|kernel| kernel.conflict_hashes().len());
                (pool.pool_map.pending_size(), conflicts)
            };
            if pending == 1 && conflicts == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with exactly one double-spend tx accepted");

    let pool = service.pool.tx_pool.read().await;
    let a_in_pool = pool.get_tx_from_pool(&id_a).is_some();
    let b_in_pool = pool.get_tx_from_pool(&id_b).is_some();
    assert!(
        a_in_pool ^ b_in_pool,
        "exactly one of the double-spend txs must be in the pool, got a={a_in_pool} b={b_in_pool}"
    );
    assert_eq!(
        service.pipeline.kernel.read(|kernel| kernel.len()),
        1,
        "the double-spend loser has one bounded Conflict wait owner"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_accepts_dep_reader_after_in_flight_spender() {
    // tx_a spends an on-chain cell X. tx_b spends a different cell but uses X
    // as a cell dep. Their arrival order must not change accepted membership;
    // reader-before-spender is imposed only on the selected template set.
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline_workers(2, 4);
    let input_a = &issue_out_points[0];
    let input_b = &issue_out_points[1];

    let tx_a = build_tx(input_a, 4_000);
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .cell_dep(CellDep::new_builder().out_point(input_a.clone()).build())
        .input(CellInput::new(input_b.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_000).unwrap())
                .lock(Script::default())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let id_a = tx_a.proposal_short_id();
    let id_b = tx_b.proposal_short_id();

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    // Submit tx_a first and wait until it is actually in flight (either in the
    // verify queue or already accepted).  Only then submit tx_b so that the
    // cell-dep-on-in-flight-input path is exercised deterministically.
    submit_remote(&service, tx_a.clone(), cycles_a, 1.into())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, in_pipeline) = {
                let pool = service.pool.tx_pool.read().await;
                let in_pipeline = service
                    .pipeline
                    .kernel
                    .read(|coordinator| coordinator.contains_hash(&tx_a.hash()));
                (pool.pool_map.pending_size(), in_pipeline)
            };
            if pending == 1 || in_pipeline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_a should enter the pipeline");

    submit_remote(&service, tx_b.clone(), cycles_b, 2.into())
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                let pipeline_len = service
                    .pipeline
                    .kernel
                    .read(|coordinator| coordinator.len());
                (pool.pool_map.pending_size(), pipeline_len)
            };
            if pending >= 1 && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle to a valid dep-before-consumer state");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "spender is accepted"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "the earlier spender must not hide X from the later dep reader"
    );
    assert!(
        !pool.pool_map.calc_ancestors(&id_a).contains(&id_b),
        "conditional template order must not leak into causal ancestry"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_allows_same_cell_as_input_and_cell_dep() {
    // CKB permits a transaction to reference the same out-point both as an
    // input and as a cell dep. The pipeline must not reject such a tx with
    // OutPointError::Dead.
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline_workers(2, 4);
    let input_a = &issue_out_points[0];

    let tx_a = build_tx(input_a, 4_000);
    let output_a = OutPoint::new(tx_a.hash(), 0);

    // tx_b consumes tx_a's output and also references it as a cell dep.
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .cell_dep(CellDep::new_builder().out_point(output_a.clone()).build())
        .input(CellInput::new(output_a.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(3_000).unwrap())
                .lock(Script::default())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let id_a = tx_a.proposal_short_id();
    let id_b = tx_b.proposal_short_id();

    // Submit tx_a first and wait until it is accepted.
    service
        .process_tx(tx_a.clone(), TxSource::Local)
        .await
        .expect("tx_a should be accepted");
    wait_for_pending(&service, 1, Duration::from_secs(10))
        .await
        .expect("tx_a should settle");

    // Now tx_b's input and cell dep point to the same in-pool out-point.
    service
        .process_tx(tx_b.clone(), TxSource::Local)
        .await
        .expect("tx_b should be accepted even though its cell dep is also its input");

    wait_for_pending(&service, 2, Duration::from_secs(10))
        .await
        .expect("tx_b should settle");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be accepted"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "tx_b should be accepted even though its cell dep is also its input"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_processes_independent_secp_remote_txs() {
    // Verify that the concurrent pre-resolver handles real secp256k1 1-in-1-out
    // transactions (not always-success scripts) correctly.
    let tx_count = 10;
    let (service, _relay, signal, _store, issue_out_points, cell_deps) =
        secp_service_with_pipeline_workers(tx_count, 4);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_secp_tx(out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE))
        .collect();

    let mut cycles = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles.push(verify_cycles(&service, tx.clone()).await);
    }

    for (tx, cycles) in txs.iter().zip(&cycles) {
        submit_remote(&service, tx.clone(), *cycles, 1.into())
            .await
            .expect("enqueue secp remote tx should succeed");
    }

    wait_for_pending(&service, txs.len(), Duration::from_secs(60))
        .await
        .expect("pipeline should process all independent secp txs in time");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_preserves_order_for_dependent_secp_txs() {
    // Realistic dependent chain: parent is a secp256k1 1-in-1-out tx, child spends
    // its output.  Submitting child before parent must still end with both in the
    // pool because the ordered resolver/orphan recovery preserves order.
    let (service, _relay, signal, _store, issue_out_points, cell_deps) =
        secp_service_with_pipeline_workers(1, 4);
    let issue_out_point = &issue_out_points[0];

    let parent = build_secp_tx(issue_out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE);
    let parent_output = OutPoint::new(parent.hash(), 0);
    let child = build_secp_tx(
        &parent_output,
        &cell_deps,
        SECP_ISSUE_CAPACITY - 2 * SECP_FEE,
    );

    // Put parent into the pool temporarily so we can measure the child's exact
    // verification cycles, then remove it so the child must go through orphan
    // recovery when submitted before the parent.
    let parent_cycles = submit_local_tx(&service, parent.clone()).await;
    let child_cycles = verify_cycles(&service, child.clone()).await;
    service.remove_tx(parent.hash()).await;

    // Submit child first; it cannot resolve yet because the parent output is not
    // in the chain nor in any queue.
    submit_remote(&service, child.clone(), child_cycles, 1.into())
        .await
        .expect("enqueue child secp tx should succeed");

    submit_remote(&service, parent.clone(), parent_cycles, 1.into())
        .await
        .expect("enqueue parent secp tx should succeed");

    wait_for_pending(&service, 2, Duration::from_secs(20))
        .await
        .expect("pipeline should process dependent secp txs in order");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, 2);

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// An attached block can commit a remote transaction before its coordinator
/// worker reaches verification. Removing that sole lifecycle owner must also
/// publish the ingress success in the same reorg effect transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attached_commit_settles_pre_pool_remote_ingress() {
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;
    use std::collections::{HashSet, VecDeque};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    let hash = tx.hash();
    let peer = ckb_network::PeerIndex::from(77);
    assert!(
        submit_remote(&h.service, tx.clone(), 0, peer)
            .await
            .unwrap()
    );
    assert!(
        h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash))
    );

    let attached = BlockBuilder::default()
        .transaction(TransactionBuilder::default().build())
        .transaction(tx)
        .build();
    let snapshot = h.service.pool.tx_pool.read().await.cloned_snapshot();
    h.service
        .update_tx_pool_for_reorg(
            VecDeque::new(),
            VecDeque::from([attached]),
            HashSet::new(),
            snapshot,
        )
        .await
        .unwrap();
    assert!(
        !h.service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.contains_hash(&hash))
    );

    let relayed = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Ok(result) = h.relay_rx.try_recv() {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("chain commit must release the remote ingress filter");
    assert!(matches!(
        relayed,
        TxVerificationResult::Ok {
            original_peer: Some(relayed_peer),
            tx_hash,
        } if relayed_peer == peer && tx_hash == hash
    ));
    h.cancel.cancel();
}

/// Detached replay uses the synchronous direct entry point after releasing
/// the pool write lock. A transaction already present in the accepted pool is
/// an idempotent duplicate, not a failed parent whose dependents may be
/// cascade-removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reorg_direct_replay_treats_pool_duplicates_as_idempotent() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(3);

    // Submit 3 independent txs and wait for all to be pending.
    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    for tx in &txs {
        let cycles = measured_cycles(&service, tx.clone()).await;
        submit_remote(&service, tx.clone(), cycles, 1.into())
            .await
            .expect("enqueue remote tx should succeed");
    }

    wait_for_pending(&service, txs.len(), Duration::from_secs(10))
        .await
        .expect("all txs should be pending before reorg");

    // Build a "detached" block that contains the first 2 txs.
    // This simulates a block being orphaned during a reorg.
    let detached_block = BlockBuilder::default()
        .number(1)
        .parent_hash(service.pool.tx_pool.read().await.snapshot.tip_hash())
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .transaction(
            // cellbase (placeholder — skip(1) in reorg handler skips this)
            TransactionBuilder::default()
                .input(CellInput::new(OutPoint::null(), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(1_000).unwrap())
                        .build(),
                )
                .output_data(Bytes::default().pack())
                .build(),
        )
        .transaction(txs[0].clone())
        .transaction(txs[1].clone())
        .build();

    let detached_blocks: VecDeque<BlockView> = [detached_block].into();
    let attached_blocks: VecDeque<BlockView> = VecDeque::new();
    let detached_proposal_id: HashSet<ProposalShortId> = HashSet::new();
    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();

    // Trigger the reorg. Direct replay observes both retained transactions as
    // already accepted. The critical contract is:
    // - No panic
    // - Pool remains consistent
    // - No dependent is removed as a consequence of the duplicate result
    service
        .update_tx_pool_for_reorg(
            detached_blocks,
            attached_blocks,
            detached_proposal_id,
            snapshot,
        )
        .await
        .unwrap();

    // Allow any effects bound by the reorg transaction to settle.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Pool should still contain all 3 txs (reorg didn't remove anything
    // since attached was empty and the txs were in pending, not committed).
    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 3,
        "pool should still have all 3 txs after reorg with empty attached"
    );

    assert_eq!(
        service
            .pipeline
            .kernel
            .read(|coordinator| coordinator.len()),
        0,
        "coordinator should be empty after duplicate reorg recovery"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ---------------------------------------------------------------------------
// Additional helpers for specialized test configurations
// ---------------------------------------------------------------------------

/// Same as `service_with_pipeline` but enables RBF by setting `min_rbf_rate`
/// above `min_fee_rate`.
pub(crate) fn service_with_rbf(
    issue_outputs: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = crate::component::tests::harness::harness(issue_outputs)
        .rbf(true)
        .build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}
