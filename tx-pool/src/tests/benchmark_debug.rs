use super::*;

#[test]
fn controller_dependent_secp_chain_reverse() {
    let (mut shared, cell_deps) = SharedBench::new_secp(2);
    shared.secp_cell_deps = Some(cell_deps);
    let mut txs = build_single_dependent_chain(&shared, 2);
    let cycles = measure_cycles(&shared, &txs, MeasureMode::PerTxProcess);
    txs.reverse();
    eprintln!("dependent secp chain cycles {:?}", cycles);
    let handle = shared.start_controller(4);
    shared.runtime.block_on(async {
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
