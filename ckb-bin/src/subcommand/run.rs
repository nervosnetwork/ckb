use crate::helper::{deadlock_detection, wait_for_exit};
use build_info::Version;
use ckb_alert_system::{alert_relayer::AlertRelayer, config::Config as AlertSystemConfig};
use ckb_app_config::{ExitCode, RunArgs};
use ckb_chain::chain::ChainService;
use ckb_db::RocksDB;
use ckb_miner::BlockAssembler;
use ckb_network::{CKBProtocol, NetworkService, NetworkState};
use ckb_notify::NotifyService;
use ckb_rpc::RpcServer;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_sync::{NetTimeProtocol, NetworkProtocol, Relayer, SyncSharedState, Synchronizer};
use ckb_traits::chain_provider::ChainProvider;
use ckb_verification::{BlockVerifier, Verifier};
use log::info;
use std::sync::Arc;

pub fn run(args: RunArgs, version: Version) -> Result<(), ExitCode> {
    deadlock_detection();

    let shared = SharedBuilder::<RocksDB>::new()
        .consensus(args.consensus)
        .db(&args.config.db)
        .tx_pool_config(args.config.tx_pool)
        .script_config(args.config.script)
        .store_config(args.config.store)
        .build()
        .map_err(|err| {
            eprintln!("Run error: {:?}", err);
            ExitCode::Failure
        })?;

    // Verify genesis every time starting node
    verify_genesis(&shared)?;

    let notify = NotifyService::default().start(Some("notify"));
    let chain_service = ChainService::new(shared.clone(), notify.clone());
    let chain_controller = chain_service.start(Some("ChainService"));
    info!(target: "main", "chain genesis hash: {:#x}", shared.genesis_hash());

    let block_assembler_controller =
        match (args.config.rpc.miner_enable(), args.config.block_assembler) {
            (true, Some(block_assembler)) => Some(
                BlockAssembler::new(shared.clone(), block_assembler)
                    .start(Some("MinerAgent"), &notify),
            ),
            _ => {
                info!(target: "main", "Miner is disabled, edit ckb.toml to enable it");

                None
            }
        };

    let network_state = Arc::new(
        NetworkState::from_config(args.config.network).expect("Init network state failed"),
    );
    let sync_shared_state = Arc::new(SyncSharedState::new(shared.clone()));
    let synchronizer = Synchronizer::new(
        chain_controller.clone(),
        Arc::clone(&sync_shared_state),
        args.config.sync,
    );

    let relayer = Relayer::new(
        chain_controller.clone(),
        sync_shared_state,
        synchronizer.peers(),
    );
    let net_timer = NetTimeProtocol::default();
    let alert_relayer = AlertRelayer::new(version.to_string(), AlertSystemConfig::default());

    let alert_notifier = alert_relayer.notifier();
    let alert_verifier = alert_relayer.verifier();

    let synchronizer_clone = synchronizer.clone();
    let protocols = vec![
        CKBProtocol::new(
            "syn".to_string(),
            NetworkProtocol::SYNC.into(),
            &["1".to_string()][..],
            move || Box::new(synchronizer_clone.clone()),
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
        CKBProtocol::new(
            "alt".to_string(),
            NetworkProtocol::ALERT.into(),
            &["1".to_string()][..],
            move || Box::new(alert_relayer.clone()),
            Arc::clone(&network_state),
        ),
    ];
    let network_controller = NetworkService::new(Arc::clone(&network_state), protocols)
        .start(version, Some("NetworkService"))
        .expect("Start network service failed");

    let rpc_server = RpcServer::new(
        args.config.rpc,
        network_controller,
        shared,
        synchronizer,
        chain_controller,
        block_assembler_controller,
        alert_notifier,
        alert_verifier,
    );

    wait_for_exit();

    info!(target: "main", "Finishing work, please wait...");

    rpc_server.close();
    info!(target: "main", "Jsonrpc shutdown");
    Ok(())
}

fn verify_genesis<CS: ChainStore + 'static>(shared: &Shared<CS>) -> Result<(), ExitCode> {
    let genesis = shared.consensus().genesis_block();
    BlockVerifier::new(shared.clone())
        .verify(genesis)
        .map_err(|err| {
            eprintln!("genesis error: {}", err);
            ExitCode::Config
        })
}
