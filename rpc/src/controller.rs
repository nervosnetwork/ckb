use std::sync::Arc;

use ckb_app_config::RpcConfig;
use ckb_chain::chain::ChainController;
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_sync::{SyncShared, Synchronizer};
use ckb_types::core::FeeRate;
use ckb_util::Mutex;

use crate::{
    module::{self, Pluginable as _},
    RpcServer, ServiceBuilder,
};

#[doc(hidden)]
pub struct RpcServerController {
    state: RpcServerState,
    server: Option<RpcServer>,
    manager: RpcServerPluginManager,
}

struct RpcServerState {
    rpc_config: RpcConfig,
    shared: Option<Shared>,
    chain_controller: Option<ChainController>,
    network_controller: Option<NetworkController>,
    synchronizer: Option<Synchronizer>,
    sync_shared: Option<Arc<SyncShared>>,
    alert_notifier: Option<Arc<Mutex<AlertNotifier>>>,
    alert_verifier: Option<Arc<AlertVerifier>>,
    min_fee_rate: FeeRate,
    miner_enabled: bool,
}

#[derive(Default)]
struct RpcServerPluginManager {
    chain: module::Plugin<module::ChainRpcImpl>,
    pool: module::Plugin<module::PoolRpcImpl>,
    miner: module::Plugin<module::MinerRpcImpl>,
    net: module::Plugin<module::NetRpcImpl>,
    stats: module::Plugin<module::StatsRpcImpl>,
    experiment: module::Plugin<module::ExperimentRpcImpl>,
    test: module::Plugin<module::IntegrationTestRpcImpl>,
    alert: module::Plugin<module::AlertRpcImpl>,
}

impl RpcServerState {
    fn new(rpc_config: &RpcConfig, min_fee_rate: FeeRate, miner_enabled: bool) -> Self {
        Self {
            rpc_config: rpc_config.clone(),
            shared: None,
            chain_controller: None,
            network_controller: None,
            synchronizer: None,
            sync_shared: None,
            alert_notifier: None,
            alert_verifier: None,
            min_fee_rate,
            miner_enabled,
        }
    }

