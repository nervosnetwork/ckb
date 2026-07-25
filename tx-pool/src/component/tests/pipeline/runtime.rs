use super::*;

#[tokio::test]
async fn cancelled_runtime_does_not_checkout_queued_raw_work() {
    use crate::component::pipeline_coordinator::RawStage;
    use crate::component::tests::harness::{WorkerSet, harness};

    let h = harness(1).workers(WorkerSet::None).build();
    let tx = build_tx(&h.out_points[0], 4_000);
    h.service
        .pipeline
        .runtime
        .admit_transaction(
            tx,
            TxSource::Local,
            h.service.pipeline.epoch.current().unwrap(),
            RawStage::PreCheck,
        )
        .unwrap();

    h.cancel.cancel();
    let lease = tokio::time::timeout(
        Duration::from_secs(1),
        h.service.pipeline.runtime.wait_raw(RawStage::PreCheck),
    )
    .await
    .expect("cancelled waiter must terminate");
    assert!(lease.is_none(), "shutdown must prevent a fresh checkout");
}

/// Submitting the same transaction twice must not duplicate it in the pool.
/// The second submission should be silently deduplicated — the pool must
/// contain exactly one copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_dedup_double_submission() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_pipeline(1);

    let tx = build_tx(&issue_out_points[0], 4_000);
    let id = tx.proposal_short_id();
    let cycles = measured_cycles(&service, tx.clone()).await;

    // First submission.
    service
        .submit_remote_tx(
            tx.clone(),
            TxSource::Remote {
                cycles,
                peer: 1.into(),
            },
        )
        .await
        .expect("first submission should succeed");

    // Wait for the tx to reach the pending pool.
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
    .expect("tx should reach pending");

    // Second submission of the same tx.
    // The coordinator and accepted pool jointly deduplicate the submission;
    // pool_map.add_entry also returns `inserted == false` for an existing short ID.
    // Either way, the pool must still have exactly 1 tx.
    let second_result = service
        .submit_remote_tx(
            tx.clone(),
            TxSource::Remote {
                cycles,
                peer: 1.into(),
            },
        )
        .await;
    // The result may be Ok (silent dedup in pool_map) or Err(Duplicated).
    // Both are correct behavior — what matters is the pool state.
    assert!(matches!(
        second_result,
        Ok(_) | Err(crate::error::Reject::Duplicated(_))
    ));

    // Brief wait for any in-flight processing.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, 1,
        "pool must have exactly 1 tx after duplicate submission"
    );

    // Verify the specific tx is still in the pool.
    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id).is_some(),
        "original tx should still be in pool"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// Creating a service with `max_tx_verify_workers` far exceeding the machine's
/// available parallelism should not panic or cause resource issues. The
/// coordinator pre-check worker cap (`min(max_workers, available_parallelism)`) should
/// keep the actual worker count reasonable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_high_pre_check_worker_cap() {
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(5, 1000);

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
    .expect("pipeline should process all txs even with high worker cap");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// With `max_workers = 1` (semaphore capacity = 2), flooding the pipeline
