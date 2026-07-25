use super::*;

/// A parent can commit after a child resolver observed `Unknown` but before
/// it registers the wait. The atomic TxPool -> coordinator settlement must
/// requeue the child instead of installing a waiter after the only wake edge.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parent_commit_before_wait_registration_requeues_child() {
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::pipeline_ops::ParentWaitOutcome;
    use std::collections::HashSet;

    let h = harness(1).workers(WorkerSet::None).build();
    let parent = build_tx(&h.out_points[0], 4_000);
    let child = build_tx(&OutPoint::new(parent.hash(), 0), 3_000);
    let child_hash = child.hash();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            child,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
            RawStage::Resolve,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::Resolve)
        .unwrap();

    h.service
        .process_tx(parent.clone(), TxSource::Local)
        .await
        .expect("parent commits before child waiter registration");
    assert!(matches!(
        h.service
            .settle_raw_parent_wait(
                &lease,
                HashSet::from([parent.hash()]),
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
            .runtime
            .read(|coordinator| coordinator.view(&child_hash).unwrap().location),
        CoordinatorLocation::RawQueued(RawStage::Resolve)
    );

    h.cancel.cancel();
}

/// A remote transaction becomes externally observable as `UnknownParents`
/// only through the same coordinator transition that installs its durable
/// parent wait. This guards against cancellation leaving either a silent
/// waiter or a parent request with no owned transaction behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_parent_wait_and_unknown_parents_effect_are_one_transition() {
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::TxVerificationResult;
    use crate::service::pipeline_ops::ParentWaitOutcome;
    use ckb_types::packed::Byte32;
    use std::collections::HashSet;

    let h = harness(0).workers(WorkerSet::None).build();
    let parent = Byte32::new([42; 32]);
    let child = build_tx(&OutPoint::new(parent.clone(), 0), 3_000);
    let child_hash = child.hash();
    let peer = 7.into();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            child,
            TxSource::Remote { cycles: 0, peer },
            epoch,
            RawStage::Resolve,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::Resolve)
        .unwrap();

    let outcome = h
        .service
        .settle_raw_parent_wait(
            &lease,
            HashSet::from([parent.clone()]),
            h.service
                .reserve_effects(TxPoolService::unknown_parents_effect_bytes(1))
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(matches!(outcome, ParentWaitOutcome::Parked));
    assert_eq!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.view(&child_hash).unwrap().location),
        CoordinatorLocation::WaitingParents {
            missing: HashSet::from([parent.clone()])
        }
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
            assert_eq!(parents, HashSet::from([parent]));
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
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage};
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::RemoveTxOutcome;
    use std::collections::HashSet;

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
        .runtime
        .admit_transaction(
            consumer,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            h.service.current_pipeline_epoch().unwrap(),
            RawStage::Resolve,
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
            .runtime
            .read(|coordinator| coordinator.view(&consumer_hash).unwrap().location),
        CoordinatorLocation::WaitingParents {
            missing: HashSet::from([child.hash()])
        }
    );

    h.cancel.cancel();
}

/// Freeing an accepted input is the linearization point for historical
/// conflict recovery. Administrative removal records durable transfer work
/// under the pool lock; maintenance then moves the candidate to the sole
/// executable coordinator owner exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_pool_entry_transfers_unblocked_conflict_cache_candidate_once() {
    use crate::component::entry::TxEntry;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::service::RemoveTxOutcome;

    let h = harness(1).workers(WorkerSet::None).build();
    let blocker = build_tx(&h.out_points[0], 4_000);
    let candidate = build_tx(&h.out_points[0], 3_000);
    let candidate_hash = candidate.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.pool_map
            .add_entry(
                TxEntry::dummy_resolve(blocker.clone(), 0, Capacity::zero(), 100),
                Status::Pending,
            )
            .unwrap();
        pool.record_conflict(candidate.clone(), TxSource::Local);
    }

    assert_eq!(
        h.service.remove_tx(blocker.hash()).await,
        RemoveTxOutcome::Removed
    );
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.conflict_cache.contains_hash(&candidate_hash));
        assert_eq!(pool.conflict_recovery_len(), 0);
        assert_eq!(pool.conflict_discovery_len(), 1);
    }
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );

    let progress = h.service.recover_conflict_cache_slice(1).await;
    assert!(!progress.capacity_blocked);
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );
    let pool = h.service.pool.tx_pool.read().await;
    assert!(!pool.conflict_cache.contains_hash(&candidate_hash));
    assert_eq!(pool.conflict_recovery_len(), 0);
    assert_eq!(pool.conflict_discovery_len(), 0);
    drop(pool);

    assert!(!h.service.recover_conflict_cache_slice(1).await.saturated);
    h.cancel.cancel();
}

