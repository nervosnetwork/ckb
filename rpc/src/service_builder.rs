#![allow(deprecated)]
use crate::module::{
    AlertRpcImpl, ChainRpcImpl, DebugRpcImpl, ExperimentRpcImpl, IndexerRpcImpl,
    IntegrationTestRpcImpl, MinerRpcImpl, NetRpcImpl, PoolRpcImpl, RichIndexerRpcImpl,
    StatsRpcImpl, SubscriptionRpcImpl, add_alert_rpc_methods, add_chain_rpc_methods,
    add_debug_rpc_methods, add_experiment_rpc_methods, add_indexer_rpc_methods,
    add_integration_test_rpc_methods, add_miner_rpc_methods, add_net_rpc_methods,
    add_pool_rpc_methods, add_rich_indexer_rpc_methods, add_stats_rpc_methods,
    add_subscription_rpc_methods,
};
use crate::{IoHandler, RPCError};
use ckb_app_config::{DBConfig, IndexerConfig, RpcConfig};
use ckb_chain::ChainController;
use ckb_indexer::IndexerService;
use ckb_indexer_sync::{PoolService, new_secondary_db};
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_pow::Pow;
use ckb_rich_indexer::RichIndexerService;
use ckb_shared::shared::Shared;
use ckb_sync::SyncShared;
use ckb_types::packed::Script;
use ckb_util::Mutex;
use jsonrpc_core::{MetaIoHandler, RemoteProcedure};
use jsonrpc_utils::pub_sub::Session;
use std::sync::Arc;

const DEPRECATED_RPC_PREFIX: &str = "deprecated.";

#[doc(hidden)]
pub struct ServiceBuilder<'a> {
    config: &'a RpcConfig,
    io_handler: IoHandler,
}

macro_rules! set_rpc_module_methods {
    ($self:ident, $name:expr, $check:ident, $add_methods:ident, $methods:expr) => {{
        let mut meta_io = MetaIoHandler::default();
        $add_methods(&mut meta_io, $methods);
        if $self.config.$check() {
            $self.add_methods(meta_io);
        } else {
            $self.update_disabled_methods($name, meta_io);
        }
        $self
    }};
}

impl<'a> ServiceBuilder<'a> {
    /// Creates the RPC service builder from config.
    pub fn new(config: &'a RpcConfig) -> Self {
        Self {
            config,
            io_handler: IoHandler::with_compatibility(jsonrpc_core::Compatibility::V2),
        }
    }

    /// Mounts methods from module Chain if it is enabled in the config.
    pub fn enable_chain(mut self, shared: Shared) -> Self {
        let methods = ChainRpcImpl { shared };
        set_rpc_module_methods!(self, "Chain", chain_enable, add_chain_rpc_methods, methods)
    }

    /// Mounts methods from module Pool if it is enabled in the config.
    pub fn enable_pool(
        mut self,
        shared: Shared,
        extra_well_known_lock_scripts: Vec<Script>,
        extra_well_known_type_scripts: Vec<Script>,
    ) -> Self {
        let methods = PoolRpcImpl::new(
            shared,
            extra_well_known_lock_scripts,
            extra_well_known_type_scripts,
        );
        set_rpc_module_methods!(self, "Pool", pool_enable, add_pool_rpc_methods, methods)
    }

    /// Mounts methods from module Miner if `enable` is `true` and it is enabled in the config.
    pub fn enable_miner(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
        enable: bool,
    ) -> Self {
        let mut meta_io = MetaIoHandler::default();
        let methods = MinerRpcImpl {
            shared,
            chain,
            network_controller,
        };
        add_miner_rpc_methods(&mut meta_io, methods);
        if enable && self.config.miner_enable() {
            self.add_methods(meta_io);
        } else {
            self.update_disabled_methods("Miner", meta_io);
        }
        self
    }

    /// Mounts methods from module Net if it is enabled in the config.
    pub fn enable_net(
        mut self,
        network_controller: NetworkController,
        sync_shared: Arc<SyncShared>,
        chain_controller: Arc<ChainController>,
    ) -> Self {
        let methods = NetRpcImpl {
            network_controller,
            sync_shared,
            chain_controller,
        };
        set_rpc_module_methods!(self, "Net", net_enable, add_net_rpc_methods, methods)
    }

    /// Mounts methods from module Stats if it is enabled in the config.
    pub fn enable_stats(
        mut self,
        shared: Shared,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
    ) -> Self {
        let methods = StatsRpcImpl {
            shared,
            alert_notifier,
        };
        set_rpc_module_methods!(self, "Stats", stats_enable, add_stats_rpc_methods, methods)
    }

    /// Mounts methods from module Experiment if it is enabled in the config.
    pub fn enable_experiment(mut self, shared: Shared) -> Self {
        let methods = ExperimentRpcImpl { shared };
        set_rpc_module_methods!(
            self,
            "Experiment",
            experiment_enable,
            add_experiment_rpc_methods,
            methods
        )
    }

