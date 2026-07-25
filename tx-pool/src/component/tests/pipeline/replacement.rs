use super::*;

/// Concurrent RBF replacements for the same input must be ordered by fee.
/// Only the highest-fee candidate should end up in the pool; lower-fee ones
/// must be rejected rather than temporarily displacing the original tx and
/// blocking the higher-fee candidate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_concurrent_rbf_prefers_highest_fee() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);
    let shared_input = &issue_out_points[0];

    // Original tx in pool: fee = 1000 bytes.
    let original = build_tx(shared_input, 4_000);
    let original_id = original.proposal_short_id();
    let original_cycles = measured_cycles(&service, original.clone()).await;
    service
        .submit_remote_tx(
            original,
            TxSource::Remote {
                cycles: original_cycles,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    // Wait until the original tx is actually in the pool before racing
    // replacements against it.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("original tx should be accepted");

    // Replacement candidates with strictly increasing fees.
    let replacements = vec![
        (3_500, 1), // fee 1500
        (3_000, 2), // fee 2000
        (2_500, 3), // fee 2500 (highest)
    ];

    let always_success_script = always_success_script();
    let mut handles = Vec::new();
    let mut ids = Vec::new();
    for (output_capacity, peer) in replacements {
        let tx = TransactionBuilder::default()
            .cell_dep(always_success_dep())
            .input(CellInput::new(shared_input.clone(), 0))
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(output_capacity).unwrap())
                    .lock(always_success_script.clone())
                    .build(),
            )
            .output_data(Bytes::default().pack())
            .witness(always_success_script.clone().into_witness())
            .build();
        ids.push((tx.proposal_short_id(), output_capacity));
        let cycles = measured_cycles(&service, tx.clone()).await;
        let svc = service.clone();
        handles.push(tokio::spawn(async move {
            svc.submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles,
                    peer: peer.into(),
                },
            )
            .await
        }));
    }

    for handle in handles {
        let _ = handle.await.expect("replacement task should not panic");
    }

    // Wait until the pipeline has fully settled and only the highest-fee
    // replacement remains in the pool.  Because remote submissions only block
    // until the tx is enqueued, a lower-fee candidate may briefly enter the
    // pool before a higher-fee candidate that is still racing through the
    // verify/submit stages replaces it.  Polling on the pool contents (not
    // just queue lengths) is required to avoid observing the transient state.
    let expected_id = ids
        .iter()
        .find(|(_, cap)| *cap == 2_500)
        .map(|(id, _)| id)
        .expect("highest-fee replacement exists")
        .clone();

    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let (pending, pipeline_len, settled) = {
                let pool = service.pool.tx_pool.read().await;
                let settled = pool.get_tx_from_pool(&original_id).is_none()
                    && pool.get_tx_from_pool(&expected_id).is_some()
                    && ids
                        .iter()
                        .all(|(id, _)| *id == expected_id || pool.get_tx_from_pool(id).is_none());
                (
                    pool.pool_map.pending_size(),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.len()),
                    settled,
                )
            };
            if pending == 1 && pipeline_len == 0 && settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("pipeline should settle with exactly one RBF replacement accepted");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&original_id).is_none(),
        "original tx should have been replaced"
    );
    assert!(
        pool.get_tx_from_pool(&expected_id).is_some(),
        "highest-fee replacement should be in the pool; ids={:?}",
        ids.iter()
            .map(|(id, cap)| (cap, pool.get_tx_from_pool(id).is_some()))
            .collect::<Vec<_>>()
    );

    // All other replacement ids should not be in the pool.
    for (id, cap) in &ids {
        if *id != expected_id {
            assert!(
                pool.get_tx_from_pool(id).is_none(),
                "lower-fee replacement {} should not be in the pool",
                cap
            );
        }
    }

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A multi-level RBF replacement must recover *all* removed transactions,
/// including descendants, in dependency order. If tx_a is replaced by a
/// higher-fee tx_r that is then rejected by the pool size limit, both tx_a
/// and its descendants tx_b and tx_c must be re-submitted so that parents
/// precede children in the recovery set.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rbf_rejected_replacement_recovers_descendants_in_order() {
    // Pool size large enough for a small three-tx chain but not for the
    // oversized replacement.
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_rbf_and_max_size(1, 2_000);

    let shared_input = &issue_out_points[0];

    // tx_a -> tx_b -> tx_c
    let tx_a = build_tx(shared_input, 4_998);
    let id_a = tx_a.proposal_short_id();
    let tx_b = build_tx(&OutPoint::new(tx_a.hash(), 0), 4_998);
    let id_b = tx_b.proposal_short_id();
    let tx_c = build_tx(&OutPoint::new(tx_b.hash(), 0), 4_998);
    let id_c = tx_c.proposal_short_id();

    // tx_r spends the same input as tx_a, pays a high enough fee to pass RBF
    // checks, but carries enough output data that it exceeds the tiny pool
    // limit once tx_a has been removed.
    let tx_r = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(shared_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(2_400).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::from(vec![0u8; 2_000]).pack())
        .build();
    let id_r = tx_r.proposal_short_id();

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;

    // Submit tx_a and wait for it to reach pending so that tx_b can be
    // resolved against the pool.
    service
        .submit_remote_tx(
            tx_a.clone(),
            TxSource::Remote {
                cycles: cycles_a,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_a should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should reach pending");

    let cycles_b = measured_cycles(&service, tx_b.clone()).await;
    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_b should be accepted");

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
    .expect("tx_b should reach pending");

    let cycles_c = measured_cycles(&service, tx_c.clone()).await;
    service
        .submit_remote_tx(
            tx_c.clone(),
            TxSource::Remote {
                cycles: cycles_c,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_c should be accepted");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_c should reach pending");

    let cycles_r = measured_cycles(&service, tx_r.clone()).await;

    // Submit the oversized replacement.
    let _ = service
        .submit_remote_tx(
            tx_r.clone(),
            TxSource::Remote {
                cycles: cycles_r,
                peer: 1.into(),
            },
        )
        .await;

    // Wait for the pipeline to drain and the original chain to be recovered.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (a_in_pool, b_in_pool, c_in_pool, r_in_pool, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                (
                    pool.get_tx_from_pool(&id_a).is_some(),
                    pool.get_tx_from_pool(&id_b).is_some(),
                    pool.get_tx_from_pool(&id_c).is_some(),
                    pool.get_tx_from_pool(&id_r).is_some(),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.len()),
                )
            };
            if a_in_pool && b_in_pool && c_in_pool && !r_in_pool && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("original chain should be recovered after rejected RBF replacement");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_r).is_none(),
        "oversized replacement should be rejected"
    );
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be recovered"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "tx_b (descendant) should be recovered"
    );
    assert!(
        pool.get_tx_from_pool(&id_c).is_some(),
        "tx_c (grand-descendant) should be recovered"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A successful RBF keeps the removed dependency tree in the historical
/// conflict cache. Removing the replacement first frees only the original
/// parent's confirmed input; each recovered parent acceptance must then make
/// its newly available outputs drive the next cached descendant. Without that
/// accepted-output event, the parent returns while child and grandchild remain
/// cache-owned forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_rbf_recovery_cascades_from_accepted_parent_outputs() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);
    let shared_input = &issue_out_points[0];

    let parent = build_tx(shared_input, 4_998);
    let child = build_tx(&OutPoint::new(parent.hash(), 0), 4_996);
    let grandchild = build_tx(&OutPoint::new(child.hash(), 0), 4_994);
    let original = [parent.clone(), child.clone(), grandchild.clone()];

    for (index, tx) in original.iter().enumerate() {
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
            .expect("original dependency entry should enqueue");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if service
                    .pool
                    .tx_pool
                    .read()
                    .await
                    .get_tx_from_pool(&tx.proposal_short_id())
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("original dependency entry {index} should reach the pool"));
    }

    let replacement = build_tx(shared_input, 4_900);
    let replacement_id = replacement.proposal_short_id();
    let replacement_cycles = measured_cycles(&service, replacement.clone()).await;
    service
        .submit_remote_tx(
            replacement.clone(),
            TxSource::Remote {
                cycles: replacement_cycles,
                peer: 2.into(),
            },
        )
        .await
        .expect("higher-fee replacement should enqueue");

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let settled = {
                let pool = service.pool.tx_pool.read().await;
                pool.get_tx_from_pool(&replacement_id).is_some()
                    && original.iter().all(|tx| {
                        pool.get_tx_from_pool(&tx.proposal_short_id()).is_none()
                            && pool.conflict_cache.contains_hash(&tx.hash())
                    })
            };
            if settled {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("successful RBF should move the complete original tree to history");

    assert_eq!(
        service.remove_tx(replacement.hash()).await,
        crate::service::RemoveTxOutcome::Removed
    );

    let recovered = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let recovered = {
                let pool = service.pool.tx_pool.read().await;
                original.iter().all(|tx| {
                    pool.get_tx_from_pool(&tx.proposal_short_id()).is_some()
                        && !pool.conflict_cache.contains_hash(&tx.hash())
                }) && pool.get_tx_from_pool(&replacement_id).is_none()
            };
            if recovered
                && service
                    .pipeline
                    .runtime
                    .read(|coordinator| coordinator.is_empty())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if recovered.is_err() {
        let pool = service.pool.tx_pool.read().await;
        let locations = original
            .iter()
            .map(|tx| {
                let id = tx.proposal_short_id();
                (
                    tx.hash(),
                    pool.get_tx_from_pool(&id).is_some(),
                    pool.conflict_cache.contains_hash(&tx.hash()),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.view(&tx.hash()).map(|view| view.location)),
                )
            })
            .collect::<Vec<_>>();
        panic!(
            "accepted parent outputs should recover the complete cached tree: locations={locations:?}, recovery={}, discovery={}",
            pool.conflict_recovery_len(),
            pool.conflict_discovery_len(),
        );
    }

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// A retained (detached) tx that is *already back in the pool* must be
/// treated as recovered, not as a failure: cascading on `Duplicated` would
/// evict its healthy dependents and emit spurious Dead rejections (this is
/// also what a retried reorg sees on its second pass).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reorg_retain_duplicate_does_not_cascade_dependents() {
    use std::collections::{HashSet, VecDeque};

    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);
    let issue_out_point = &issue_out_points[0];

    // Parent and its child, both pending in the pool.
    let parent = build_tx(issue_out_point, 4_000);
    let parent_output = OutPoint::new(parent.hash(), 0);
    let child = build_tx(&parent_output, 3_000);

    // Child first (it parks in the ordered queue), then the parent. The
    // child cannot be cycle-measured until the parent is in the pool, so
    // reuse the parent's (identical always-success script).
    let cycles = measured_cycles(&service, parent.clone()).await;
    for tx in [&child, &parent] {
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
            if pending == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("parent and child should be pending before reorg");

    // A detached block containing the parent: the retain loop re-adds it and
    // hits `Duplicated` (it never left the pool). Pre-fix this cascaded and
    // evicted the child with a spurious Dead rejection.
    let detached_block = BlockBuilder::default()
        .number(1)
        .parent_hash(service.pool.tx_pool.read().await.snapshot.tip_hash())
        .epoch(EpochNumberWithFraction::new(0, 0, 1).full_value())
        .transaction(
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
        .transaction(parent.clone())
        .build();

    let snapshot = service.pool.tx_pool.read().await.cloned_snapshot();
    service
        .update_tx_pool_for_reorg(
            [detached_block].into(),
            VecDeque::new(),
            HashSet::new(),
            snapshot,
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 2,
        "Duplicated retain must not cascade-remove the child"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