/// A historical Local candidate can be scheduled just before the same raw
/// hash arrives from the higher-trust Proposal path. Recovery must consume the
/// stale cache owner without asking the coordinator to downgrade or replace
/// the Proposal witness; the old behavior escalated `SourceDowngrade` into a
/// service-wide fail-stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflict_recovery_yields_to_existing_proposal_without_fail_stop() {
    use crate::component::pipeline_coordinator::{CoordinatorSource, RawStage};
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let historical = build_tx(&h.out_points[0], 3_000);
    let hash = historical.hash();
    let proposal = historical
        .as_advanced_builder()
        .set_witnesses(vec![Bytes::from_static(b"proposal-variant").pack()])
        .build();
    let proposal_witness = proposal.witness_hash();
    assert_eq!(proposal.hash(), hash);
    assert_ne!(proposal_witness, historical.witness_hash());
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.record_conflict(historical, TxSource::Local);
        assert_eq!(
            pool.schedule_conflict_candidates([hash.clone()].into_iter()),
            1
        );
    }
    let epoch = h.service.current_pipeline_epoch().expect("current epoch");
    assert!(
        h.service
            .pipeline
            .runtime
            .admit_transaction(proposal, TxSource::Proposal, epoch, RawStage::PreCheck)
            .expect("proposal admission")
            .0
    );

    let progress = h.service.recover_conflict_cache_slice(1).await;
    assert!(!progress.capacity_blocked);
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&hash),
        "the stronger coordinator owner consumes stale historical ownership"
    );
    h.service.pipeline.runtime.read(|coordinator| {
        assert_eq!(
            coordinator.view(&hash).expect("coordinator owner").source,
            CoordinatorSource::Proposal
        );
        assert_eq!(
            coordinator
                .raw_by_hash(&hash)
                .expect("proposal payload")
                .tx
                .witness_hash(),
            proposal_witness
        );
    });
    assert!(!h.service.pipeline.runtime.is_failed());
    assert!(!h.cancel.is_cancelled());
    h.cancel.cancel();
}

/// Pipeline clear is also an epoch barrier for cache-owned recovery work.
/// Historical conflict visibility remains, but an old scheduled transfer may
/// not recreate coordinator ownership after the clear returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_pipeline_cancels_conflict_recovery_schedule_without_deleting_history() {
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let candidate = build_tx(&h.out_points[0], 4_000);
    let candidate_hash = candidate.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.record_conflict(candidate, TxSource::Local);
        assert_eq!(
            pool.schedule_conflict_candidates(std::iter::once(candidate_hash.clone())),
            1
        );
        assert_eq!(pool.conflict_recovery_len(), 1);
    }

    h.service.clear_pipeline().await;
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.conflict_cache.contains_hash(&candidate_hash));
        assert_eq!(pool.conflict_recovery_len(), 0);
    }
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );
    let progress = h.service.recover_conflict_cache_slice(1).await;
    assert!(!progress.saturated && !progress.capacity_blocked);
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );

    h.cancel.cancel();
}

