use crate::mock_sync::mock_synchronizer::MockSynchronizer;
use crate::modify_config::write_network_config;
use crate::run::ckb_init;
use ckb_app_config::{AppConfig, ExitCode, RunArgs, Setup, SupportProtocol};
use ckb_async_runtime::Handle;
use ckb_build_info::Version;
use ckb_chain::chain::{ChainController, ProcessBlockRequest, TruncateRequest};
use ckb_channel::Receiver;
use ckb_launcher::{Launcher, SharedPackage};
use ckb_logger::info;
use ckb_network::{
    CKBProtocol, DefaultExitHandler, Flags, NetworkController, NetworkService, NetworkState,
    SupportProtocols,
};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_rpc::{RpcServer, ServiceBuilder};
use ckb_shared::Shared;
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_sync::{NetTimeProtocol, SyncShared};
use ckb_tx_pool::service::TxVerificationResult;
use ckb_types::{core::cell::setup_system_cell_cache, u256, U256};
use std::path::PathBuf;
use std::sync::Arc;

pub struct MockNode {
    pub binary_path: PathBuf,
    pub rpc_port: u64,
    pub p2p_port: u64,
    pub work_dir: PathBuf,
    pub shared_db_path: PathBuf,
    pub handle: Handle,
    pub bootnode: Option<String>,
    pub shared: Arc<once_cell::sync::OnceCell<(Shared, SharedPackage)>>,
    pub exit_handler: DefaultExitHandler,
}

impl MockNode {
    pub fn start(&self) {
        ckb_init(
            self.binary_path.clone(),
            self.work_dir.clone(),
            self.rpc_port,
            self.p2p_port,
        );

        {
            let filepath = self.work_dir.join("ckb.toml");
            let mut bootnodes = toml_edit::Array::new();
            if self.bootnode.is_some() {
                bootnodes.push(self.bootnode.clone().unwrap());
            }
            write_network_config(filepath.clone(), bootnodes.clone());
        }

        if !self.work_dir.exists() {
            let cur_dir = std::env::current_dir().unwrap();
            panic!(
                "current dir:{:?}, {:?} not exist",
                cur_dir,
                self.work_dir.clone()
            );
        }

        self.run_app(Version {
            major: u8::MAX,
            minor: u8::MAX,
            patch: u16::MAX,
            dash_pre: "".to_string(),
            code_name: None,
            commit_describe: None,
            commit_date: None,
        })
        .unwrap();
    }

    fn run_app(&self, version: Version) -> Result<(), ExitCode> {
        // Always print backtrace on panic.
        ::std::env::set_var("RUST_BACKTRACE", "full");
        let mut config = AppConfig::load_for_subcommand(&self.work_dir, "run")?;
        let subcommand_name = "mock_node_bin";
        config.set_bin_name(subcommand_name.to_string());
        let setup = Setup {
            subcommand_name: subcommand_name.to_string(),
            config,
            is_sentry_enabled: false,
        };

        let consensus = setup.consensus()?;
        let chain_spec_hash = setup.chain_spec()?.hash;
        let mut config = setup.config.into_ckb()?;
        config.network.sync.min_chain_work = u256!("0x0");
        config.network.sync.assume_valid_target = None;

        let run_args = RunArgs {
            config,
            consensus,
            block_assembler_advanced: false,
            skip_chain_spec_check: false,
            overwrite_chain_spec: false,
            chain_spec_hash,
            indexer: false,
        };

        let ret = self.run(run_args, version, self.handle.clone());

        ret
    }

    fn run(&self, args: RunArgs, version: Version, async_handle: Handle) -> Result<(), ExitCode> {
        let mut launcher = Launcher::new(args, version, async_handle);
        launcher.args.config.db.path = self.shared_db_path.clone();

        let miner_enable = false;

        let (shared, _pack) = self.shared.get_or_init(|| {
            let block_assembler_config = launcher
                .sanitize_block_assembler_config()
                .unwrap_or_else(|_e| panic!("sanitize block assembler config failed"));
            let (shared, pack) = launcher
                .build_shared(block_assembler_config)
                .unwrap_or_else(|e| panic!("build shared failed {:?}", e));
            setup_system_cell_cache(
                shared.consensus().genesis_block(),
                shared.snapshot().as_ref(),
            )
            .expect("SYSTEM_CELL cache init once");
            (shared, pack)
        });

        launcher.check_assume_valid_target(&shared);

        let (fake_process_block_sender, _recv) = ckb_channel::bounded::<ProcessBlockRequest>(1000);
        let (fake_truncate_sender, _recv) = ckb_channel::bounded::<TruncateRequest>(1000);
        let (fake_signal_sender, _recv) = ckb_channel::bounded::<()>(1000);
        let fake_stop_handler = StopHandler::new(
            SignalSender::Crossbeam(fake_signal_sender),
            None,
            "fake_chain".to_string(),
        );
        let fake_chain_controller = ChainController::new(
            fake_process_block_sender,
            fake_truncate_sender,
            fake_stop_handler,
        );

        let (_sender, fake_relay_tx_receiver) = ckb_channel::bounded::<TxVerificationResult>(1000);
        let (network_controller, rpc_server) = start_network_and_rpc(
            &launcher,
            &shared,
            fake_chain_controller.non_owning_clone(),
            &self.exit_handler.clone(),
            miner_enable,
            fake_relay_tx_receiver,
        );

        self.exit_handler.wait_for_exit();

        info!("Finishing work, please wait...");

        drop(rpc_server);
        drop(network_controller);
        drop(fake_chain_controller);
        Ok(())
    }
}

