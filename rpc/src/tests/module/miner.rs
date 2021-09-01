use crate::{
    tests::{always_success_transaction, next_block, RpcTestRequest, RpcTestSuite},
    RpcServer, ServiceBuilder,
};
use ckb_app_config::{BlockAssemblerConfig, NetworkConfig, RpcConfig, RpcModule};
use ckb_chain::chain::ChainService;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_launcher::SharedBuilder;
use ckb_network::{DefaultExitHandler, NetworkService, NetworkState};
use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cell, always_success_consensus};
use ckb_types::{
    core::{capacity_bytes, Capacity, FeeRate, TransactionBuilder},
    h256,
    packed::{CellDep, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
    H256,
};
use reqwest::blocking::Client;
use serde_json::json;
use std::{sync::Arc, thread::sleep, time::Duration};

#[test]
fn test_get_block_template_cache() {
    let suite = setup();
    // block template cache will expire after 3 seconds
    {
        let response_old = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });

        sleep(Duration::from_secs(4));
        let response_new = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "get_block_template".to_string(),
            params: vec![],
        });
        assert_ne!(response_old.json(), response_new.json());
    }

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

fn setup() -> RpcTestSuite {
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(always_success_consensus())
        .block_assembler_config(Some(BlockAssemblerConfig {
            code_hash: h256!("0x0"),
            args: Default::default(),
            hash_type: ScriptHashType::Data,
            message: Default::default(),
            use_binary_version_as_message_prefix: false,
            binary_version: "TEST".to_string(),
        }))
        .build()
        .unwrap();
    let chain_controller =
        ChainService::new(shared.clone(), pack.take_proposal_table()).start::<&str>(None);

    // Start network services
    let dir = tempfile::tempdir()
        .expect("create tempdir failed")
        .path()
        .to_path_buf();
    let network_controller = {
        let network_config = NetworkConfig {
            path: dir,
            ping_interval_secs: 1,
            ping_timeout_secs: 1,
            connect_outbound_interval_secs: 1,
            ..Default::default()
        };
        let network_state =
            Arc::new(NetworkState::from_config(network_config).expect("Init network state failed"));
        NetworkService::new(
            Arc::clone(&network_state),
            Vec::new(),
            Vec::new(),
            shared.consensus().identify_name(),
            "0.1.0".to_string(),
            DefaultExitHandler::default(),
        )
        .start(shared.async_handle())
        .expect("Start network service failed")
    };

    pack.take_tx_pool_builder()
        .start(network_controller.clone());

    // Build chain, insert 20 blocks
    let mut parent = shared.consensus().genesis_block().clone();

    for _ in 0..20 {
        let block = next_block(&shared, &parent.header());
        chain_controller
            .process_block(Arc::new(block.clone()))
            .expect("processing new block should be ok");
        parent = block;
    }

    // Start rpc services
    let rpc_config = RpcConfig {
        listen_address: "127.0.0.1:0".to_owned(),
        tcp_listen_address: None,
        ws_listen_address: None,
        max_request_body_size: 20_000_000,
        threads: None,
        modules: vec![RpcModule::Chain, RpcModule::Miner, RpcModule::Pool],
        reject_ill_transactions: false,
        enable_deprecated_rpc: false,
        extra_well_known_lock_scripts: vec![],
        extra_well_known_type_scripts: vec![],
    };

    let builder = ServiceBuilder::new(&rpc_config)
        .enable_chain(shared.clone())
        .enable_pool(shared.clone(), FeeRate::zero(), true, vec![], vec![])
        .enable_miner(
            shared.clone(),
            network_controller,
            chain_controller.clone(),
            true,
        );
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(rpc_config, io_handler, shared.notify_controller());
    let rpc_uri = format!(
        "http://{}:{}/",
        rpc_server.http_address().ip(),
        rpc_server.http_address().port()
    );
    let rpc_client = Client::new();
    RpcTestSuite {
        shared,
        chain_controller,
        rpc_server,
        rpc_uri,
        rpc_client,
    }
}
