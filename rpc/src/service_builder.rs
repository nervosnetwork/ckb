use crate::config::Config;
use crate::module::{
    AlertRpc, AlertRpcImpl, ChainRpc, ChainRpcImpl, ExperimentRpc, ExperimentRpcImpl, IndexerRpc,
    IndexerRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl, NetworkRpc,
    NetworkRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use crate::IoHandler;
use ckb_chain::chain::ChainController;
use ckb_indexer::{DefaultIndexerStore, IndexerConfig};
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_sync::SyncShared;
use ckb_sync::Synchronizer;
use ckb_tx_pool::FeeRate;
use ckb_util::Mutex;
use std::sync::Arc;

pub struct ServiceBuilder<'a> {
    config: &'a Config,
    io_handler: IoHandler,
}

impl<'a> ServiceBuilder<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            io_handler: IoHandler::default(),
        }
    }
    pub fn enable_chain(mut self, shared: Shared) -> Self {
        let rpc_method = ChainRpcImpl { shared }.to_delegate();
        if self.config.chain_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("Chain", rpc_method);
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
        let rpc_method =
            PoolRpcImpl::new(shared, sync_shared, min_fee_rate, reject_ill_transactions)
                .to_delegate();
        if self.config.pool_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("Pool", rpc_method);
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
        let rpc_method = MinerRpcImpl {
            shared,
            chain,
            network_controller,
        }
        .to_delegate();
        if enable && self.config.miner_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("Miner", rpc_method);
        }
        self
    }

    pub fn enable_net(mut self, network_controller: NetworkController) -> Self {
        let rpc_method = NetworkRpcImpl { network_controller }.to_delegate();
        if self.config.net_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("Net", rpc_method);
        }
        self
    }

    pub fn enable_stats(
        mut self,
        shared: Shared,
        synchronizer: Synchronizer,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
    ) -> Self {
        let rpc_method = StatsRpcImpl {
            shared,
            synchronizer,
            alert_notifier,
        }
        .to_delegate();
        if self.config.stats_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("Stats", rpc_method);
        }
        self
    }

    pub fn enable_experiment(mut self, shared: Shared) -> Self {
        let rpc_method = ExperimentRpcImpl { shared }.to_delegate();
        if self.config.experiment_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("Experiment", rpc_method);
        }
        self
    }

    pub fn enable_integration_test(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
    ) -> Self {
        let rpc_method = IntegrationTestRpcImpl {
            shared,
            network_controller,
            chain,
        }
        .to_delegate();
        if self.config.integration_test_enable() {
            self.io_handler.extend_with(rpc_method);
        } else {
            self.update_disabled_methods("IntegrationTest", rpc_method);
        }
        self
    }

    pub fn enable_alert(
        mut self,
        alert_verifier: Arc<AlertVerifier>,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
        network_controller: NetworkController,
    ) -> Self {
        let rpc_method =
            AlertRpcImpl::new(alert_verifier, alert_notifier, network_controller).to_delegate();
        if self.config.alert_enable() {
            self.io_handler.extend_with(rpc_method)
        } else {
            self.update_disabled_methods("Alert", rpc_method);
        }
        self
    }

    pub fn enable_indexer(mut self, indexer_config: &IndexerConfig, shared: Shared) -> Self {
        let store = DefaultIndexerStore::new(indexer_config, shared);
        let rpc_method = IndexerRpcImpl {
            store: store.clone(),
        }
        .to_delegate();
        if self.config.indexer_enable() {
            store.start(Some("IndexerStore"));
            self.io_handler.extend_with(rpc_method)
        } else {
            self.update_disabled_methods("Indexer", rpc_method);
        }
        self
    }

    fn update_disabled_methods<I, M>(&mut self, module: &str, rpc_method: I)
    where
        I: IntoIterator<Item = (String, M)>,
    {
        use crate::error::RPCError;

        let error = |method: &str| {
            RPCError::custom(
                RPCError::Invalid,
                format!(
                    "You need to enable `{module}` module to invoke `{method}` rpc, \
                        please modify `rpc.modules` {miner_info} of configuration file ckb.toml and restart the ckb node",
                    method = method, module = module, miner_info = if module == "Miner" {"and `block_assembler`"} else {""}
                ))
        };
        rpc_method
            .into_iter()
            .map(|(method, _)| method)
            .for_each(|method| {
                let error = error(&method);
                self.io_handler
                    .add_method(&method, move |_param| Err(error.clone()))
            });
    }

    pub fn build(self) -> IoHandler {
        let mut io_handler = self.io_handler;
        io_handler.add_method("ping", |_| futures::future::ok("pong".into()));

        io_handler
    }
}