/// with many concurrent submissions must not lose any transactions. The
/// semaphore provides backpressure: when all permits are consumed, the actor
/// loop blocks on `acquire_owned()`, but no messages are dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_semaphore_backpressure() {
    let tx_count = 10;
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_pipeline_workers(tx_count, 1);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_tx(out_point, 4_000))
        .collect();

    let mut cycles_vec = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles_vec.push(measured_cycles(&service, tx.clone()).await);
    }

    // Submit all txs concurrently. With semaphore cap = 2, at most 2
    // process() calls run simultaneously. All 10 must still complete.
    let mut handles = Vec::new();
    for (tx, cycles) in txs.iter().zip(&cycles_vec) {
        let svc = service.clone();
        let tx = tx.clone();
        let cycles = *cycles;
        handles.push(tokio::spawn(async move {
            svc.submit_remote_tx(
                tx,
                TxSource::Remote {
                    cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("submit under backpressure should succeed");
        }));
    }
    for h in handles {
        h.await.expect("submit task should not panic");
    }

    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
            if pending == tx_count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("all txs should reach pending despite semaphore backpressure");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(
        pending, tx_count,
        "semaphore backpressure must not lose transactions"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// The `ChunkCommand` watch signal propagates from `TxPoolController` through
/// `VerifyMgr` and `OrderedResolver`. This test verifies the signal path
/// end-to-end using real secp256k1 transactions:
///
/// 1. Submit secp txs with 1 worker (slow, sequential verification).
/// 2. Send `ChunkCommand::Suspend` — VerifyMgr stops picking up new work.
/// 3. Send `ChunkCommand::Resume` — verification resumes.
/// 4. All txs must eventually reach pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_chunk_command_pause_resume() {
    let (service, _relay, signal, _store, issue_out_points, cell_deps, chunk_tx) =
        secp_service_with_pipeline_workers_and_chunk(4, 1);

    let txs: Vec<_> = issue_out_points
        .iter()
        .map(|out_point| build_secp_tx(out_point, &cell_deps, SECP_ISSUE_CAPACITY - SECP_FEE))
        .collect();

    let mut cycles_vec = Vec::with_capacity(txs.len());
    for tx in &txs {
        cycles_vec.push(verify_cycles(&service, tx.clone()).await);
    }

    for (tx, cycles) in txs.iter().zip(&cycles_vec) {
        service
            .submit_remote_tx(
                tx.clone(),
                TxSource::Remote {
                    cycles: *cycles,
                    peer: 1.into(),
                },
            )
            .await
            .expect("enqueue secp tx should succeed");
    }

    // Brief yield to let the first tx start verifying.
    tokio::time::sleep(Duration::from_millis(5)).await;

    // Suspend — VerifyMgr stops checking out new coordinator verify tickets.
    chunk_tx
        .send(ChunkCommand::Suspend)
        .expect("send suspend signal");

    // Wait briefly while suspended. In-flight verification continues, but
    // no new verify tickets are checked out from the coordinator.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pending_while_suspended = service.pool.tx_pool.read().await.pool_map.pending_size();

    // Resume — remaining txs should now drain through verification.
    chunk_tx
        .send(ChunkCommand::Resume)
        .expect("send resume signal");

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
    .expect("all txs should reach pending after resume");

    let pending = service.pool.tx_pool.read().await.pool_map.pending_size();
    assert_eq!(pending, txs.len());

    // With 1 worker and suspend, some txs should have been delayed.
    assert!(
        pending_while_suspended < txs.len(),
        "suspend should have delayed some txs (got {pending_while_suspended}/{})",
        txs.len()
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// When RBF is enabled (`min_rbf_rate > min_fee_rate`), submitting a
/// higher-fee transaction that spends the same input as an existing
/// lower-fee transaction should:
///
/// 1. Remove the lower-fee tx from the pool (via `process_rbf`).
/// 2. Insert the higher-fee tx.
/// 3. Exercise the authoritative conflict-cache bookkeeping for the displaced
///    transaction.
///
/// This tests the full RBF → pool state transition path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rbf_displaces_lower_fee_tx() {
    let (service, _relay, signal, _store, issue_out_points) = service_with_rbf(1);

    let shared_input = &issue_out_points[0];

    // tx_a: lower fee (input 5000 CKB → output 4998 CKB, fee = 2 CKB).
    let tx_a = build_tx(shared_input, 4_998);
    let id_a = tx_a.proposal_short_id();

    // tx_b: higher fee, same input (output 4990 CKB, fee = 10 CKB).
    let tx_b = build_tx(shared_input, 4_990);
    let id_b = tx_b.proposal_short_id();

    assert_ne!(id_a, id_b, "txs must have different proposal_short_ids");

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    // Submit tx_a and wait for it to reach pending.
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

    {
        let pool = service.pool.tx_pool.read().await;
        assert!(
            pool.get_tx_from_pool(&id_a).is_some(),
            "tx_a should be in pool before replacement"
        );
    }

    // Submit tx_b — triggers RBF, displacing tx_a.
    service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 1.into(),
            },
        )
        .await
        .expect("tx_b (RBF replacement) should be accepted");

    // Wait for RBF to complete: tx_b must appear in the pool, which can only
    // happen after tx_a is removed (they conflict on the same input).
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (b_in_pool, pipeline_len) = {
                let pool = service.pool.tx_pool.read().await;
                (
                    pool.get_tx_from_pool(&id_b).is_some(),
                    service
                        .pipeline
                        .runtime
                        .read(|coordinator| coordinator.len()),
                )
            };
            if b_in_pool && pipeline_len == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("RBF should complete: tx_a displaced, tx_b in pool");

    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_a).is_none(),
        "tx_a should be removed after RBF"
    );
    assert!(
        pool.get_tx_from_pool(&id_b).is_some(),
        "tx_b (higher fee) should be in pool after RBF"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}