    fn start_server(&self) -> (RpcServer, RpcServerPluginManager) {
        let mut builder = ServiceBuilder::new(&self.rpc_config);

        let chain_plugin = if let Some(ref shared) = self.shared {
            module::ChainRpcImpl {
                shared: shared.to_owned(),
            }
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_chain(chain_plugin.clone_arc());

        let pool_plugin = if let Self {
            shared: Some(ref shared),
            sync_shared: Some(ref sync_shared),
            ..
        } = self
        {
            module::PoolRpcImpl::new(
                shared.to_owned(),
                Arc::clone(sync_shared),
                self.min_fee_rate,
                self.rpc_config.reject_ill_transactions,
            )
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_pool(pool_plugin.clone_arc());

        let miner_plugin = if let Self {
            shared: Some(ref shared),
            network_controller: Some(ref network_controller),
            chain_controller: Some(ref chain_controller),
            ..
        } = self
        {
            module::MinerRpcImpl {
                shared: shared.to_owned(),
                network_controller: network_controller.to_owned(),
                chain: chain_controller.to_owned(),
            }
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_miner(miner_plugin.clone_arc(), self.miner_enabled);

        let net_plugin = if let Self {
            network_controller: Some(ref network_controller),
            sync_shared: Some(ref sync_shared),
            ..
        } = self
        {
            module::NetRpcImpl {
                network_controller: network_controller.to_owned(),
                sync_shared: Arc::clone(sync_shared),
            }
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_net(net_plugin.clone_arc());

        let stats_plugin = if let Self {
            shared: Some(ref shared),
            synchronizer: Some(ref synchronizer),
            alert_notifier: Some(ref alert_notifier),
            ..
        } = self
        {
            module::StatsRpcImpl {
                shared: shared.to_owned(),
                synchronizer: synchronizer.to_owned(),
                alert_notifier: Arc::clone(alert_notifier),
            }
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_stats(stats_plugin.clone_arc());

        let experiment_plugin = if let Some(ref shared) = self.shared {
            module::ExperimentRpcImpl {
                shared: shared.to_owned(),
            }
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_experiment(experiment_plugin.clone_arc());

        let test_plugin = if let Self {
            shared: Some(ref shared),
            network_controller: Some(ref network_controller),
            chain_controller: Some(ref chain_controller),
            ..
        } = self
        {
            module::IntegrationTestRpcImpl {
                shared: shared.to_owned(),
                network_controller: network_controller.to_owned(),
                chain: chain_controller.to_owned(),
            }
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_integration_test(test_plugin.clone_arc());

        let alert_plugin = if let Self {
            network_controller: Some(ref network_controller),
            alert_verifier: Some(ref alert_verifier),
            alert_notifier: Some(ref alert_notifier),
            ..
        } = self
        {
            module::AlertRpcImpl::new(
                Arc::clone(alert_verifier),
                Arc::clone(alert_notifier),
                network_controller.to_owned(),
            )
            .pluginable()
        } else {
            Default::default()
        };
        builder.enable_alert(alert_plugin.clone_arc());

        builder.enable_debug();
        let io_handler = builder.build();

        let server = RpcServer::new(
            self.rpc_config.clone(),
            io_handler,
            self.shared
                .as_ref()
                .map(|shared| shared.notify_controller()),
        );
        let plugin_manager = RpcServerPluginManager {
            chain: chain_plugin,
            pool: pool_plugin,
            miner: miner_plugin,
            net: net_plugin,
            stats: stats_plugin,
            experiment: experiment_plugin,
            test: test_plugin,
            alert: alert_plugin,
        };
        (server, plugin_manager)
    }
}

impl RpcServerPluginManager {
    fn update_chain(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            shared: Some(ref shared),
            ..
        } = state
        {
            let rpc_impl = module::ChainRpcImpl {
                shared: shared.to_owned(),
            };
            Some(rpc_impl)
        } else {
            None
        };
        self.chain.update(inner_opt);
    }

    fn update_pool(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            shared: Some(ref shared),
            sync_shared: Some(ref sync_shared),
            min_fee_rate,
            ref rpc_config,
            ..
        } = state
        {
            let rpc_impl = module::PoolRpcImpl::new(
                shared.to_owned(),
                Arc::clone(sync_shared),
                *min_fee_rate,
                rpc_config.reject_ill_transactions,
            );
            Some(rpc_impl)
        } else {
            None
        };
        self.pool.update(inner_opt);
    }

    fn update_miner(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            shared: Some(ref shared),
            chain_controller: Some(ref chain_controller),
            network_controller: Some(ref network_controller),
            ..
        } = state
        {
            let rpc_impl = module::MinerRpcImpl {
                shared: shared.to_owned(),
                chain: chain_controller.to_owned(),
                network_controller: network_controller.to_owned(),
            };
            Some(rpc_impl)
        } else {
            None
        };
        self.miner.update(inner_opt);
    }

    fn update_net(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            network_controller: Some(ref network_controller),
            sync_shared: Some(ref sync_shared),
            ..
        } = state
        {
            let rpc_impl = module::NetRpcImpl {
                network_controller: network_controller.to_owned(),
                sync_shared: Arc::clone(sync_shared),
            };
            Some(rpc_impl)
        } else {
            None
        };
        self.net.update(inner_opt);
    }

    fn update_stats(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            shared: Some(ref shared),
            synchronizer: Some(ref synchronizer),
            alert_notifier: Some(ref alert_notifier),
            ..
        } = state
        {
            let rpc_impl = module::StatsRpcImpl {
                shared: shared.to_owned(),
                synchronizer: synchronizer.to_owned(),
                alert_notifier: Arc::clone(alert_notifier),
            };
            Some(rpc_impl)
        } else {
            None
        };
        self.stats.update(inner_opt);
    }

    fn update_experiment(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            shared: Some(ref shared),
            ..
        } = state
        {
            let rpc_impl = module::ExperimentRpcImpl {
                shared: shared.to_owned(),
            };
            Some(rpc_impl)
        } else {
            None
        };
        self.experiment.update(inner_opt);
    }

    fn update_test(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            shared: Some(ref shared),
            network_controller: Some(ref network_controller),
            chain_controller: Some(ref chain_controller),
            ..
        } = state
        {
            let rpc_impl = module::IntegrationTestRpcImpl {
                shared: shared.to_owned(),
                network_controller: network_controller.to_owned(),
                chain: chain_controller.to_owned(),
            };
            Some(rpc_impl)
        } else {
            None
        };
        self.test.update(inner_opt);
    }

