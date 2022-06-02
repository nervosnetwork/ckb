#![allow(deprecated)]
use crate::error::RPCError;
use crate::module::SubscriptionSession;
use crate::module::{
    AlertRpc, AlertRpcImpl, ChainRpc, ChainRpcImpl, DebugRpc, DebugRpcImpl, ExperimentRpc,
    ExperimentRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl, NetRpc,
    NetRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use crate::IoHandler;
use ckb_app_config::RpcConfig;
use ckb_chain::chain::ChainController;
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_pow::Pow;
use ckb_shared::shared::Shared;
use ckb_sync::SyncShared;
use ckb_types::{core::FeeRate, packed::Script};
use ckb_util::Mutex;
use jsonrpc_core::RemoteProcedure;
use std::sync::Arc;

const DEPRECATED_RPC_PREFIX: &str = "deprecated.";

#[doc(hidden)]
pub struct ServiceBuilder<'a> {
    config: &'a RpcConfig,
    io_handler: IoHandler,
}

impl<'a> ServiceBuilder<'a> {
    /// Creates the RPC service builder from config.
    pub fn new(config: &'a RpcConfig) -> Self {
        Self {
            config,
            io_handler: IoHandler::default(),
        }
    }

    /// Mounts methods from module Chain if it is enabled in the config.
    pub fn enable_chain(mut self, shared: Shared) -> Self {
        let rpc_methods = ChainRpcImpl { shared }.to_delegate();
        if self.config.chain_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Chain", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Pool if it is enabled in the config.
    pub fn enable_pool(
        mut self,
        shared: Shared,
        min_fee_rate: FeeRate,
        extra_well_known_lock_scripts: Vec<Script>,
        extra_well_known_type_scripts: Vec<Script>,
    ) -> Self {
        let rpc_methods = PoolRpcImpl::new(
            shared,
            min_fee_rate,
            extra_well_known_lock_scripts,
            extra_well_known_type_scripts,
        )
        .to_delegate();
        if self.config.pool_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Pool", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Miner if `enable` is `true` and it is enabled in the config.
    pub fn enable_miner(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
        enable: bool,
    ) -> Self {
        let rpc_methods = MinerRpcImpl {
            shared,
            chain,
            network_controller,
        }
        .to_delegate();
        if enable && self.config.miner_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Miner", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Net if it is enabled in the config.
    pub fn enable_net(
        mut self,
        network_controller: NetworkController,
        sync_shared: Arc<SyncShared>,
    ) -> Self {
        let rpc_methods = NetRpcImpl {
            network_controller,
            sync_shared,
        }
        .to_delegate();
        if self.config.net_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Net", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Stats if it is enabled in the config.
    pub fn enable_stats(
        mut self,
        shared: Shared,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
    ) -> Self {
        let rpc_methods = StatsRpcImpl {
            shared,
            alert_notifier,
        }
        .to_delegate();
        if self.config.stats_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Stats", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Experiment if it is enabled in the config.
    pub fn enable_experiment(mut self, shared: Shared) -> Self {
        let rpc_methods = ExperimentRpcImpl { shared }.to_delegate();
        if self.config.experiment_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Experiment", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Integration if it is enabled in the config.
    pub fn enable_integration_test(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
    ) -> Self {
        let rpc_methods = IntegrationTestRpcImpl {
            shared: shared.clone(),
            network_controller,
            chain,
        }
        .to_delegate();

        if self.config.integration_test_enable() {
            // IntegrationTest only on Dummy PoW chain
            assert_eq!(
                shared.consensus().pow,
                Pow::Dummy,
                "Only run integration test on Dummy PoW chain"
            );

            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("IntegrationTest", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Alert if it is enabled in the config.
    pub fn enable_alert(
        mut self,
        alert_verifier: Arc<AlertVerifier>,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
        network_controller: NetworkController,
    ) -> Self {
        let rpc_methods =
            AlertRpcImpl::new(alert_verifier, alert_notifier, network_controller).to_delegate();
        if self.config.alert_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Alert", rpc_methods);
        }
        self
    }

    /// Mounts methods from module Debug if it is enabled in the config.
    pub fn enable_debug(mut self) -> Self {
        if self.config.debug_enable() {
            self.io_handler.extend_with(DebugRpcImpl {}.to_delegate());
        }
        self
    }

    fn update_disabled_methods<I, M>(&mut self, module: &str, rpc_methods: I)
    where
        I: IntoIterator<Item = (String, M)>,
    {
        rpc_methods.into_iter().for_each(|(name, _method)| {
            let error = Err(RPCError::rpc_module_is_disabled(module));
            self.io_handler.add_sync_method(
                name.split("deprecated.")
                    .collect::<Vec<&str>>()
                    .last()
                    .unwrap(),
                move |_param| error.clone(),
            )
        });
    }

    fn add_methods<I>(&mut self, rpc_methods: I)
    where
        I: IntoIterator<Item = (String, RemoteProcedure<Option<SubscriptionSession>>)>,
    {
        let enable_deprecated_rpc = self.config.enable_deprecated_rpc;
        self.io_handler
            .extend_with(rpc_methods.into_iter().map(|(name, method)| {
                if let Some(deprecated_method_name) = name.strip_prefix(DEPRECATED_RPC_PREFIX) {
                    (
                        deprecated_method_name.to_owned(),
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

    /// Builds the RPC methods handler used in the RPC server.
    pub fn build(self) -> IoHandler {
        let mut io_handler = self.io_handler;
        io_handler.add_sync_method("ping", |_| Ok("pong".into()));

        io_handler
    }
}
