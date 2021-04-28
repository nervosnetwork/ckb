//! CKB launcher.
//!
//! ckb launcher is helps to launch ckb node.

// declare here for mute ./devtools/ci/check-cargotoml.sh error
extern crate num_cpus;

mod migrations;

use ckb_app_config::{BlockAssemblerConfig, ExitCode, RunArgs};
use ckb_async_runtime::Handle;
use ckb_build_info::Version;
use ckb_chain::chain::{ChainController, ChainService};
use ckb_channel::Receiver;
use ckb_db::{ReadOnlyDB, RocksDB};
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_db_schema::COLUMNS;
use ckb_error::Error;
use ckb_jsonrpc_types::ScriptHashType;
use ckb_logger::info;
use ckb_network::{
    CKBProtocol, DefaultExitHandler, NetworkController, NetworkService, NetworkState, PeerIndex,
    SupportProtocols,
};
use ckb_network_alert::alert_relayer::AlertRelayer;
use ckb_proposal_table::ProposalTable;
use ckb_resource::Resource;
use ckb_rpc::{RpcServer, ServiceBuilder};
use ckb_shared::{Shared, SharedBuilder, SharedPackage};
use ckb_store::{ChainDB, ChainStore};
use ckb_sync::{NetTimeProtocol, Relayer, SyncShared, Synchronizer};
use ckb_types::{packed::Byte32, prelude::*};
use ckb_verification::GenesisVerifier;
use ckb_verification_traits::Verifier;
use std::path::PathBuf;
use std::sync::Arc;

const INIT_DB_VERSION: &str = "20191127135521";
const SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN: usize = 20;

/// Wrapper contains migration and db
pub struct DatabaseMigration {
    migrations: Migrations,
    path: PathBuf,
}

impl DatabaseMigration {
    /// Open db with bulk loading parameters, init migration
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        let mut migrations = Migrations::default();
        migrations.add_migration(Box::new(DefaultMigration::new(INIT_DB_VERSION)));
        migrations.add_migration(Box::new(migrations::ChangeMoleculeTableToStruct));
        migrations.add_migration(Box::new(migrations::CellMigration));
        migrations.add_migration(Box::new(migrations::AddNumberHashMapping));

        DatabaseMigration {
            migrations,
            path: path.into(),
        }
    }

    /// Return true if migration is required
    pub fn migration_check(&self) -> bool {
        ReadOnlyDB::open(&self.path)
            .unwrap_or_else(|err| panic!("{}", err))
            .map(|db| self.migrations.check(&db))
            .unwrap_or(false)
    }

    /// Check whether database requires expensive migrations.
    pub fn require_expensive_migrations(&self) -> bool {
        ReadOnlyDB::open(&self.path)
            .unwrap_or_else(|err| panic!("{}", err))
            .map(|db| self.migrations.expensive(&db))
            .unwrap_or(false)
    }

    /// Perform migrate.
    pub fn migrate(self) -> Result<(), Error> {
        if let Some(db) = RocksDB::prepare_for_bulk_load_open(&self.path, COLUMNS)? {
            self.migrations.migrate(db)?;
        }
        Ok(())
    }
}

/// Ckb launcher is helps to launch ckb node.
pub struct Launcher {
    /// cli `run` subcommand parsed args
    pub args: RunArgs,
    /// ckb node version
    pub version: Version,
    /// ckb global runtime handle
    pub async_handle: Handle,
}

impl Launcher {
    /// Construct new Launcher from cli args
    pub fn new(args: RunArgs, version: Version, async_handle: Handle) -> Self {
        Launcher {
            args,
            version,
            async_handle,
        }
    }

