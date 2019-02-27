use crate::helper::wait_for_exit;
use crate::Setup;
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_core::script::Script;
use ckb_db::diskdb::RocksDB;
use ckb_miner::{BlockAssembler, BlockAssemblerController};
use ckb_network::CKBProtocol;
use ckb_network::NetworkConfig;
use ckb_network::NetworkService;
use ckb_notify::{NotifyController, NotifyService};
use ckb_pow::PowEngine;
use ckb_rpc::{Config as RpcConfig, RpcServer};
use ckb_shared::cachedb::CacheDB;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_sync::{
    NetTimeProtocol, Relayer, Synchronizer, RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID, TIME_PROTOCOL_ID,
};
use ckb_traits::ChainProvider;
use crypto::secp::Generator;
use log::info;
use numext_fixed_hash::H256;
use std::sync::Arc;

pub fn run(setup: Setup) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let pow_engine = setup.chain_spec.pow_engine();

    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(consensus)
        .db(&setup.configs.db)
        .tx_pool_config(setup.configs.tx_pool.clone())
        .txs_verify_cache_size(setup.configs.txs_verify_cache_size)
        .build();

    let notify = NotifyService::default().start(Some("notify"));

    let chain_controller = setup_chain(shared.clone(), notify.clone());
    info!(target: "main", "chain genesis hash: {:#x}", shared.genesis_hash());
    // let tx_pool_controller = setup_tx_pool(setup.configs.pool, shared.clone(), notify.clone());

    let block_assembler =
        BlockAssembler::new(shared.clone(), setup.configs.block_assembler.type_hash);
    let block_assembler_controller = block_assembler.start(Some("MinerAgent"), &notify);

    let synchronizer = Arc::new(Synchronizer::new(
        chain_controller.clone(),
        shared.clone(),
        setup.configs.sync,
    ));

    let relayer = Arc::new(Relayer::new(
        chain_controller.clone(),
        shared.clone(),
        synchronizer.peers(),
    ));

    let net_time_checker = Arc::new(NetTimeProtocol::default());

    let network_config = NetworkConfig::from(setup.configs.network);
    let protocol_base_name = "ckb";
    let protocols = vec![
        CKBProtocol::new(
            protocol_base_name.to_string(),
            synchronizer as Arc<_>,
            SYNC_PROTOCOL_ID,
            &[1][..],
        ),
        CKBProtocol::new(
            protocol_base_name.to_string(),
            relayer as Arc<_>,
            RELAY_PROTOCOL_ID,
            &[1][..],
        ),
        CKBProtocol::new(
            protocol_base_name.to_string(),
            net_time_checker as Arc<_>,
            TIME_PROTOCOL_ID,
            &[1][..],
        ),
    ];
    let network = Arc::new(
        NetworkService::run_in_thread(&network_config, protocols)
            .expect("Create and start network"),
    );

    let rpc_server = setup_rpc(
        setup.configs.rpc,
        &pow_engine,
        Arc::clone(&network),
        shared,
        chain_controller,
        block_assembler_controller,
    );

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    rpc_server.close();
    info!(target: "main", "Jsonrpc shutdown");

    network.close();
    info!(target: "main", "Network shutdown");
}

fn setup_chain<CI: ChainIndex + 'static>(
    shared: Shared<CI>,
    notify: NotifyController,
) -> ChainController {
    let chain_service = ChainBuilder::new(shared, notify).build();
    chain_service.start(Some("ChainService"))
}

// fn setup_tx_pool<CI: ChainIndex + 'static>(
//     config: PoolConfig,
//     shared: Shared<CI>,
//     notify: NotifyController,
// ) -> TransactionPoolController {
//     let tx_pool_service = TransactionPoolService::new(config, shared, notify);
//     tx_pool_service.start(Some("TransactionPoolService"))
// }

fn setup_rpc<CI: ChainIndex + 'static>(
    config: RpcConfig,
    pow: &Arc<dyn PowEngine>,
    network: Arc<NetworkService>,
    shared: Shared<CI>,
    chain: ChainController,
    agent: BlockAssemblerController,
) -> RpcServer {
    use ckb_pow::Clicker;

    let pow = pow
        .as_ref()
        .as_any()
        .downcast_ref::<Clicker>()
        .map(|pow| Arc::new(pow.clone()));

    RpcServer::new(config, network, shared, chain, agent, pow)
}

pub fn type_hash(setup: &Setup) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let system_cell_tx = &consensus.genesis_block().commit_transactions()[0];
    let system_cell_data_hash = system_cell_tx.outputs()[0].data_hash();

    let script = Script::new(0, vec![], Some(system_cell_data_hash), None, vec![]);
    println!("{:#x}", script.type_hash());
}

pub fn keygen() {
    let result: H256 = Generator::new().random_privkey().into();
    println!("{:#x}", result);
}
