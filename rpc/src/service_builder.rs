use crate::error::RPCError;
use crate::module::SubscriptionSession;
use crate::module::{
    AlertRpc, AlertRpcImpl, ChainRpc, ChainRpcImpl, DebugRpc, DebugRpcImpl, ExperimentRpc,
    ExperimentRpcImpl, IndexerRpc, IndexerRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl,
    MinerRpc, MinerRpcImpl, NetworkRpc, NetworkRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc,
    StatsRpcImpl,
};
use crate::IoHandler;
use ckb_app_config::IndexerConfig;
use ckb_app_config::RpcConfig;
use ckb_chain::chain::ChainController;
use ckb_fee_estimator::FeeRate;
use ckb_indexer::DefaultIndexerStore;
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_sync::SyncShared;
use ckb_sync::Synchronizer;
use ckb_util::Mutex;
use jsonrpc_core::RemoteProcedure;
use std::sync::Arc;

const DEPRECATED_RPC_PREFIX: &str = "deprecated.";

pub struct ServiceBuilder<'a> {
    config: &'a RpcConfig,
    io_handler: IoHandler,
}

impl<'a> ServiceBuilder<'a> {
    pub fn new(config: &'a RpcConfig) -> Self {
        Self {
            config,
            io_handler: IoHandler::default(),
        }
    }
    pub fn enable_chain(mut self, shared: Shared) -> Self {
        let rpc_methods = ChainRpcImpl { shared }.to_delegate();
        if self.config.chain_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Chain", rpc_methods);
        }
        self
    }

    pub fn enable_pool(
        mut self,
        shared: Shared,
        sync_shared: Arc<SyncShared>,
        min_fee_rate: FeeRate,
        reject_ill_transactions: bool,
    ) -> Self {
        let rpc_methods =
            PoolRpcImpl::new(shared, sync_shared, min_fee_rate, reject_ill_transactions)
                .to_delegate();
        if self.config.pool_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Pool", rpc_methods);
        }
        self
    }

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

    pub fn enable_net(
        mut self,
        network_controller: NetworkController,
        sync_shared: Arc<SyncShared>,
    ) -> Self {
        let rpc_methods = NetworkRpcImpl {
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

    pub fn enable_stats(
        mut self,
        shared: Shared,
        synchronizer: Synchronizer,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
    ) -> Self {
        let rpc_methods = StatsRpcImpl {
            shared,
            synchronizer,
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

    pub fn enable_experiment(mut self, shared: Shared) -> Self {
        let rpc_methods = ExperimentRpcImpl { shared }.to_delegate();
        if self.config.experiment_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Experiment", rpc_methods);
        }
        self
    }

    pub fn enable_integration_test(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
    ) -> Self {
        let rpc_methods = IntegrationTestRpcImpl {
            shared,
            network_controller,
            chain,
        }
        .to_delegate();
        if self.config.integration_test_enable() {
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("IntegrationTest", rpc_methods);
        }
        self
    }

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

    pub fn enable_indexer(mut self, indexer_config: &IndexerConfig, shared: Shared) -> Self {
        let store = DefaultIndexerStore::new(indexer_config, shared);
        let rpc_methods = IndexerRpcImpl {
            store: store.clone(),
        }
        .to_delegate();
        if self.config.indexer_enable() {
            store.start(Some("IndexerStore"));
            self.add_methods(rpc_methods);
        } else {
            self.update_disabled_methods("Indexer", rpc_methods);
        }
        self
    }

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
            self.io_handler
                .add_method(&name, move |_param| error.clone())
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
                            RemoteProcedure::Method(Arc::new(|_param, _meta| {
                                Err(RPCError::rpc_method_is_deprecated())
                            }))
                        },
                    )
                } else {
                    (name, method)
                }
            }));
    }

    pub fn build(self) -> IoHandler {
        let mut io_handler = self.io_handler;
        io_handler.add_method("ping", |_| futures::future::ok("pong".into()));

        io_handler
    }
}
