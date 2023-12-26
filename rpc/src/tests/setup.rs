use crate::{
    tests::{always_success_transaction, next_block},
    RpcServer, ServiceBuilder,
};
use ckb_app_config::{
    BlockAssemblerConfig, NetworkAlertConfig, NetworkConfig, RpcConfig, RpcModule,
};
use ckb_chain::start_chain_services;
use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_chain_spec::versionbits::{ActiveMode, Deployment, DeploymentPos};
use ckb_dao_utils::genesis_dao_data;
use ckb_network::{Flags, NetworkService, NetworkState};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_notify::NotifyService;
use ckb_shared::SharedBuilder;
use ckb_sync::SyncShared;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::{thread::sleep, time::Duration};

use ckb_types::{
    core::{BlockBuilder, Capacity, EpochNumberWithFraction, Ratio},
    h256,
    packed::{AlertBuilder, RawAlertBuilder},
    prelude::*,
};

use super::RpcTestSuite;

const GENESIS_TIMESTAMP: u64 = 1_557_310_743;
const GENESIS_TARGET: u32 = 0x2001_0000;
const EPOCH_REWARD: u64 = 125_000_000_000_000;
const CELLBASE_MATURITY: u64 = 0;
const ALERT_UNTIL_TIMESTAMP: u64 = 2_524_579_200;

