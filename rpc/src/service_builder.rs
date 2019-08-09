use crate::config::Config;
use crate::module::{
    AlertRpc, AlertRpcImpl, ChainRpc, ChainRpcImpl, ExperimentRpc, ExperimentRpcImpl, IndexerRpc,
    IndexerRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl, NetworkRpc,
    NetworkRpcImpl, PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use ckb_chain::chain::ChainController;
use ckb_db::DBConfig;
use ckb_indexer::DefaultIndexerStore;
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_sync::SyncSharedState;
use ckb_sync::Synchronizer;
use ckb_util::Mutex;
use jsonrpc_core::IoHandler;
use std::sync::Arc;

pub struct ServiceBuilder<'a> {
    config: &'a Config,
    io_handler: IoHandler,
}

impl<'a> ServiceBuilder<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            config,
            io_handler: IoHandler::new(),
        }
    }
    pub fn enable_chain(mut self, shared: Shared) -> Self {
        if self.config.chain_enable() {
            self.io_handler
                .extend_with(ChainRpcImpl { shared }.to_delegate());
        }
        self
    }

    pub fn enable_pool(mut self, shared: Shared, sync_shared_state: Arc<SyncSharedState>) -> Self {
        if self.config.pool_enable() {
            self.io_handler
                .extend_with(PoolRpcImpl::new(shared, sync_shared_state).to_delegate());
        }
        self
    }

    pub fn enable_miner(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
        block_assembler: Option<BlockAssemblerController>,
    ) -> Self {
        if let Some(block_assembler) = block_assembler {
            if self.config.miner_enable() {
                self.io_handler.extend_with(
                    MinerRpcImpl {
                        shared: shared.clone(),
                        block_assembler,
                        chain: chain.clone(),
                        network_controller: network_controller.clone(),
                    }
                    .to_delegate(),
                );
            }
        }
        self
    }

    pub fn enable_net(mut self, network_controller: NetworkController) -> Self {
        if self.config.net_enable() {
            self.io_handler
                .extend_with(NetworkRpcImpl { network_controller }.to_delegate());
        }
        self
    }

    pub fn enable_stats(
        mut self,
        shared: Shared,
        synchronizer: Synchronizer,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
    ) -> Self {
        if self.config.stats_enable() {
            self.io_handler.extend_with(
                StatsRpcImpl {
                    shared,
                    synchronizer,
                    alert_notifier,
                }
                .to_delegate(),
            );
        }
        self
    }

    pub fn enable_experiment(mut self, shared: Shared) -> Self {
        if self.config.experiment_enable() {
            self.io_handler
                .extend_with(ExperimentRpcImpl { shared }.to_delegate());
        }
        self
    }

    pub fn enable_integration_test(
        mut self,
        shared: Shared,
        network_controller: NetworkController,
        chain: ChainController,
    ) -> Self {
        if self.config.integration_test_enable() {
            self.io_handler.extend_with(
                IntegrationTestRpcImpl {
                    shared,
                    network_controller,
                    chain,
                }
                .to_delegate(),
            );
        }
        self
    }

    pub fn enable_alert(
        mut self,
        alert_verifier: Arc<AlertVerifier>,
        alert_notifier: Arc<Mutex<AlertNotifier>>,
        network_controller: NetworkController,
    ) -> Self {
        if self.config.alert_enable() {
            self.io_handler.extend_with(
                AlertRpcImpl::new(alert_verifier, alert_notifier, network_controller).to_delegate(),
            )
        }
        self
    }

    pub fn enable_indexer(mut self, db_config: &DBConfig, shared: Shared) -> Self {
        if self.config.indexer_enable() {
            let store = DefaultIndexerStore::new(db_config, shared);
            store.clone().start(Some("IndexerStore"));

            self.io_handler
                .extend_with(IndexerRpcImpl { store }.to_delegate())
        }
        self
    }

    pub fn build(self) -> IoHandler {
        self.io_handler
    }
}
