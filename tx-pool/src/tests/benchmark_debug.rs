use super::*;

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