    /// Sanitize block assembler config
    pub fn sanitize_block_assembler_config(
        &self,
    ) -> Result<Option<BlockAssemblerConfig>, ExitCode> {
        let block_assembler_config = match (
            self.args.config.rpc.miner_enable(),
            self.args.config.block_assembler.clone(),
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
                    let genesis_cellbase = &self.args.consensus.genesis_block().transactions()[0];
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
                if self.args.block_assembler_advanced
                    || (block_assembler.hash_type == ScriptHashType::Type
                        && block_assembler.args.len() == SECP256K1_BLAKE160_SIGHASH_ALL_ARG_LEN
                        && check_lock_code_hash(&block_assembler.code_hash.pack())?)
                {
                    Some(block_assembler)
                } else {
                    info!(
                        "Miner is disabled because block assmebler is not a recommended lock format. \
                         Edit ckb.toml or use `ckb run --ba-advanced` to use other lock scripts"
                    );

                    None
                }
            }

            _ => {
                info!("Miner is disabled, edit ckb.toml to enable it");

                None
            }
        };
        Ok(block_assembler_config)
    }

    /// Migrate prompt
    pub fn migrate_guard(&self) -> Result<(), ExitCode> {
        let migration = DatabaseMigration::new(&self.args.config.db.path);
        if migration.require_expensive_migrations() {
            eprintln!(
                "For optimal performance, CKB wants to migrate the data into new format.\n\
                You can use the old version CKB if you don't want to do the migration.\n\
                We strongly recommended you to use the latest stable version of CKB, \
                since the old versions may have unfixed vulnerabilities.\n\
                Run `ckb migrate --help` for more information about migration."
            );
            return Err(ExitCode::Failure);
        }
        Ok(())
    }

    fn write_chain_spec_hash(&self, store: &ChainDB) -> Result<(), ExitCode> {
        store
            .put_chain_spec_hash(&self.args.chain_spec_hash)
            .map_err(|err| {
                eprintln!(
                    "store.put_chain_spec_hash {} error: {}",
                    self.args.chain_spec_hash, err
                );
                ExitCode::IO
            })
    }

    fn check_spec(&self, shared: &Shared) -> Result<(), ExitCode> {
        let store = shared.store();
        let stored_spec_hash = store.get_chain_spec_hash();

        if stored_spec_hash.is_none() {
            // fresh yet
            self.write_chain_spec_hash(store)?;
            info!("Touch chain spec hash: {}", self.args.chain_spec_hash);
        } else if stored_spec_hash.as_ref() == Some(&self.args.chain_spec_hash) {
            // stored == configured
            // do nothing
        } else if self.args.overwrite_chain_spec {
            // stored != configured with --overwrite-spec
            self.write_chain_spec_hash(store)?;
            info!(
                "Overwrite chain spec hash from {} to {}",
                stored_spec_hash.expect("checked"),
                self.args.overwrite_chain_spec,
            );
        } else if self.args.skip_chain_spec_check {
            // stored != configured with --skip-spec-check
            // do nothing
        } else {
            // stored != configured
            eprintln!(
                "chain_spec_hash mismatch Config({}) storage({}), pass command line argument \
                    --skip-spec-check if you are sure that the two different chains are compatible; \
                    or pass --overwrite-spec to force overriding stored chain spec with configured chain spec",
                self.args.chain_spec_hash, stored_spec_hash.expect("checked")
            );
            return Err(ExitCode::Config);
        }
        Ok(())
    }

    fn verify_genesis(&self, shared: &Shared) -> Result<(), ExitCode> {
        GenesisVerifier::new()
            .verify(shared.consensus())
            .map_err(|err| {
                eprintln!("genesis error: {}", err);
                ExitCode::Config
            })
    }

    /// Build shared
    pub fn build_shared(
        &self,
        block_assembler_config: Option<BlockAssemblerConfig>,
    ) -> Result<(Shared, SharedPackage), ExitCode> {
        let (shared, pack) = SharedBuilder::new(
            &self.args.config.db,
            Some(self.args.config.ancient.clone()),
            self.async_handle.clone(),
        )
        .consensus(self.args.consensus.clone())
        .tx_pool_config(self.args.config.tx_pool.clone())
        .notify_config(self.args.config.notify.clone())
        .store_config(self.args.config.store)
        .block_assembler_config(block_assembler_config)
        .build()
        .map_err(|err| {
            eprintln!("Build shared error: {:?}", err);
            ExitCode::Failure
        })?;

        // Verify genesis every time starting node
        self.verify_genesis(&shared)?;
        self.check_spec(&shared)?;

        Ok((shared, pack))
    }

    /// Check whether the data already exists in the database before starting
    pub fn check_assume_valid_target(&mut self, shared: &Shared) {
        if let Some(ref target) = self.args.config.network.sync.assume_valid_target {
            if shared.snapshot().block_exists(&target.pack()) {
                self.args.config.network.sync.assume_valid_target.take();
            }
        }
    }

    /// Start chain service, return ChainController
    pub fn start_chain_service(&self, shared: &Shared, table: ProposalTable) -> ChainController {
        let chain_service = ChainService::new(shared.clone(), table);
        let chain_controller = chain_service.start(Some("ChainService"));
        info!("chain genesis hash: {:#x}", shared.genesis_hash());
        chain_controller
    }

    /// Start network service and rpc servre
    pub fn start_network_and_rpc(
        &self,
        shared: &Shared,
        chain_controller: ChainController,
        exit_handler: &DefaultExitHandler,
        miner_enable: bool,
        relay_tx_receiver: Receiver<(PeerIndex, Byte32)>,
    ) -> (NetworkController, RpcServer) {
        let sync_shared = Arc::new(SyncShared::with_tmpdir(
            shared.clone(),
            self.args.config.network.sync.clone(),
            self.args.config.tmp_dir.as_ref(),
            relay_tx_receiver,
        ));
        let network_state = Arc::new(
            NetworkState::from_config(self.args.config.network.clone())
                .expect("Init network state failed"),
        );
        let synchronizer = Synchronizer::new(chain_controller.clone(), Arc::clone(&sync_shared));

        let relayer = Relayer::new(
            chain_controller.clone(),
            Arc::clone(&sync_shared),
            self.args.config.tx_pool.min_fee_rate,
            self.args.config.tx_pool.max_tx_verify_cycles,
        );
        let net_timer = NetTimeProtocol::default();
        let alert_signature_config = self.args.config.alert_signature.clone().unwrap_or_default();
        let alert_relayer = AlertRelayer::new(
            self.version.to_string(),
            shared.notify_controller().clone(),
            alert_signature_config,
        );

        let alert_notifier = Arc::clone(alert_relayer.notifier());
        let alert_verifier = Arc::clone(alert_relayer.verifier());

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
            self.version.to_string(),
            exit_handler.clone(),
        )
        .start(shared.async_handle())
        .expect("Start network service failed");

        let builder = ServiceBuilder::new(&self.args.config.rpc)
            .enable_chain(shared.clone())
            .enable_pool(
                shared.clone(),
                self.args.config.tx_pool.min_fee_rate,
                self.args.config.rpc.reject_ill_transactions,
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
            .enable_debug();
        let io_handler = builder.build();

        let rpc_server = RpcServer::new(
            self.args.config.rpc.clone(),
            io_handler,
            shared.notify_controller(),
        );

        (network_controller, rpc_server)
    }
}