    /// Mounts methods from module Integration if it is enabled in the config.
    pub fn enable_integration_test(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
        well_known_lock_scripts: Vec<Script>,
        well_known_type_scripts: Vec<Script>,
    ) -> Self {
        if self.config.integration_test_enable() {
            // IntegrationTest only on Dummy PoW chain
            assert_eq!(
                shared.consensus().pow,
                Pow::Dummy,
                "Only run integration test on Dummy PoW chain"
            );
        }
        let methods = IntegrationTestRpcImpl {
            shared,
            network_controller,
            chain,
            well_known_lock_scripts,
            well_known_type_scripts,
        };
        set_rpc_module_methods!(
            self,
            "IntegrationTest",
            integration_test_enable,
            add_integration_test_rpc_methods,
            methods
        )
    }

    /// Mounts methods from module Alert if it is enabled in the config.
    pub fn enable_alert(
        mut self,
        alert_verifier: Arc<AlertVerifier>,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
        network_controller: NetworkController,
        shared: Shared,
    ) -> Self {
        let methods = AlertRpcImpl::new(
            alert_verifier,
            alert_notifier,
            network_controller,
            shared.async_handle().clone(),
        );
        set_rpc_module_methods!(self, "Alert", alert_enable, add_alert_rpc_methods, methods)
    }

    /// Mounts methods from module Debug if it is enabled in the config.
    pub fn enable_debug(mut self) -> Self {
        let methods = DebugRpcImpl {};
        set_rpc_module_methods!(self, "Debug", debug_enable, add_debug_rpc_methods, methods)
    }

    /// Mounts methods from module Indexer if it is enabled in the config.
    pub fn enable_indexer(
        mut self,
        shared: Shared,
        db_config: &DBConfig,
        indexer_config: &IndexerConfig,
    ) -> Self {
        // Initialize instances of data sources that will be shared for use by indexer and rich-indexer.
        let ckb_secondary_db = new_secondary_db(db_config, &indexer_config.into());
        let pool_service =
            PoolService::new(indexer_config.index_tx_pool, shared.async_handle().clone());

        if self.config.indexer_enable() {
            // Init indexer service.
            let mut indexer = IndexerService::new(
                ckb_secondary_db.clone(),
                pool_service.clone(),
                indexer_config,
                shared.async_handle().clone(),
            );
            indexer.spawn_poll(shared.notify_controller().clone());
            if indexer_config.index_tx_pool {
                indexer.index_tx_pool(shared.notify_controller().clone());
            }

            let indexer_handle = indexer.handle();
            let methods = IndexerRpcImpl::new(indexer_handle);
            self = set_rpc_module_methods!(
                self,
                "Indexer",
                indexer_enable,
                add_indexer_rpc_methods,
                methods
            );
        }

        if self.config.rich_indexer_enable() {
            // Init rich-indexer service
            let mut rich_indexer = RichIndexerService::new(
                ckb_secondary_db,
                pool_service,
                indexer_config,
                shared.async_handle().clone(),
            );
            rich_indexer.spawn_poll(shared.notify_controller().clone());
            if indexer_config.index_tx_pool {
                rich_indexer.index_tx_pool(shared.notify_controller().clone());
            }

            let rich_indexer_handle = rich_indexer.async_handle();
            let rich_indexer_methods = RichIndexerRpcImpl::new(rich_indexer_handle);
            self = set_rpc_module_methods!(
                self,
                "RichIndexer",
                rich_indexer_enable,
                add_rich_indexer_rpc_methods,
                rich_indexer_methods
            )
        }
        self
    }

    pub fn enable_subscription(&mut self, shared: Shared) {
        if self.config.subscription_enable() {
            let methods = SubscriptionRpcImpl::new(
                shared.notify_controller().clone(),
                shared.async_handle().clone(),
            );
            let mut meta_io = MetaIoHandler::default();
            add_subscription_rpc_methods(&mut meta_io, methods);
            self.add_methods(meta_io);
        }
    }

    fn add_methods<I>(&mut self, rpc_methods: I)
    where
        I: IntoIterator<Item = (String, RemoteProcedure<Option<Session>>)>,
    {
        let enable_deprecated_rpc = self.config.enable_deprecated_rpc;
        self.io_handler
            .extend_with(rpc_methods.into_iter().map(|(name, method)| {
                if let Some(striped_method_name) = name.strip_prefix(DEPRECATED_RPC_PREFIX) {
                    (
                        striped_method_name.to_owned(),
                        if enable_deprecated_rpc {
                            method
                        } else {
                            RemoteProcedure::Method(Arc::new(|_param, _meta| async {
                                Err(RPCError::rpc_method_is_deprecated())
                            }))
                        },
                    )
                } else {
                    (name, method)
                }
            }));
    }

    fn update_disabled_methods<I, M>(&mut self, module: &str, rpc_methods: I)
    where
        I: IntoIterator<Item = (String, M)>,
    {
        rpc_methods.into_iter().for_each(|(name, _method)| {
            let error = Err(RPCError::rpc_module_is_disabled(module));
            self.io_handler.add_sync_method(
                name.split(DEPRECATED_RPC_PREFIX)
                    .collect::<Vec<&str>>()
                    .last()
                    .unwrap(),
                move |_param| error.clone(),
            )
        });
    }

    /// Builds the RPC methods handler used in the RPC server.
    pub fn build(self) -> IoHandler {
        let mut io_handler = self.io_handler;
        io_handler.add_method("ping", |_| async { Ok("pong".into()) });
        io_handler
    }
}
