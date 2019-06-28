use crate::module::{
    ChainRpc, ChainRpcImpl, ExperimentRpc, ExperimentRpcImpl, IndexerRpc, IndexerRpcImpl,
    NetworkRpc, NetworkRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use crate::RpcServer;
use ckb_chain::chain::{ChainController, ChainService};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::{alert::AlertBuilder, capacity_bytes, BlockNumber, Bytes, Capacity};
use ckb_dao_utils::genesis_dao_data;
use ckb_db::DBConfig;
use ckb_db::MemoryKeyValueDB;
use ckb_indexer::{DefaultIndexerStore, IndexerStore};
use ckb_network::{NetworkConfig, NetworkService, NetworkState};
use ckb_network_alert::{alert_relayer::AlertRelayer, config::Config as AlertConfig};
use ckb_notify::NotifyService;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainKVStore;
use ckb_sync::{SyncSharedState, Synchronizer};
use ckb_test_chain_utils::create_always_success_cell;
use ckb_traits::chain_provider::ChainProvider;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use numext_fixed_uint::U256;
use pretty_assertions::assert_eq as pretty_assert_eq;
use reqwest;
use serde_derive::Deserialize;
use serde_json::{from_reader, json, to_string_pretty, Map, Value};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

const GENESIS_TIMESTAMP: u64 = 1_557_310_743;
const ALERT_UNTIL_TIMESTAMP: u64 = 2_524_579_200;

#[derive(Debug, Deserialize, Clone)]
pub struct JsonResponse {
    pub jsonrpc: String,
    pub id: usize,
    pub result: Option<Value>,
    pub error: Option<Value>,
}

fn new_cellbase(number: BlockNumber, always_success_script: &Script) -> Transaction {
    let outputs = (0..1)
        .map(|_| {
            CellOutput::new(
                capacity_bytes!(500000),
                Bytes::default(),
                always_success_script.to_owned(),
                None,
            )
        })
        .collect::<Vec<_>>();
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(number))
        .outputs(outputs)
        .build()
}