/// ConflictCache owns complete transaction identities, while PoolMap and the
/// proposal protocol can host only one transaction per short ID. A colliding
/// accepted entry must therefore park—not delete—the historical candidate;
/// once the protocol slot is free, the same cache generation can transfer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflict_recovery_retries_pool_short_id_collision_without_losing_history() {
    use crate::component::entry::TxEntry;
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(2).workers(WorkerSet::None).build();
    let mut accepted_hash = [0x42; 32];
    let mut cached_hash = accepted_hash;
    accepted_hash[31] = 1;
    cached_hash[31] = 2;
    let accepted = with_cached_hash(
        build_tx(&h.out_points[0], 4_000),
        ckb_types::packed::Byte32::new(accepted_hash),
    );
    let candidate = with_cached_hash(
        build_tx(&h.out_points[1], 3_000),
        ckb_types::packed::Byte32::new(cached_hash),
    );
    assert_eq!(accepted.proposal_short_id(), candidate.proposal_short_id());
    assert_ne!(accepted.hash(), candidate.hash());
    let accepted_tx_hash = accepted.hash();
    let candidate_hash = candidate.hash();
    let accepted_id = accepted.proposal_short_id();

    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.pool_map
            .add_entry(
                TxEntry::dummy_resolve(accepted.clone(), 0, Capacity::zero(), 100),
                Status::Pending,
            )
            .unwrap();
        pool.record_conflict(candidate, TxSource::Local);
        assert_eq!(
            pool.schedule_conflict_candidates(std::iter::once(candidate_hash.clone())),
            1
        );
    }

    let blocked = h.service.recover_conflict_cache_slice(1).await;
    assert!(blocked.capacity_blocked);
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.conflict_cache.contains_hash(&candidate_hash));
        assert_eq!(pool.conflict_recovery_len(), 1);
    }
    assert!(
        !h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );

    h.service
        .pool
        .tx_pool
        .write()
        .await
        .pool_map
        .remove_entry(&accepted_id)
        .expect("colliding accepted entry remains present");

    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction_journaled(
            accepted,
            TxSource::Local,
            epoch,
            crate::component::pipeline_coordinator::RawStage::Resolve,
            |_| {},
        )
        .unwrap();
    let coordinator_blocked = h.service.recover_conflict_cache_slice(1).await;
    assert!(coordinator_blocked.capacity_blocked);
    assert!(
        h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&candidate_hash)
    );
    h.service.pipeline.runtime.mutate_required(
        "test collision owner removal failed",
        |coordinator| {
            coordinator.force_terminalize(
                &accepted_tx_hash,
                crate::component::pipeline_coordinator::TerminalDisposition::Removed,
            )
        },
    );

    let recovered = h.service.recover_conflict_cache_slice(1).await;
    assert!(!recovered.capacity_blocked);
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&candidate_hash))
    );
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&candidate_hash)
    );

    h.cancel.cancel();
}

