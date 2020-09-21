use crate::helper::deadlock_detection;
use ckb_app_config::{BlockAssemblerConfig, ExitCode, RunArgs};
use ckb_build_info::Version;
use ckb_chain::chain::ChainService;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_logger::info_target;
use ckb_network::{
    CKBProtocol, DefaultExitHandler, ExitHandler, NetworkService, NetworkState, SupportProtocols,
};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_resource::Resource;
use ckb_rpc::{RpcServer, ServiceBuilder};
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::ChainStore;
use ckb_sync::{NetTimeProtocol, Relayer, SyncShared, Synchronizer};
use ckb_types::{core::cell::setup_system_cell_cache, prelude::*};
use ckb_verification::{GenesisVerifier, Verifier};
use std::sync::Arc;

const SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN: usize = 20;

pub fn run(args: RunArgs, version: Version) -> Result<(), ExitCode> {
    deadlock_detection();

    let block_assembler_config = sanitize_block_assembler_config(&args)?;
    let miner_enable = block_assembler_config.is_some();
    let exit_handler = DefaultExitHandler::default();

    let (shared, table) = SharedBuilder::with_db_config(&args.config.db)
        .consensus(args.consensus)
        .tx_pool_config(args.config.tx_pool)
        .notify_config(args.config.notify)
        .store_config(args.config.store)
        .block_assembler_config(block_assembler_config)
        .build()
        .map_err(|err| {
            eprintln!("Run error: {:?}", err);
            ExitCode::Failure
        })?;

    // Verify genesis every time starting node
    verify_genesis(&shared)?;

    setup_system_cell_cache(
        shared.consensus().genesis_block(),
        &shared.store().cell_provider(),
    );

    rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("RayonGlobal-{}", i))
        .build_global()
        .expect("Init the global thread pool for rayon failed");

    ckb_memory_tracker::track_current_process(
        args.config.memory_tracker.interval,
        Some(shared.store().db().inner()),
    );

    let chain_service = ChainService::new(shared.clone(), table);
    let chain_controller = chain_service.start(Some("ChainService"));
    info_target!(crate::LOG_TARGET_MAIN, "ckb version: {}", version);
    info_target!(
        crate::LOG_TARGET_MAIN,
        "chain genesis hash: {:#x}",
        shared.genesis_hash()
    );

    let sync_shared = Arc::new(SyncShared::with_tmpdir(
        shared.clone(),
        args.config
            .network
            .sync
            .as_ref()
            .cloned()
            .unwrap_or_default(),
        args.config.tmp_dir.as_ref(),
    ));
    let network_state = Arc::new(
        NetworkState::from_config(args.config.network).expect("Init network state failed"),
    );
    let synchronizer = Synchronizer::new(chain_controller.clone(), Arc::clone(&sync_shared));

    let relayer = Relayer::new(
        chain_controller.clone(),
        Arc::clone(&sync_shared),
        args.config.tx_pool.min_fee_rate,
        args.config.tx_pool.max_tx_verify_cycles,
    );
    let net_timer = NetTimeProtocol::default();
    let alert_signature_config = args.config.alert_signature.unwrap_or_default();
    let alert_relayer = AlertRelayer::new(
        version.to_string(),
        shared.notify_controller().clone(),
        alert_signature_config,
    );

    let alert_notifier = Arc::clone(alert_relayer.notifier());
    let alert_verifier = Arc::clone(alert_relayer.verifier());

    let protocols = vec![
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Sync,
            Box::new(synchronizer.clone()),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Relay,
            Box::new(relayer),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Time,
            Box::new(net_timer),
            Arc::clone(&network_state),
        ),
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Alert,
            Box::new(alert_relayer),
            Arc::clone(&network_state),
        ),
    ];

    let required_protocol_ids = vec![SupportProtocols::Sync.protocol_id()];

    let network_controller = NetworkService::new(
        Arc::clone(&network_state),
        protocols,
        required_protocol_ids,
        shared.consensus().identify_name(),
        version.to_string(),
        exit_handler.clone(),
    )
    .start(Some("NetworkService"))
    .expect("Start network service failed");

    let builder = ServiceBuilder::new(&args.config.rpc)
        .enable_chain(shared.clone())
        .enable_pool(
            shared.clone(),
            Arc::clone(&sync_shared),
            args.config.tx_pool.min_fee_rate,
            args.config.rpc.reject_ill_transactions,
        )
        .enable_miner(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
            miner_enable,
        )
        .enable_net(network_controller.clone(), sync_shared)
        .enable_stats(shared.clone(), synchronizer, Arc::clone(&alert_notifier))
        .enable_experiment(shared.clone())
        .enable_integration_test(shared.clone(), network_controller.clone(), chain_controller)
        .enable_alert(alert_verifier, alert_notifier, network_controller)
        .enable_indexer(&args.config.indexer, shared.clone())
        .enable_debug();
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(args.config.rpc, io_handler, shared.notify_controller());

    let exit_handler_clone = exit_handler.clone();
    ctrlc::set_handler(move || {
        exit_handler_clone.notify_exit();
    })
    .expect("Error setting Ctrl-C handler");
    exit_handler.wait_for_exit();

    info_target!(crate::LOG_TARGET_MAIN, "Finishing work, please wait...");
    drop(rpc_server);

    Ok(())
}

fn verify_genesis(shared: &Shared) -> Result<(), ExitCode> {
    GenesisVerifier::new()
        .verify(shared.consensus())
        .map_err(|err| {
            eprintln!("genesis error: {}", err);
            ExitCode::Config
        })
}

fn sanitize_block_assembler_config(
    args: &RunArgs,
) -> Result<Option<BlockAssemblerConfig>, ExitCode> {
    let block_assembler_config = match (
        args.config.rpc.miner_enable(),
        args.config.block_assembler.clone(),
    ) {
        (true, Some(block_assembler)) => {
            let check_lock_code_hash = |code_hash| -> Result<bool, ExitCode> {
                let secp_cell_data =
                    Resource::bundled("specs/cells/secp256k1_blake160_sighash_all".to_string())
                        .get()
                        .map_err(|err| {
                            eprintln!(
                                "Load specs/cells/secp256k1_blake160_sighash_all error: {:?}",
                                err
                            );
                            ExitCode::Failure
                        })?;
                let genesis_cellbase = &args.consensus.genesis_block().transactions()[0];
                Ok(genesis_cellbase
                    .outputs()
                    .into_iter()
                    .zip(genesis_cellbase.outputs_data().into_iter())
                    .any(|(output, data)| {
                        data.raw_data() == secp_cell_data.as_ref()
                            && output
                                .type_()
                                .to_opt()
                                .map(|script| script.calc_script_hash())
                                .as_ref()
                                == Some(code_hash)
                    }))
            };
            if args.block_assembler_advanced
                || (block_assembler.hash_type == ScriptHashType::Type
                    && block_assembler.args.len() == SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN
                    && check_lock_code_hash(&block_assembler.code_hash.pack())?)
            {
                Some(block_assembler)
            } else {
                info_target!(
                    crate::LOG_TARGET_MAIN,
                    "Miner is disabled because block assmebler is not a recommended lock format. \
                     Edit ckb.toml or use `ckb run --ba-advanced` to use other lock scripts"
                );

                None
            }
        }

        _ => {
            info_target!(
                crate::LOG_TARGET_MAIN,
                "Miner is disabled, edit ckb.toml to enable it"
            );

            None
        }
    };
    Ok(block_assembler_config)
}
