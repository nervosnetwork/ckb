use crate::{Node, Spec, utils::wait_until};
use ckb_app_config::{CKBAppConfig, RpcModule};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, FeeRate, capacity_bytes},
    packed::OutPoint,
    prelude::*,
};
use serde_json::{Value, json};

pub struct IndexerPoolRecovery;
pub struct RichIndexerPoolRecovery;

impl Spec for IndexerPoolRecovery {
    fn modify_app_config(&self, config: &mut CKBAppConfig) {
        configure(config, RpcModule::Indexer);
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        exercise_recovery(&mut nodes[0]);
    }
}

impl Spec for RichIndexerPoolRecovery {
    fn modify_app_config(&self, config: &mut CKBAppConfig) {
        configure(config, RpcModule::RichIndexer);
    }

    fn run(&self, nodes: &mut Vec<Node>) {
        exercise_recovery(&mut nodes[0]);
    }
}

fn configure(config: &mut CKBAppConfig, module: RpcModule) {
    config.rpc.modules.push(module);
    config.indexer.index_tx_pool = true;
    config.indexer.poll_interval = 1;
    config.tx_pool.min_fee_rate = FeeRate::zero();
    config.tx_pool.min_rbf_rate = FeeRate::from_u64(1000);
}

fn exercise_recovery(node: &mut Node) {
    node.mine_until_out_bootstrap_period();
    let lock = node
        .always_success_script()
        .as_builder()
        .args(Bytes::from_static(b"indexer-pool"))
        .build();
    let funding = node.new_transaction_spend_tip_cellbase();
    let output = funding
        .output(0)
        .unwrap()
        .as_builder()
        .lock(lock.clone())
        .build();
    let funding = funding
        .as_advanced_builder()
        .set_outputs(vec![output])
        .build();
    node.submit_transaction(&funding);
    node.mine_until_transaction_confirm(&funding.hash());
    let funding_tip = node.get_tip_block().hash();
    let point = OutPoint::new(funding.hash(), 0);
    let search = json!({
        "script": ckb_jsonrpc_types::Script::from(lock),
        "script_type": "lock",
        "script_search_mode": "exact",
    });
    assert_view(node, &search, &point, true);

    let spend = node.new_transaction_with_since_capacity(funding.hash(), 0, capacity_bytes!(99));
    node.submit_transaction(&spend);
    assert_view(node, &search, &point, false);

    // Both transactions consume the same input. Removing the replaced owner
    // must not expose the cell while its successor remains accepted.
    let replacement =
        node.new_transaction_with_since_capacity(funding.hash(), 0, capacity_bytes!(98));
    node.submit_transaction(&replacement);
    assert_eq!(node.get_tip_tx_pool_info().pending.value(), 1);
    assert!(node.remove_transaction(spend.hash()));
    assert_view(node, &search, &point, false);
    assert!(node.remove_transaction(replacement.hash()));
    assert_view(node, &search, &point, true);

    node.submit_transaction(&spend);
    // Pool recovery and indexer startup do not share a notification history.
    // The existing Windows harness cannot preserve a pool across graceful stop.
    #[cfg(not(target_os = "windows"))]
    {
        node.stop_gracefully();
        node.start();
        assert!(wait_until(30, || node
            .get_tip_tx_pool_info()
            .pending
            .value()
            == 1));
    }
    assert_view(node, &search, &point, false);
    assert!(node.remove_transaction(spend.hash()));
    assert_view(node, &search, &point, true);

    // Rapid add/remove cycles cannot leave a stale consumed-cell filter.
    for _ in 0..3 {
        node.submit_transaction(&spend);
        assert!(node.remove_transaction(spend.hash()));
    }
    assert_view(node, &search, &point, true);

    node.submit_transaction(&spend);
    assert_view(node, &search, &point, false);
    node.mine_until_transaction_confirm(&spend.hash());
    assert_view(node, &search, &point, false);

    // Truncation clears the pool and restores the cell without a successor
    // block to trigger the usual parent-hash mismatch check.
    node.rpc_client().truncate(funding_tip);
    node.assert_tx_pool_size(0, 0);
    assert_view(node, &search, &point, true);
}

#[track_caller]
fn assert_view(node: &Node, search: &Value, point: &OutPoint, visible: bool) {
    let rpc = node.rpc_client().inner();
    let tip = json!(ckb_types::H256::from(node.get_tip_block().hash()));
    let point = json!(ckb_jsonrpc_types::OutPoint::from(point.clone()));
    let expected_capacity = json!(ckb_jsonrpc_types::Uint64::from(if visible {
        capacity_bytes!(100).as_u64()
    } else {
        0
    }));
    let mut observed = Value::Null;
    assert!(
        wait_until(30, || {
            let indexed = rpc.get_indexer_tip().unwrap();
            let page = rpc.get_cells(search.clone(), "asc", 2.into()).unwrap();
            let cells = page["objects"].as_array().expect("indexer cell page");
            let capacity = rpc
                .get_cells_capacity(search.clone())
                .unwrap()
                .map_or(json!("0x0"), |value| value["capacity"].clone());
            let matches_cells = if visible {
                cells.len() == 1 && cells[0]["out_point"] == point
            } else {
                cells.is_empty()
            };
            let matches = indexed
                .as_ref()
                .is_some_and(|indexed| indexed["block_hash"] == tip)
                && matches_cells
                && capacity == expected_capacity;
            observed = json!({"tip": indexed, "page": page, "capacity": capacity});
            matches
        }),
        "indexer view did not converge: visible={visible}, tip={tip}, observed={observed}"
    );
}