// Construct `Consensus` with an always-success cell
pub(crate) fn always_success_consensus() -> Consensus {
    let always_success_tx = always_success_transaction();
    let dao = genesis_dao_data(vec![&always_success_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(GENESIS_TIMESTAMP.pack())
        .compact_target(GENESIS_TARGET.pack())
        .epoch(EpochNumberWithFraction::new_unchecked(0, 0, 0).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();
    let mut deployments = HashMap::new();
    let test_dummy = Deployment {
        bit: 1,
        start: 0,
        timeout: 0,
        min_activation_epoch: 0,
        period: 10,
        active_mode: ActiveMode::Never,
        threshold: Ratio::new(3, 4),
    };
    deployments.insert(DeploymentPos::Testdummy, test_dummy);
    ConsensusBuilder::default()
        .genesis_block(genesis)
        .initial_primary_epoch_reward(Capacity::shannons(EPOCH_REWARD))
        .cellbase_maturity(EpochNumberWithFraction::from_full_value(CELLBASE_MATURITY))
        .softfork_deployments(deployments)
        .build()
}

fn json_bytes(hex: &str) -> ckb_jsonrpc_types::JsonBytes {
    serde_json::from_value(json!(hex)).expect("JsonBytes")
}

// Setup the running environment
pub(crate) fn setup_rpc_test_suite(height: u64, consensus: Option<Consensus>) -> RpcTestSuite {
    let consensus = consensus.unwrap_or_else(always_success_consensus);
    let (shared, mut pack) = SharedBuilder::with_temp_db()
        .consensus(consensus)
        .block_assembler_config(Some(BlockAssemblerConfig {
            code_hash: h256!("0x1892ea40d82b53c678ff88312450bbb17e164d7a3e0a90941aa58839f56f8df2"),
            hash_type: ckb_jsonrpc_types::ScriptHashType::Type,
            args: json_bytes("0xb2e61ff569acf041b3c2c17724e2379c581eeac3"),
            message: "message".pack().into(),
            use_binary_version_as_message_prefix: true,
            binary_version: "TEST".to_string(),
            update_interval_millis: 800,
            notify: vec![],
            notify_scripts: vec![],
            notify_timeout_millis: 800,
        }))
        .build()
        .unwrap();
    let chain_controller = start_chain_services(pack.take_chain_services_builder());

    // Start network services
    let temp_dir = tempfile::tempdir().expect("create tmp_dir failed");

    let network_controller = {
        let network_config = NetworkConfig {
            path: temp_dir.path().join("network").to_path_buf(),
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
            (
                shared.consensus().identify_name(),
                "0.1.0".to_string(),
                Flags::COMPATIBILITY,
            ),
        )
        .start(shared.async_handle())
        .expect("Start network service failed")
    };

    pack.take_tx_pool_builder()
        .start(network_controller.clone());

    let tx_pool = shared.tx_pool_controller();
    while !tx_pool.service_started() {
        sleep(Duration::from_millis(400));
    }

    // Build chain, insert [1, height) blocks
    let mut parent = shared.consensus().genesis_block().clone();

    for _ in 0..height {
        let block = next_block(&shared, &parent.header());
        chain_controller
            .blocking_process_block(Arc::new(block.clone()))
            .expect("processing new block should be ok");
        parent = block;
    }

    let sync_shared = Arc::new(SyncShared::new(
        shared.clone(),
        Default::default(),
        pack.take_relay_tx_receiver(),
    ));

    let notify_controller =
        NotifyService::new(Default::default(), shared.async_handle().clone()).start();
    let (alert_notifier, alert_verifier) = {
        let alert_relayer = AlertRelayer::new(
            "0.1.0".to_string(),
            notify_controller,
            NetworkAlertConfig::default(),
        );
        let alert_notifier = alert_relayer.notifier();
        let alert = AlertBuilder::default()
            .raw(
                RawAlertBuilder::default()
                    .id(42u32.pack())
                    .min_version(Some("0.0.1".to_string()).pack())
                    .max_version(Some("1.0.0".to_string()).pack())
                    .priority(1u32.pack())
                    .notice_until((ALERT_UNTIL_TIMESTAMP * 1000).pack())
                    .message("An example alert message!".pack())
                    .build(),
            )
            .build();
        alert_notifier.lock().add(&alert);
        (
            Arc::clone(alert_notifier),
            Arc::clone(alert_relayer.verifier()),
        )
    };

    // Start rpc services
    let rpc_config = RpcConfig {
        listen_address: "127.0.0.1:0".to_owned(),
        tcp_listen_address: Some("127.0.0.1:0".to_owned()),
        ws_listen_address: None,
        max_request_body_size: 20_000_000,
        threads: None,
        // enable all rpc modules in unit test
        modules: vec![
            RpcModule::Net,
            RpcModule::Chain,
            RpcModule::Miner,
            RpcModule::Pool,
            RpcModule::Experiment,
            RpcModule::Stats,
            RpcModule::IntegrationTest,
            RpcModule::Alert,
            RpcModule::Subscription,
            RpcModule::Debug,
        ],
        reject_ill_transactions: true,
        // enable deprecated rpc in unit test
        enable_deprecated_rpc: true,
        extra_well_known_lock_scripts: vec![],
        extra_well_known_type_scripts: vec![],
    };

    let builder = ServiceBuilder::new(&rpc_config)
        .enable_chain(shared.clone())
        .enable_pool(shared.clone(), vec![], vec![])
        .enable_miner(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
            true,
        )
        .enable_net(
            network_controller.clone(),
            sync_shared,
            Arc::new(chain_controller.clone()),
        )
        .enable_stats(shared.clone(), Arc::clone(&alert_notifier))
        .enable_experiment(shared.clone())
        .enable_integration_test(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
        )
        .enable_debug()
        .enable_alert(alert_verifier, alert_notifier, network_controller);

    let io_handler = builder.build();
    let shared_clone = shared.clone();
    let handler = shared_clone.async_handle().clone();
    let rpc_server = RpcServer::new(rpc_config, io_handler, handler.clone());

    let rpc_client = reqwest::blocking::Client::new();
    let rpc_uri = format!(
        "http://{}:{}/",
        rpc_server.http_address.ip(),
        rpc_server.http_address.port()
    );
    let tcp_uri = rpc_server
        .tcp_address
        .as_ref()
        .map(|addr| format!("{}:{}", addr.ip(), addr.port()));

    let suite = RpcTestSuite {
        shared,
        chain_controller: chain_controller.clone(),
        rpc_uri,
        tcp_uri,
        rpc_client,
        _tmp_dir: temp_dir,
    };

    suite.wait_block_template_number(height + 1);
    // insert a fork block for rpc `get_fork_block` test
    {
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
        chain_controller
            .blocking_process_block(Arc::new(fork_block))
            .expect("processing new block should be ok");
    }

    suite
}