fn start_network_and_rpc(
    lc: &Launcher,
    shared: &Shared,
    chain_controller: ChainController,
    exit_handler: &DefaultExitHandler,
    miner_enable: bool,
    relay_tx_receiver: Receiver<TxVerificationResult>,
) -> (NetworkController, RpcServer) {
    let sync_shared = Arc::new(SyncShared::with_tmpdir(
        shared.clone(),
        lc.args.config.network.sync.clone(),
        lc.args.config.tmp_dir.as_ref(),
        relay_tx_receiver,
    ));
    let network_state = Arc::new(
        NetworkState::from_config(lc.args.config.network.clone())
            .expect("Init network state failed"),
    );

    // Sync is a core protocol, user cannot disable it via config
    let synchronizer = MockSynchronizer::new(Arc::clone(&sync_shared));
    let mut protocols = vec![CKBProtocol::new_with_support_protocol(
        SupportProtocols::Sync,
        Box::new(synchronizer),
        Arc::clone(&network_state),
    )];

    let support_protocols = &lc.args.config.network.support_protocols;
    let mut flags = Flags::all();

    if support_protocols.contains(&SupportProtocol::Time) {
        let net_timer = NetTimeProtocol::default();
        protocols.push(CKBProtocol::new_with_support_protocol(
            SupportProtocols::Time,
            Box::new(net_timer),
            Arc::clone(&network_state),
        ));
    }

    flags.remove(Flags::RELAY);
    flags.remove(Flags::BLOCK_FILTER);
    flags.remove(Flags::LIGHT_CLIENT);

    let alert_signature_config = lc.args.config.alert_signature.clone().unwrap_or_default();
    let alert_relayer = AlertRelayer::new(
        lc.version.to_string(),
        shared.notify_controller().clone(),
        alert_signature_config,
    );

    let alert_notifier = Arc::clone(alert_relayer.notifier());
    let alert_verifier = Arc::clone(alert_relayer.verifier());
    if support_protocols.contains(&SupportProtocol::Alert) {
        protocols.push(CKBProtocol::new_with_support_protocol(
            SupportProtocols::Alert,
            Box::new(alert_relayer),
            Arc::clone(&network_state),
        ));
    }

    let required_protocol_ids = vec![SupportProtocols::Sync.protocol_id()];

    let network_controller = NetworkService::new(
        Arc::clone(&network_state),
        protocols,
        required_protocol_ids,
        (
            shared.consensus().identify_name(),
            lc.version.to_string(),
            flags,
        ),
        exit_handler.clone(),
    )
    .start(shared.async_handle())
    .expect("Start network service failed");

    let rpc_config = lc.args.config.rpc.clone();
    let builder = ServiceBuilder::new(&rpc_config)
        .enable_chain(shared.clone())
        .enable_pool(
            shared.clone(),
            lc.args.config.tx_pool.min_fee_rate,
            rpc_config
                .extra_well_known_lock_scripts
                .iter()
                .map(|script| script.clone().into())
                .collect(),
            rpc_config
                .extra_well_known_type_scripts
                .iter()
                .map(|script| script.clone().into())
                .collect(),
        )
        .enable_miner(
            shared.clone(),
            network_controller.clone(),
            chain_controller.clone(),
            miner_enable,
        )
        .enable_net(network_controller.clone(), sync_shared)
        .enable_stats(shared.clone(), Arc::clone(&alert_notifier))
        .enable_experiment(shared.clone())
        .enable_integration_test(shared.clone(), network_controller.clone(), chain_controller)
        .enable_alert(alert_verifier, alert_notifier, network_controller.clone())
        // .enable_indexer(shared.clone(), &lc.args.config.db, &lc.args.config.indexer)
        .enable_debug();
    let io_handler = builder.build();

    let rpc_server = RpcServer::new(
        rpc_config.clone(),
        io_handler,
        shared.notify_controller(),
        lc.async_handle.clone().into_inner(),
    );

    (network_controller, rpc_server)
}