    fn update_alert(&mut self, state: &RpcServerState) {
        let inner_opt = if let RpcServerState {
            network_controller: Some(ref network_controller),
            alert_verifier: Some(ref alert_verifier),
            alert_notifier: Some(ref alert_notifier),
            ..
        } = state
        {
            let rpc_impl = module::AlertRpcImpl::new(
                Arc::clone(alert_verifier),
                Arc::clone(alert_notifier),
                network_controller.to_owned(),
            );
            Some(rpc_impl)
        } else {
            None
        };
        self.alert.update(inner_opt);
    }
}

impl RpcServerController {
    /// Create a new RPC server controller.
    pub fn new(rpc_config: &RpcConfig, min_fee_rate: FeeRate, miner_enabled: bool) -> Self {
        let state = RpcServerState::new(rpc_config, min_fee_rate, miner_enabled);
        let server = None;
        let manager = Default::default();
        Self {
            state,
            server,
            manager,
        }
    }

    /// Start the RPC server.
    pub fn start_server(&mut self) {
        if self.server.is_none() {
            let (server, manager) = self.state.start_server();
            self.server.replace(server);
            self.manager = manager;
        }
    }

    /// Shutdown the RPC server.
    pub fn stop_server(&mut self) {
        let server = self.server.take();
        drop(server);
    }

    /// Update `Shared` and related RPC modules.
    pub fn update_shared(&mut self, v: Option<Shared>) -> &mut Self {
        self.state.shared = v;
        let config = &self.state.rpc_config;
        if self.server.is_some()
            && config.subscription_enable()
            && (config.tcp_listen_address.is_some() || config.ws_listen_address.is_some())
        {
            // TODO how to enable subscription without restart the server?
            self.stop_server();
            self.start_server();
        } else {
            self.manager.update_chain(&self.state);
            self.manager.update_pool(&self.state);
            self.manager.update_miner(&self.state);
            self.manager.update_stats(&self.state);
            self.manager.update_experiment(&self.state);
            self.manager.update_test(&self.state);
        }
        self
    }

    /// Update `ChainController` and related RPC modules.
    pub fn update_chain_controller(&mut self, v: Option<ChainController>) -> &mut Self {
        self.state.chain_controller = v;
        self.manager.update_miner(&self.state);
        self.manager.update_test(&self.state);
        self
    }

    /// Update `NetworkController` and related RPC modules.
    pub fn update_network_controller(&mut self, v: Option<NetworkController>) -> &mut Self {
        self.state.network_controller = v;
        self.manager.update_miner(&self.state);
        self.manager.update_net(&self.state);
        self.manager.update_test(&self.state);
        self.manager.update_alert(&self.state);
        self
    }

    /// Update `Synchronizer` and related RPC modules.
    pub fn update_synchronizer(&mut self, v: Option<Synchronizer>) -> &mut Self {
        self.state.synchronizer = v;
        self.manager.update_stats(&self.state);
        self
    }

    /// Update `SyncShared` and related RPC modules.
    pub fn update_sync_shared(&mut self, v: Option<Arc<SyncShared>>) -> &mut Self {
        self.state.sync_shared = v;
        self.manager.update_pool(&self.state);
        self.manager.update_net(&self.state);
        self
    }

    /// Update `AlertNotifier` and related RPC modules.
    pub fn update_alert_notifier(&mut self, v: Option<Arc<Mutex<AlertNotifier>>>) -> &mut Self {
        self.state.alert_notifier = v;
        self.manager.update_stats(&self.state);
        self.manager.update_alert(&self.state);
        self
    }

    /// Update `AlertVerifier` and related RPC modules.
    pub fn update_alert_verifier(&mut self, v: Option<Arc<AlertVerifier>>) -> &mut Self {
        self.state.alert_verifier = v;
        self.manager.update_alert(&self.state);
        self
    }
}
