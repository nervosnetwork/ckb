use crate::helper::deadlock_detection;
use ckb_app_config::{exit_failure, BlockAssemblerConfig, ExitCode, RunArgs};
use ckb_async_runtime::Handle;
use ckb_build_info::Version;
use ckb_chain::chain::ChainService;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_logger::info_target;
use ckb_network::{
    CKBProtocol, DefaultExitHandler, ExitHandler, NetworkService, NetworkState, SupportProtocols,
};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_resource::Resource;
use ckb_rpc::RpcServerController;
use ckb_shared::shared::{Shared, SharedBuilder};
use ckb_store::{ChainDB, ChainStore};
use ckb_sync::{NetTimeProtocol, Relayer, SyncShared, Synchronizer};
use ckb_types::packed::Byte32;
use ckb_types::{core::cell::setup_system_cell_cache, prelude::*};
use ckb_verification::GenesisVerifier;
use ckb_verification_traits::Verifier;
use std::sync::Arc;

const SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN: usize = 20;

pub fn run(mut args: RunArgs, version: Version, async_handle: Handle) -> Result<(), ExitCode> {
    deadlock_detection();

    info_target!(crate::LOG_TARGET_MAIN, "ckb version: {}", version);

    let block_assembler_config = sanitize_block_assembler_config(&args)?;
    let miner_enable = block_assembler_config.is_some();
    let exit_handler = DefaultExitHandler::default();

    let mut rpc_controller = RpcServerController::new(
        &args.config.rpc,
        args.config.tx_pool.min_fee_rate,
        miner_enable,
    );
    rpc_controller.start_server();

    let (shared, table) = {
        let shared_builder = SharedBuilder::new(
            &args.config.db,
            Some(args.config.ancient.clone()),
            async_handle,
        );

        if shared_builder.require_expensive_migrations() {
            eprintln!(
                "For optimal performance, CKB wants to migrate the data into new format.\n\
                You can use the old version CKB if you don't want to do the migration.\n\
                We strongly recommended you to use the latest stable version of CKB, \
                since the old versions may have unfixed vulnerabilities.\n\
                Run `ckb migrate --help` for more information about migration."
            );
            return Err(ExitCode::Failure);
        }

        shared_builder
            .consensus(args.consensus.clone())
            .tx_pool_config(args.config.tx_pool)
            .notify_config(args.config.notify.clone())
            .store_config(args.config.store)
            .block_assembler_config(block_assembler_config)
            .build()
            .map_err(|err| {
                eprintln!("Run error: {:?}", err);
                ExitCode::Failure
            })?
    };
    rpc_controller.update_shared(Some(shared.clone()));

    // Verify genesis every time starting node
    verify_genesis(&shared)?;
    check_spec(&shared, &args)?;

    // spawn freezer background process
    let _freezer = shared.spawn_freeze();

    setup_system_cell_cache(
        shared.consensus().genesis_block(),
        &shared.store().cell_provider(),
    );

    rayon::ThreadPoolBuilder::new()
        .thread_name(|i| format!("RayonGlobal-{}", i))
        .build_global()
        .map_err(|_| exit_failure!("Init the global thread pool for rayon failed"))?;

    ckb_memory_tracker::track_current_process(
        args.config.memory_tracker.interval,
        Some(shared.store().db().inner()),
    );

    // Check whether the data already exists in the database before starting
    if let Some(ref target) = args.config.network.sync.assume_valid_target {
        if shared.snapshot().block_exists(&target.pack()) {
            args.config.network.sync.assume_valid_target.take();
        }
    }

    let chain_service = ChainService::new(shared.clone(), table);
    let chain_controller = chain_service.start(Some("ChainService"));
    info_target!(
        crate::LOG_TARGET_MAIN,
        "chain genesis hash: {:#x}",
        shared.genesis_hash()
    );
    rpc_controller.update_chain_controller(Some(chain_controller.clone()));

    let sync_shared = Arc::new(SyncShared::with_tmpdir(
        shared.clone(),
        args.config.network.sync.clone(),
        args.config.tmp_dir.as_ref(),
    ));
    rpc_controller.update_sync_shared(Some(Arc::clone(&sync_shared)));

    let network_state = Arc::new(
        NetworkState::from_config(args.config.network)
            .map_err(|_| exit_failure!("Init network state failed"))?,
    );
    let synchronizer = Synchronizer::new(chain_controller.clone(), Arc::clone(&sync_shared));
    rpc_controller.update_synchronizer(Some(synchronizer.clone()));

    let relayer = Relayer::new(
        chain_controller,
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
    rpc_controller.update_alert_notifier(Some(alert_notifier));
    rpc_controller.update_alert_verifier(Some(alert_verifier));

    let protocols = vec![
        CKBProtocol::new_with_support_protocol(
            SupportProtocols::Sync,
            Box::new(synchronizer),
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
    .start(shared.async_handle())
    .map_err(|_| exit_failure!("Start network service failed"))?;

    rpc_controller.update_network_controller(Some(network_controller));

    let exit_handler_clone = exit_handler.clone();
    ctrlc::set_handler(move || {
        exit_handler_clone.notify_exit();
    })
    .map_err(|_| exit_failure!("Error setting Ctrl-C handler"))?;
    exit_handler.wait_for_exit();

    info_target!(crate::LOG_TARGET_MAIN, "Finishing work, please wait...");
    rpc_controller.stop_server();

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

fn check_spec(shared: &Shared, args: &RunArgs) -> Result<(), ExitCode> {
    let store = shared.store();
    let stored_spec_hash = store.get_chain_spec_hash();

    if stored_spec_hash.is_none() {
        // fresh yet
        write_chain_spec_hash(store, &args.chain_spec_hash)?;
        info_target!(
            crate::LOG_TARGET_MAIN,
            "Touch chain spec hash: {}",
            args.chain_spec_hash
        );
    } else if stored_spec_hash.as_ref() == Some(&args.chain_spec_hash) {
        // stored == configured
        // do nothing
    } else if args.overwrite_chain_spec {
        // stored != configured with --overwrite-spec
        write_chain_spec_hash(store, &args.chain_spec_hash)?;
        info_target!(
            crate::LOG_TARGET_MAIN,
            "Overwrite chain spec hash from {} to {}",
            stored_spec_hash.expect("checked"),
            args.overwrite_chain_spec,
        );
    } else if args.skip_chain_spec_check {
        // stored != configured with --skip-spec-check
        // do nothing
    } else {
        // stored != configured
        eprintln!(
            "chain_spec_hash mismatch Config({}) storage({}), pass command line argument \
                --skip-spec-check if you are sure that the two different chains are compatible; \
                or pass --overwrite-spec to force overriding stored chain spec with configured chain spec",
            args.chain_spec_hash, stored_spec_hash.expect("checked")
        );
        return Err(ExitCode::Config);
    }
    Ok(())
}

fn write_chain_spec_hash(store: &ChainDB, chain_spec_hash: &Byte32) -> Result<(), ExitCode> {
    store.put_chain_spec_hash(chain_spec_hash).map_err(|err| {
        eprintln!(
            "store.put_chain_spec_hash {} error: {}",
            chain_spec_hash, err
        );
        ExitCode::IO
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
