use crate::helper::{deadlock_detection, wait_for_exit};
use crate::setup::{ExitCode, RunArgs};
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_db::{CacheDB, RocksDB};
use ckb_miner::BlockAssembler;
use ckb_network::{CKBProtocol, NetworkService, NetworkState, ProtocolId};
use ckb_notify::{NotifyController, NotifyService};
use ckb_rpc::RpcServer;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_shared::store::ChainStore;
use ckb_sync::{NetTimeProtocol, NetworkProtocol, Relayer, Synchronizer};
use ckb_traits::chain_provider::ChainProvider;
use log::info;

pub fn run(args: RunArgs) -> Result<(), ExitCode> {
    deadlock_detection();

    let shared = SharedBuilder::<CacheDB<RocksDB>>::default()
        .consensus(args.consensus)
        .db(&args.config.db)
        .tx_pool_config(args.config.tx_pool)
        .build();

    let notify = NotifyService::default().start(Some("notify"));

    let chain_controller = setup_chain(shared.clone(), notify.clone());
    info!(target: "main", "chain genesis hash: {:#x}", shared.genesis_hash());

    let block_assembler = BlockAssembler::new(shared.clone(), args.config.block_assembler);
    let block_assembler_controller = block_assembler.start(Some("MinerAgent"), &notify);

    let synchronizer =
        Synchronizer::new(chain_controller.clone(), shared.clone(), args.config.sync);

    let relayer = Relayer::new(
        chain_controller.clone(),
        shared.clone(),
        synchronizer.peers(),
    );

    let net_time_checker = NetTimeProtocol::default();

    let network_state =
        NetworkState::from_config(args.config.network).expect("Init network state failed");

    let protocols = vec![
        CKBProtocol::new(
            "syn".to_string(),
            NetworkProtocol::SYNC as ProtocolId,
            &[1][..],
            Box::new(synchronizer),
        ),
        CKBProtocol::new(
            "rel".to_string(),
            NetworkProtocol::RELAY as ProtocolId,
            &[1][..],
            Box::new(relayer),
        ),
        CKBProtocol::new(
            "tim".to_string(),
            NetworkProtocol::TIME as ProtocolId,
            &[1][..],
            Box::new(net_time_checker),
        ),
    ];

    let (network_service, p2p_service, mut network_controller) =
        NetworkService::build(network_state, protocols);
    let (network_runtime, network_thread_handle) =
        NetworkService::start(network_service, p2p_service).expect("Start network service failed");

    let rpc_server = RpcServer::new(
        args.config.rpc,
        network_controller.clone(),
        shared,
        chain_controller,
        block_assembler_controller,
    );

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");
    network_controller.shutdown();
    network_runtime.shutdown_now();
    network_thread_handle.join().expect("wait network thread");
    info!(target: "main", "Network shutdown");

    rpc_server.close();
    info!(target: "main", "Jsonrpc shutdown");

    Ok(())
}

fn setup_chain<CS: ChainStore + 'static>(
    shared: Shared<CS>,
    notify: NotifyController,
) -> ChainController {
    let chain_service = ChainBuilder::new(shared, notify).build();
    chain_service.start(Some("ChainService"))
}
