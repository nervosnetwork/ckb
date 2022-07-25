use crate::tests::{always_success_transaction, setup, RpcTestRequest};
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionBuilder},
    packed::{CellDep, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};
use serde_json::json;
use std::{sync::Arc, thread::sleep, time::Duration};

#[test]
#[ignore]
fn test_get_block_template_cache() {
    let suite = setup();
    // block template cache will expire when new uncle block is added to the chain
    {
        let response_old = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });

        let store = suite.shared.store();
        let tip = store.get_tip_header().unwrap();
        let parent = store.get_block(&tip.parent_hash()).unwrap();
        let fork_block = parent
            .as_advanced_builder()
            .header(
                parent
                    .header()
                    .as_advanced_builder()
                    .timestamp((parent.header().timestamp() + 1).pack())
                    .build(),
            )
            .build();
        suite
            .chain_controller
            .process_block(Arc::new(fork_block))
            .expect("processing new block should be ok");

        assert_eq!(response_old.result["uncles"].to_string(), "[]");
        sleep(Duration::from_secs(4));
        let response_new = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });
        assert_ne!(response_new.result["uncles"].to_string(), "[]");
    }

    // block template cache will expire when new transaction is added to the pool
    {
        let response_old = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });

        let store = suite.shared.store();
        let tip = store.get_tip_header().unwrap();
        let tip_block = store.get_block(&tip.hash()).unwrap();
        let previous_output = OutPoint::new(tip_block.transactions().get(0).unwrap().hash(), 0);

        let input = CellInput::new(previous_output, 0);
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_cell().2.clone())
            .build();
        let cell_dep = CellDep::new_builder()
            .out_point(OutPoint::new(always_success_transaction().hash(), 0))
            .build();
        let tx = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(Default::default())
            .cell_dep(cell_dep)
            .build();

        let new_tx: ckb_jsonrpc_types::Transaction = tx.data().into();
        suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "send_transaction".to_string(),
            params: vec![json!(new_tx), json!("passthrough")],
        });

        assert_eq!(response_old.result["proposals"].to_string(), "[]");
        sleep(Duration::from_secs(4));
        let response_new = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });
        assert_ne!(response_new.result["proposals"].to_string(), "[]");
    }
}