/// When an RBF replacement is rejected by the pool (e.g. it no longer fits
/// after the old tx is removed), the original conflicted transaction must be
/// recovered rather than silently dropped. This prevents a remote peer from
/// evicting an in-pool tx by submitting a replacement that passes RBF checks
/// but fails insertion.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pipeline_rbf_rejected_replacement_recovers_original_tx() {
    // Pool size just large enough for the small original tx but not for the
    // large replacement.
    let (service, _relay, signal, _store, issue_out_points) =
        service_with_rbf_and_max_size(1, 1_500);

    let shared_input = &issue_out_points[0];

    // tx_a: small tx in the pool.
    let tx_a = build_tx(shared_input, 4_998);
    let id_a = tx_a.proposal_short_id();

    // tx_b: higher fee, same input, but with a large output_data so its
    // serialized size exceeds the tiny pool limit after tx_a is removed.
    let tx_b = TransactionBuilder::default()
        .cell_dep(always_success_dep())
        .input(CellInput::new(shared_input.clone(), 0))
        .output(
            CellOutput::new_builder()
                .capacity(Capacity::bytes(4_000).unwrap())
                .lock(always_success_script())
                .build(),
        )
        .output_data(Bytes::from(vec![0u8; 2_000]).pack())
        .build();
    let id_b = tx_b.proposal_short_id();

    assert_ne!(id_a, id_b, "txs must have different proposal_short_ids");

    let cycles_a = measured_cycles(&service, tx_a.clone()).await;
    let cycles_b = measured_cycles(&service, tx_b.clone()).await;

    // Submit tx_a and wait for it to reach pending.
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

    // Submit tx_b. This merely enqueues the tx and returns Ok; actual
    // success/failure is determined by inspecting the final pool state.
    let _ = service
        .submit_remote_tx(
            tx_b.clone(),
            TxSource::Remote {
                cycles: cycles_b,
                peer: 1.into(),
            },
        )
        .await
        .unwrap();

    // Failed replacement rollback restores tx_a synchronously under the pool
    // write guard. The observable outcome is tx_a back in the pool.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let pool = service.pool.tx_pool.read().await;
                if pool.get_tx_from_pool(&id_a).is_some() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("tx_a should be recovered after the rejected replacement");

    // tx_b passes RBF checks, removes tx_a, but is then rejected by
    // `limit_size` because the pool is too small. tx_a must be recovered from
    // the waiting room rather than left out of the mempool.
    let pool = service.pool.tx_pool.read().await;
    assert!(
        pool.get_tx_from_pool(&id_b).is_none(),
        "tx_b should be rejected because it exceeds the tiny pool size"
    );
    assert!(
        pool.get_tx_from_pool(&id_a).is_some(),
        "tx_a should be recovered after the rejected replacement"
    );

    signal.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
}
