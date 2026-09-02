use super::*;

#[test]
fn controller_repeated_idle_then_burst_never_loses_compute_wake() {
    const BURSTS: usize = 32;
    const TRANSACTIONS: usize = 30;

    let executor = Arc::new(BenchExecutor::new());
    let data = BenchData::new(
        TxType::AlwaysSuccess,
        BURSTS * TRANSACTIONS,
        0,
        Arc::clone(&executor),
    );
    let handle = data.shared.start_controller(8);

    for burst in 0..BURSTS {
        let start = burst * TRANSACTIONS;
        let end = start + TRANSACTIONS;
        let txs = Arc::new(data.txs[start..end].to_vec());
        let cycles = Arc::new(
            txs.iter()
                .map(|tx| *data.cycles.get(&tx.hash()).expect("missing cycle"))
                .collect(),
        );
        let settled = executor.runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_secs(5),
                submit_and_wait_inner(&handle.controller, &handle.completion, txs, cycles, end, 1),
            )
            .await
        });
        assert!(
            settled.is_ok(),
            "burst {burst} lost the idle-to-burst compute wake: {}/{} accepted",
            handle.completion.completed.load(Ordering::Acquire),
            end
        );
    }
}

#[test]
fn controller_dependent_secp_chain_reverse() {
    let executor = Arc::new(BenchExecutor::new());
    let (mut shared, cell_deps) = SharedBench::new_secp(2, executor);
    shared.secp_cell_deps = Some(cell_deps);
    let mut txs = build_single_dependent_chain(&shared, 2);
    let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxProcess);
    txs.reverse();
    eprintln!("dependent secp chain cycles {:?}", cycles);
    let handle = shared.start_controller(4);
    shared.executor.runtime.block_on(async {
        for tx in txs.iter() {
            let c = cycles.get(&tx.hash()).copied().expect("missing cycle");
            handle
                .controller
                .submit_remote_tx(tx.clone(), c, 1.into())
                .await
                .expect("submit");
        }
        for i in 0..200 {
            let info = handle.controller.get_tx_pool_info().unwrap();
            eprintln!(
                "iter {i} pending={} orphan={}",
                info.pending_size, info.orphan_size
            );
            if info.pending_size >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(handle.controller.get_tx_pool_info().unwrap().pending_size >= 2);
    });
}

#[test]
fn profile_span_counters_observe_only_the_active_registered_window() {
    let counters = ProfileSpanCounters::new();
    counters.record(PROFILE_SPAN_NAMES[0]);
    counters.begin().expect("activate counters");
    counters.record(PROFILE_SPAN_NAMES[0]);
    counters.record(PROFILE_SPAN_NAMES[0]);
    counters.record(PROFILE_SPAN_NAMES[PROFILE_SPAN_NAMES.len() - 1]);
    let snapshot = counters.end().expect("close counters");
    assert_eq!(snapshot[0], 2);
    assert_eq!(snapshot[PROFILE_SPAN_NAMES.len() - 1], 1);
    assert_eq!(snapshot.iter().sum::<u64>(), 3);

    counters.record(PROFILE_SPAN_NAMES[0]);
    counters.begin().expect("reactivate counters");
    assert_eq!(
        counters
            .end()
            .expect("close reset counters")
            .iter()
            .sum::<u64>(),
        0
    );
}

#[test]
fn profile_span_counters_reject_an_unregistered_target_span() {
    let counters = ProfileSpanCounters::new();
    counters.begin().expect("activate counters");
    counters.record("tx_pool.unregistered");
    let error = counters.end().expect_err("unknown span must fail");
    assert!(error.contains("1 unregistered target spans"));
}

#[test]
fn profile_span_registry_is_sorted_unique_and_owns_scheduler_gate_lifetimes() {
    assert!(
        PROFILE_SPAN_NAMES
            .array_windows::<2>()
            .all(|[left, right]| left < right)
    );
    for required in [
        "tx_pool.scheduler.fairness_stage_hold",
        "tx_pool.scheduler.fairness_stage_wait",
        "tx_pool.scheduler.queue_stage_hold",
        "tx_pool.scheduler.queue_stage_wait",
    ] {
        assert!(PROFILE_SPAN_NAMES.binary_search(&required).is_ok());
    }
}
