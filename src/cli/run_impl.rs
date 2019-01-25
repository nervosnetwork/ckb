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
use ckb_pool::txs_pool::{PoolConfig, TransactionPoolController, TransactionPoolService};
use ckb_pow::PowEngine;
use ckb_rpc::RpcServer;
use ckb_shared::cachedb::CacheDB;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared, SharedBuilder};
use ckb_shared::store::ChainKVStore;
use ckb_sync::{
    NetTimeProtocol, Relayer, Synchronizer, RELAY_PROTOCOL_ID, SYNC_PROTOCOL_ID, TIME_PROTOCOL_ID,
};
use crypto::secp::Generator;
use log::info;
use numext_fixed_hash::H256;
use std::sync::{atomic::AtomicUsize, Arc};
use std::thread;

pub fn run(setup: Setup) {
    let consensus = setup.chain_spec.to_consensus().unwrap();
    let pow_engine = setup.chain_spec.pow_engine();

    let shared = SharedBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_rocks(&setup.configs.db)
        .consensus(consensus)
        .build();

    let (_handle, notify) = NotifyService::default().start(Some("notify"));
    let (chain_controller, chain_receivers) = ChainController::build();
    let (block_assembler_controller, block_assembler_receivers) = BlockAssemblerController::build();

    let chain_service = ChainBuilder::new(shared.clone())
        .notify(notify.clone())
        .build();
    let _handle = chain_service.start(Some("ChainService"), chain_receivers);

    info!(target: "main", "chain genesis hash: {:#x}", shared.genesis_hash());

    let tx_pool_controller = setup_tx_pool(setup.configs.pool, shared.clone(), notify.clone());

    let block_assembler = BlockAssembler::new(
        shared.clone(),
        tx_pool_controller.clone(),
        setup.configs.block_assembler.type_hash,
    );
    let _handle = block_assembler.start(Some("MinerAgent"), block_assembler_receivers, &notify);

    let synchronizer = Arc::new(Synchronizer::new(
        chain_controller.clone(),
        shared.clone(),
        setup.configs.sync,
    ));

    let relayer = Arc::new(Relayer::new(
        chain_controller.clone(),
        shared.clone(),
        tx_pool_controller.clone(),
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

    let rpc_server = RpcServer {
        config: setup.configs.rpc,
    };

    setup_rpc(
        rpc_server,
        &pow_engine,
        Arc::clone(&network),
        shared,
        tx_pool_controller,
        chain_controller,
        block_assembler_controller,
    );

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");
}

fn setup_tx_pool<CI: ChainIndex + 'static>(
    config: PoolConfig,
    shared: Shared<CI>,
    notify: NotifyController,
) -> TransactionPoolController {
    let last_txs_updated_at = Arc::new(AtomicUsize::new(0));
    let (tx_pool_controller, tx_pool_receivers) =
        TransactionPoolController::build(Arc::clone(&last_txs_updated_at));
    let tx_pool_service = TransactionPoolService::new(config, shared, notify, last_txs_updated_at);
    let _handle = tx_pool_service.start(Some("TransactionPoolService"), tx_pool_receivers);
    tx_pool_controller
}

fn setup_rpc<CI: ChainIndex + 'static>(
    server: RpcServer,
    pow: &Arc<dyn PowEngine>,
    network: Arc<NetworkService>,
    shared: Shared<CI>,
    tx_pool: TransactionPoolController,
    chain: ChainController,
    agent: BlockAssemblerController,
) {
    use ckb_pow::Clicker;

    let pow = pow
        .as_ref()
        .as_any()
        .downcast_ref::<Clicker>()
        .map(|pow| Arc::new(pow.clone()));

    let _ = thread::Builder::new().name("rpc".to_string()).spawn({
        move || {
            server.start(network, shared, tx_pool, chain, agent, pow);
        }
    });
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
