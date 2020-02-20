use crate::config::Config;
use crate::module::{
    AlertRpc, AlertRpcImpl, ChainRpc, ChainRpcImpl, ExperimentRpc, ExperimentRpcImpl, IndexerRpc,
    IndexerRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl, NetworkRpc,
    NetworkRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use crate::server::ModuleEnableCheck;
use crate::IoHandler;
use ckb_chain::chain::ChainController;
use ckb_indexer::{DefaultIndexerStore, IndexerConfig};
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_sync::SyncSharedState;
use ckb_sync::Synchronizer;
use ckb_tx_pool::FeeRate;
use ckb_util::Mutex;
use jsonrpc_core::MetaIoHandler;
use std::{collections::HashMap, sync::Arc};

pub struct ServiceBuilder<'a> {
    config: &'a Config,
    io_handler: IoHandler,
    disable_methods: HashMap<String, Arc<String>>,
}

impl<'a> ServiceBuilder<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            io_handler: IoHandler::new(MetaIoHandler::new(
                Default::default(),
                ModuleEnableCheck::default(),
            )),
            disable_methods: HashMap::new(),
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
        sync_shared_state: Arc<SyncSharedState>,
        min_fee_rate: FeeRate,
        reject_ill_transactions: bool,
    ) -> Self {
        let rpc_method = PoolRpcImpl::new(
            shared,
            sync_shared_state,
            min_fee_rate,
            reject_ill_transactions,
        )
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
        let module = Arc::new(module.to_string());
        rpc_method
            .into_iter()
            .map(|(method, _)| method)
            .for_each(|method| {
                self.disable_methods.insert(method, Arc::clone(&module));
            });
    }

    pub fn build(self) -> IoHandler {
        let rpc: Vec<_> = Into::<
            jsonrpc_core::MetaIoHandler<
                Option<crate::module::SubscriptionSession>,
                crate::server::ModuleEnableCheck,
            >,
        >::into(self.io_handler)
        .into_iter()
        .collect();

        let mut io = IoHandler::new(MetaIoHandler::new(
            Default::default(),
            ModuleEnableCheck::new(self.disable_methods),
        ));
        io.extend_with(rpc);
        io.add_method("ping", |_| futures::future::ok("pong".into()));

        io
    }
}