/// Failed detached recovery removes accepted descendants. Their independent
/// inputs are release events too: without scheduling ConflictCache discovery,
/// a valid historical competitor remains cache-owned forever even though its
/// blocker has disappeared.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_reorg_recovery_cascade_wakes_conflict_history() {
    use crate::component::entry::TxEntry;
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(2).workers(WorkerSet::None).build();
    let failed = build_tx(&h.out_points[0], 4_000);
    let failed_output = OutPoint::new(failed.hash(), 0);
    let independent_input = h.out_points[1].clone();
    let child = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(failed_output, 0))
        .input(CellInput::new(independent_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(3_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::default().pack())
        .build();
    let child_id = child.proposal_short_id();
    let competitor = build_tx(&independent_input, 3_500);
    let competitor_hash = competitor.hash();
    {
        let mut pool = h.service.pool.tx_pool.write().await;
        pool.pool_map
            .add_entry(
                TxEntry::dummy_resolve(child, 0, Capacity::zero(), 100),
                Status::Pending,
            )
            .unwrap();
        pool.record_conflict(competitor, TxSource::Local);
    }

    h.service.cascade_failed_reorg_recovery(&failed).await;
    {
        let pool = h.service.pool.tx_pool.read().await;
        assert!(pool.get_pool_entry(&child_id).is_none());
        assert!(pool.conflict_cache.contains_hash(&competitor_hash));
        assert_ne!(
            pool.conflict_discovery_len(),
            0,
            "the released independent input must become level-triggered work"
        );
    }

    let progress = h.service.recover_conflict_cache_slice(8).await;
    assert!(!progress.capacity_blocked);
    assert!(
        h.service
            .pipeline
            .runtime
            .read(|coordinator| coordinator.contains_hash(&competitor_hash))
    );
    assert!(
        !h.service
            .pool
            .tx_pool
            .read()
            .await
            .conflict_cache
            .contains_hash(&competitor_hash)
    );

    h.cancel.cancel();
}

/// A successful replacement removes an accepted parent without changing the
/// chain tip. Its already-resolved coordinator consumers must be demoted in
/// the same pool/coordinator commit transaction or they could commit using a
/// stale `ResolvedTransaction`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_rbf_commit_demotes_in_flight_consumers_of_removed_parent() {
    use crate::component::pipeline_coordinator::{CoordinatorLocation, RawStage, VerifySchedule};
    use crate::component::pipeline_runtime::resolved_charge_bytes;
    use crate::component::tests::harness::{WorkerSet, harness};
    use crate::resolved_tx::ResolveJob;
    use std::collections::HashSet;

    let h = harness(1).rbf(true).workers(WorkerSet::None).build();
    let original = build_tx(&h.out_points[0], 4_000);
    h.service
        .process_tx(original.clone(), TxSource::Local)
        .await
        .expect("original enters the accepted pool");

    let consumer = build_tx(&OutPoint::new(original.hash(), 0), 3_000);
    let consumer_hash = consumer.hash();
    let epoch = h.service.current_pipeline_epoch().unwrap();
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            consumer.clone(),
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
            RawStage::Resolve,
        )
        .unwrap();
    let lease = h
        .service
        .pipeline
        .runtime
        .checkout_raw(RawStage::Resolve)
        .unwrap();
    let resolved = match crate::resolve_mgr::resolve_job(
        &h.service,
        ResolveJob::new_at(
            consumer,
            TxSource::Remote {
                cycles: 0,
                peer: 1.into(),
            },
            epoch,
        ),
    )
    .await
    {
        crate::resolve_mgr::ResolveStageResult::Ready(resolved) => resolved,
        other => panic!("consumer should resolve against original: {other:?}"),
    };
    let charge = resolved_charge_bytes(&resolved).unwrap();
    h.service
        .pipeline
        .runtime
        .mutate(|coordinator| {
            coordinator.complete_raw(&lease, resolved, charge, VerifySchedule::default())
        })
        .unwrap();

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
        .runtime
        .read(|coordinator| coordinator.view(&consumer_hash).unwrap());
    assert_eq!(
        view.location,
        CoordinatorLocation::WaitingParents {
            missing: HashSet::from([original.hash()])
        }
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
    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: tx_a_cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: tx_a_cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("pipeline should process dependent txs in time");

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
    let handle_a = tokio::spawn(async move {
        service_a
            .submit_remote_tx(
                tx_a,
                TxSource::Remote {
                    cycles: cycles_a,
                    peer: 1.into(),
                },
            )
            .await
    });
    let handle_b = tokio::spawn(async move {
        service_b
            .submit_remote_tx(
                tx_b,
                TxSource::Remote {
                    cycles: cycles_b,
                    peer: 1.into(),
                },
            )
            .await
    });

    let (res_a, res_b) = tokio::join!(handle_a, handle_b);
    let _ = res_a.expect("task a should not panic");
    let _ = res_b.expect("task b should not panic");

    // Wait for the pipeline to drain. Both txs should leave the ordered/verify
    // queues and exactly one must land in the pending pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                let pipeline_len = service
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.len());
                (pool.pool_map.pending_size(), pipeline_len)
            };
            if pending == 1 && pipeline_len == 0 {
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

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_preserves_cell_dep_before_in_flight_consumer() {
    // tx_a spends an on-chain cell X. tx_b spends a different cell but uses X as
    // a cell dep. Both can coexist when tx_b commits first: the pool records tx_b
    // as tx_a's ancestor so block assembly uses X as a dep before consuming it.
    // If tx_a commits first, tx_b is correctly rejected as Dead. The concurrent
    // pipeline may reach either valid state, but never the invalid reverse order.
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
    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: cycles_a,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, in_pipeline) = {
                let pool = service.pool.tx_pool.read().await;
                let in_pipeline = service
                    .pipeline
                    .runtime
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

    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 2.into(),
            },
        )
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (pending, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                let pipeline_len = service
                    .pipeline
                    .runtime
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
        "tx_a should be accepted"
    );
    if pool.get_tx_from_pool(&id_b).is_some() {
        assert!(
            pool.pool_map.calc_ancestors(&id_a).contains(&id_b),
            "when both transactions are accepted, the dep user must precede the consumer"
        );
    }

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
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = {
                let pool = service.pool.tx_pool.read().await;
                pool.pool_map.pending_size()
            };
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tx_a should settle");

    // Now tx_b's input and cell dep point to the same in-pool out-point.
    service
        .process_tx(tx_b.clone(), TxSource::Local)
        .await
        .expect("tx_b should be accepted even though its cell dep is also its input");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = {
                let pool = service.pool.tx_pool.read().await;
                pool.pool_map.pending_size()
            };
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
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
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles: *cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue secp remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
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
    service
        .submit_remote_tx(
            child.clone(),
            TxSource::Remote {
                cycles: child_cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("enqueue child secp tx should succeed");

    service
        .submit_remote_tx(
            parent.clone(),
            TxSource::Remote {
                cycles: parent_cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("enqueue parent secp tx should succeed");

    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
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
        h.service
            .submit_remote_tx(tx.clone(), TxSource::Remote { cycles: 0, peer },)
            .await
            .unwrap()
    );
    assert!(
        h.service
            .pipeline
            .runtime
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
            .runtime
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

/// Test that `update_tx_pool_for_reorg` correctly routes retained (detached)
/// transactions through the pipeline entry point rather than blocking the
/// write lock with inline verification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_reorg_routes_retained_txs_through_classify() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(3);

    // Submit 3 independent txs and wait for all to be pending.
    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    for tx in &txs {
        let cycles = measured_cycles(&service, tx.clone()).await;
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue remote tx should succeed");
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == txs.len() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
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

    // Trigger the reorg. This should call classify_and_enqueue_tx for each
    // retained tx after releasing the write lock. The calls will fail with
    // "already in pool" errors (expected), but the critical thing is:
    // - No panic
    // - Pool remains consistent
    // - classify_and_enqueue_tx is exercised
    service
        .update_tx_pool_for_reorg(
            detached_blocks,
            attached_blocks,
            detached_proposal_id,
            snapshot,
        )
        .await
        .unwrap();

    // Give the pipeline a moment to process any classify_and_enqueue_tx calls.
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
            .runtime
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

/// Same as `service_with_rbf` but with a custom `max_tx_pool_size`. Used to
/// force `limit_size` to reject a replacement after the original transaction
/// has already been removed by RBF.
#[allow(clippy::type_complexity)]
pub(super) fn service_with_rbf_and_max_size(
    issue_outputs: usize,
    max_tx_pool_size: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
) {
    let h = crate::component::tests::harness::harness(issue_outputs)
        .rbf(true)
        .max_tx_pool_size(max_tx_pool_size)
        .build();
    (h.service, h.relay_rx, h.cancel, h.store, h.out_points)
}

/// Same as `secp_service_with_pipeline_workers` but also returns
/// `watch::Sender<ChunkCommand>` so tests can send Suspend/Resume signals.
#[allow(clippy::type_complexity)]
pub(super) fn secp_service_with_pipeline_workers_and_chunk(
    issue_outputs: usize,
    max_workers: usize,
) -> (
    TxPoolService,
    ckb_channel::Receiver<crate::service::TxVerificationResult>,
    CancellationToken,
    MockStore,
    Vec<OutPoint>,
    Vec<CellDep>,
    watch::Sender<ChunkCommand>,
) {
    let h = crate::component::tests::harness::harness(issue_outputs)
        .secp(true)
        .max_workers(max_workers)
        .with_chunk_sender(true)
        .build();
    (
        h.service,
        h.relay_rx,
        h.cancel,
        h.store,
        h.out_points,
        h.cell_deps.expect("secp harness provides cell deps"),
        h.chunk_tx.expect("chunk sender requested"),
    )
}

// ---------------------------------------------------------------------------
// Integration tests: dedup, worker cap, backpressure, pause/resume, RBF
// ---------------------------------------------------------------------------