fn setup_node(
    height: u64,
) -> (
    Shared<ChainKVStore<MemoryKeyValueDB>>,
    ChainController,
    RpcServer,
) {
    let (always_success_cell, always_success_script) = create_always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .witness(always_success_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .build();
    let dao = genesis_dao_data(&always_success_tx).unwrap();

    let consensus = {
        let genesis = BlockBuilder::default()
            .header_builder(
                HeaderBuilder::default()
                    .timestamp(GENESIS_TIMESTAMP)
                    .difficulty(U256::from(1000u64))
                    .dao(dao),
            )
            .transaction(always_success_tx)
            .build();
        Consensus::default()
            .set_genesis_block(genesis)
            .set_cellbase_maturity(0)
    };
    let shared = SharedBuilder::<MemoryKeyValueDB>::new()
        .consensus(consensus)
        .build()
        .unwrap();
    let chain_service = {
        let notify = NotifyService::default().start::<&str>(None);
        ChainService::new(shared.clone(), notify)
    };
    let chain_controller = chain_service.start::<&str>(None);
    let mut parent = {
        let consensus = shared.consensus();
        consensus.genesis_block().clone()
    };

    // Build chain, insert [1, height) blocks
    for _i in 0..height {
        let epoch = {
            let last_epoch = shared
                .get_block_epoch(&parent.header().hash())
                .expect("current epoch exists");
            shared
                .next_epoch_ext(&last_epoch, parent.header())
                .unwrap_or(last_epoch)
        };
        let cellbase = new_cellbase(parent.header().number() + 1, &always_success_script);
        let dao = genesis_dao_data(&cellbase).unwrap();
        let block = BlockBuilder::default()
            .transaction(cellbase)
            .header_builder(
                HeaderBuilder::default()
                    .parent_hash(parent.header().hash().to_owned())
                    .number(parent.header().number() + 1)
                    .epoch(epoch.number())
                    .timestamp(parent.header().timestamp() + 1)
                    .difficulty(epoch.difficulty().clone())
                    .dao(dao),
            )
            .build();
        chain_controller
            .process_block(Arc::new(block.clone()), false)
            .expect("processing new block should be ok");
        parent = block;
    }

    // Start network services
    let dir = tempfile::Builder::new()
        .prefix("ckb_resource_test")
        .tempdir()
        .unwrap();
    let mut network_config = NetworkConfig::default();
    network_config.path = dir.path().to_path_buf();
    network_config.ping_interval_secs = 1;
    network_config.ping_timeout_secs = 1;
    network_config.connect_outbound_interval_secs = 1;

    File::create(dir.path().join("network")).expect("create network database");
    let network_state =
        Arc::new(NetworkState::from_config(network_config).expect("Init network state failed"));
    let network_controller = NetworkService::new(
        Arc::clone(&network_state),
        Vec::new(),
        shared.consensus().identify_name(),
    )
    .start::<&str>(Default::default(), None)
    .expect("Start network service failed");
    let sync_shared_state = Arc::new(SyncSharedState::new(shared.clone()));
    let synchronizer = Synchronizer::new(chain_controller.clone(), Arc::clone(&sync_shared_state));

    let db_config = DBConfig {
        path: dir.path().join("indexer").to_path_buf(),
        ..Default::default()
    };
    let indexer_store = DefaultIndexerStore::new(&db_config, shared.clone());
    indexer_store.insert_lock_hash(&always_success_script.hash(), Some(0));
    // use hardcoded BATCH_ATTACH_BLOCK_NUMS (100) value here to setup testing data.
    (0..=height / 100).for_each(|_| indexer_store.sync_index_states());

    // Start rpc services
    let mut io = IoHandler::new();
    io.extend_with(
        ChainRpcImpl {
            shared: shared.clone(),
        }
        .to_delegate(),
    );
    io.extend_with(PoolRpcImpl::new(shared.clone(), network_controller.clone()).to_delegate());
    io.extend_with(
        NetworkRpcImpl {
            network_controller: network_controller.clone(),
        }
        .to_delegate(),
    );
    let alert_relayer = AlertRelayer::new("0.1.0".to_string(), AlertConfig::default());

    let alert_notifier = {
        let alert_notifier = alert_relayer.notifier();
        let alert = Arc::new(
            AlertBuilder::default()
                .id(42)
                .min_version(Some("0.0.1".into()))
                .max_version(Some("1.0.0".into()))
                .priority(1)
                .notice_until(ALERT_UNTIL_TIMESTAMP * 1000)
                .message("An example alert message!".into())
                .build(),
        );
        alert_notifier.lock().add(alert);
        Arc::clone(alert_notifier)
    };
    io.extend_with(
        StatsRpcImpl {
            shared: shared.clone(),
            synchronizer: synchronizer.clone(),
            alert_notifier,
        }
        .to_delegate(),
    );
    io.extend_with(
        IndexerRpcImpl {
            store: indexer_store,
        }
        .to_delegate(),
    );
    io.extend_with(
        ExperimentRpcImpl {
            shared: shared.clone(),
        }
        .to_delegate(),
    );
    let server = ServerBuilder::new(io)
        .cors(DomainsValidation::AllowOnly(vec![
            AccessControlAllowOrigin::Null,
            AccessControlAllowOrigin::Any,
        ]))
        .threads(1)
        .max_request_body_size(20_000_000)
        .start_http(&"127.0.0.1:0".parse().unwrap())
        .expect("JsonRpc initialize");
    let rpc_server = RpcServer { server };

    (shared, chain_controller, rpc_server)
}

#[test]
fn test_rpc() {
    // Set `print_mode = true` manually to print the result of rpc test cases.
    // It is useful when we just want get the actual results.
    let print_mode = false;

    // Setup node
    let height = 1024;
    let (_shared, _chain_controller, server) = setup_node(height);

    // Load cases in json format and run
    let mut cases: Value = {
        let mut file_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        file_path.push("json/rpc.json");
        let file = File::open(file_path).expect("opening test data json");
        from_reader(file).expect("reading test data json")
    };
    let mut outputs: Vec<Value> = Vec::new();

    // Run cases one by one
    let client = reqwest::Client::new();
    for case in cases
        .as_array_mut()
        .expect("test data is array format")
        .iter_mut()
    {
        if let Some(skip) = case.get("skip") {
            if skip.as_bool().unwrap_or(false) {
                outputs.push(case.clone());
                continue;
            }
        }

        let case = case.as_object_mut().expect("case object should be map");
        let method = case.get("method").expect("get case method");
        let params = case.get("params").expect("get case params");
        let request = {
            let mut request = Map::new();
            request.insert("id".to_owned(), json!(1));
            request.insert("jsonrpc".to_owned(), json!("2.0"));
            request.insert("method".to_owned(), method.clone());
            request.insert("params".to_owned(), params.clone());
            Value::Object(request)
        };
        // TODO handle error response
        let uri = format!(
            "http://{}:{}/",
            server.server.address().ip(),
            server.server.address().port()
        );
        let response: JsonResponse = client
            .post(&uri)
            .json(&json!(request))
            .send()
            .expect("send jsonrpc request")
            .json()
            .expect("transform jsonrpc response into json");
        let actual = response.result.clone().unwrap_or_else(|| Value::Null);
        let expect = case.remove("result").expect("get case result");

        // Print only at print_mode, otherwise do real testing asserts
        if print_mode {
            case.insert("result".to_owned(), actual.clone());
            outputs.push(Value::Object(case.clone()));
        } else {
            pretty_assert_eq!(
                expect,
                actual,
                "Expect RPC {}",
                case.get("method").expect("get jsonrpc method")
            );
        }

        //// Uncomment the code below if you wanna print request of `send_transaction`.
        //// Print rpc request of `send_transaction` at print_mode.
        //// It is just a convenient way to get the json of `send_transaction`
        // if print_mode {
        //     let tip_header = {
        //         let chain_state = shared.lock_chain_state();
        //         chain_state.tip_header().clone()
        //     };
        //     let cellbase = new_cellbase(tip_header.number());
        //     let transaction = TransactionBuilder::default()
        //         .input(CellInput::new(
        //             ckb_core::transaction::OutPoint::new(tip_header.hash().clone(), cellbase.hash().clone(), 0),
        //                 0,
        //                 Vec::new(),
        //         ))
        //         .output(
        //             CellOutput::new(capacity_bytes!(1000), Bytes::new(), Script::always_success(), None),
        //         )
        //         .build();
        //     let json_transaction: ckb_jsonrpc_types::Transaction = (&transaction).into();
        //     let mut object = Map::new();
        //     object.insert("id".to_owned(), json!(1));
        //     object.insert("jsonrpc".to_owned(), json!("2.0"));
        //     object.insert("method".to_owned(), json!("send_transaction"));
        //     object.insert("params".to_owned(), json!(vec![json_transaction]));
        //     let response: JsonResponse = client.post(&uri)
        //         .json(&object)
        //         .send()
        //         .expect("send jsonrpc request")
        //         .json()
        //         .expect("transform send_transaction response into json");
        //     object.insert("result".to_owned(), response.result.clone());
        //     object.remove("id");
        //     object.remove("jsonrpc");
        //     println!("{}", to_string_pretty(&Value::Array(object)).unwrap());
        // }
    }

    if print_mode {
        println!("{}", to_string_pretty(&Value::Array(outputs)).unwrap());
    }

    server.close();
}
