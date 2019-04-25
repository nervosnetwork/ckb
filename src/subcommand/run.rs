use crate::helper::{deadlock_detection, wait_for_exit};
use ckb_app_config::{ExitCode, RunArgs};
use ckb_chain::chain::{ChainBuilder, ChainController};
use ckb_db::{CacheDB, RocksDB};
use ckb_miner::BlockAssembler;
use ckb_network::{CKBProtocol, NetworkService, NetworkState};
use ckb_notify::{NotifyController, NotifyService};
use ckb_rpc::RpcServer;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_shared::store::ChainStore;
use ckb_sync::{NetTimeProtocol, NetworkProtocol, Relayer, Synchronizer};
use ckb_traits::chain_provider::ChainProvider;
use log::info;
use std::sync::Arc;

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

    let network_state = Arc::new(
        NetworkState::from_config(args.config.network).expect("Init network state failed"),
    );
    let synchronizer =
        Synchronizer::new(chain_controller.clone(), shared.clone(), args.config.sync);

    let relayer = Relayer::new(
        chain_controller.clone(),
        shared.clone(),
        synchronizer.peers(),
    );
    let net_timer = NetTimeProtocol::default();

    let protocols = vec![
        CKBProtocol::new(
            "syn".to_string(),
            NetworkProtocol::SYNC.into(),
            &["1".to_string()][..],
            move || Box::new(synchronizer.clone()),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new(
            "rel".to_string(),
            NetworkProtocol::RELAY.into(),
            &["1".to_string()][..],
            move || Box::new(relayer.clone()),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new(
            "tim".to_string(),
            NetworkProtocol::TIME.into(),
            &["1".to_string()][..],
            move || Box::new(net_timer.clone()),
            Arc::clone(&network_state),
        ),
    ];
    let network_controller = NetworkService::new(Arc::clone(&network_state), protocols)
        .start(Some("NetworkService"))
        .expect("Start network service failed");

    let rpc_server = RpcServer::new(
        args.config.rpc,
        network_controller,
        shared,
        chain_controller,
        block_assembler_controller,
    );

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

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
