use std::{
    io,
    sync::{mpsc, Arc},
    thread,
};

use ckb_app_config::{IndexerConfig, RpcConfig};
use ckb_chain::chain::ChainController;
use ckb_fee_estimator::FeeRate;
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_sync::{SyncShared, Synchronizer};
use ckb_util::Mutex;

use crate::{RpcServer, ServiceBuilder};

type ControlError = String;

enum RpcServerControl {
    Off,
    On,
    Reload,
    RpcConfig(RpcConfig),
    IndexerConfig(IndexerConfig),
    Shared(Option<Shared>),
    ChainController(Option<ChainController>),
    NetworkController(Option<NetworkController>),
    Synchronizer(Option<Synchronizer>),
    SyncShared(Option<Arc<SyncShared>>),
    AlertNotifier(Option<Arc<Mutex<AlertNotifier>>>),
    AlertVerifier(Option<Arc<AlertVerifier>>),
    // TODO Remove this copy, use a global min fee rate.
    MinFeeRate(FeeRate),
    MinerEnabled(bool),
}

pub struct RpcServerController {
    sender: mpsc::Sender<RpcServerControl>,
}

struct RpcServerState {
    rpc_config: RpcConfig,
    indexer_config: IndexerConfig,
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

impl RpcServerState {
    #[allow(clippy::too_many_arguments)]
    fn new(
        rpc_config: &RpcConfig,
        indexer_config: &IndexerConfig,
        shared: Option<Shared>,
        chain_controller: Option<ChainController>,
        network_controller: Option<NetworkController>,
        synchronizer: Option<Synchronizer>,
        sync_shared: Option<Arc<SyncShared>>,
        alert_notifier: Option<Arc<Mutex<AlertNotifier>>>,
        alert_verifier: Option<Arc<AlertVerifier>>,
        min_fee_rate: FeeRate,
        miner_enabled: bool,
    ) -> Self {
        Self {
            rpc_config: rpc_config.clone(),
            indexer_config: indexer_config.clone(),
            shared,
            chain_controller,
            network_controller,
            synchronizer,
            sync_shared,
            alert_notifier,
            alert_verifier,
            min_fee_rate,
            miner_enabled,
        }
    }

    fn start_server(&self) -> RpcServer {
        let mut builder = ServiceBuilder::new(&self.rpc_config);
        if let Some(ref shared) = self.shared {
            builder = builder.enable_chain(shared.to_owned());
        }
        if let Self {
            shared: Some(ref shared),
            sync_shared: Some(ref sync_shared),
            ..
        } = self
        {
            builder = builder.enable_pool(
                shared.to_owned(),
                Arc::clone(sync_shared),
                self.min_fee_rate,
                self.rpc_config.reject_ill_transactions,
            )
        }
        if let Self {
            shared: Some(ref shared),
            network_controller: Some(ref network_controller),
            chain_controller: Some(ref chain_controller),
            ..
        } = self
        {
            builder = builder.enable_miner(
                shared.to_owned(),
                network_controller.to_owned(),
                chain_controller.to_owned(),
                self.miner_enabled,
            )
        }
        if let Self {
            network_controller: Some(ref network_controller),
            sync_shared: Some(ref sync_shared),
            ..
        } = self
        {
            builder = builder.enable_net(network_controller.to_owned(), Arc::clone(sync_shared))
        }
        if let Self {
            shared: Some(ref shared),
            synchronizer: Some(ref synchronizer),
            alert_notifier: Some(ref alert_notifier),
            ..
        } = self
        {
            builder = builder.enable_stats(
                shared.to_owned(),
                synchronizer.to_owned(),
                Arc::clone(alert_notifier),
            );
        }
        if let Some(ref shared) = self.shared {
            builder = builder.enable_experiment(shared.to_owned());
        }
        if let Self {
            shared: Some(ref shared),
            network_controller: Some(ref network_controller),
            chain_controller: Some(ref chain_controller),
            ..
        } = self
        {
            builder = builder.enable_integration_test(
                shared.to_owned(),
                network_controller.to_owned(),
                chain_controller.to_owned(),
            )
        }
        if let Self {
            network_controller: Some(ref network_controller),
            alert_verifier: Some(ref alert_verifier),
            alert_notifier: Some(ref alert_notifier),
            ..
        } = self
        {
            builder = builder.enable_alert(
                Arc::clone(alert_verifier),
                Arc::clone(alert_notifier),
                network_controller.to_owned(),
            )
        }
        if let Some(ref shared) = self.shared {
            builder = builder.enable_indexer(&self.indexer_config, shared.to_owned());
        }
        builder = builder.enable_debug();
        let io_handler = builder.build();
        RpcServer::new(
            self.rpc_config.clone(),
            io_handler,
            self.shared
                .as_ref()
                .map(|shared| shared.notify_controller()),
        )
    }
}

impl RpcServerController {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rpc_config: &RpcConfig,
        indexer_config: &IndexerConfig,
        shared: Option<Shared>,
        chain_controller: Option<ChainController>,
        network_controller: Option<NetworkController>,
        synchronizer: Option<Synchronizer>,
        sync_shared: Option<Arc<SyncShared>>,
        alert_notifier: Option<Arc<Mutex<AlertNotifier>>>,
        alert_verifier: Option<Arc<AlertVerifier>>,
        min_fee_rate: FeeRate,
        miner_enabled: bool,
    ) -> Result<Self, io::Error> {
        let mut state = RpcServerState::new(
            rpc_config,
            indexer_config,
            shared,
            chain_controller,
            network_controller,
            synchronizer,
            sync_shared,
            alert_notifier,
            alert_verifier,
            min_fee_rate,
            miner_enabled,
        );
        let (sender, receiver) = mpsc::channel();
        // TODO async
        thread::Builder::new()
            .name("RpcServerController".to_owned())
            .spawn(move || {
                let mut current_server = None;
                while let Ok(msg) = receiver.recv() {
                    match msg {
                        RpcServerControl::Off => {
                            current_server.replace(None);
                        }
                        RpcServerControl::On => {
                            if current_server.is_none() {
                                let server = state.start_server();
                                current_server.replace(Some(server));
                            }
                        }
                        RpcServerControl::Reload => {
                            if let Some(old_server) = current_server.replace(None) {
                                drop(old_server);
                            }
                            let server = state.start_server();
                            current_server.replace(Some(server));
                        }
                        RpcServerControl::RpcConfig(rpc_config) => {
                            state.rpc_config = rpc_config;
                        }
                        RpcServerControl::IndexerConfig(indexer_config) => {
                            state.indexer_config = indexer_config;
                        }
                        RpcServerControl::Shared(shared) => {
                            state.shared = shared;
                        }
                        RpcServerControl::ChainController(chain_controller) => {
                            state.chain_controller = chain_controller;
                        }
                        RpcServerControl::NetworkController(network_controller) => {
                            state.network_controller = network_controller;
                        }
                        RpcServerControl::Synchronizer(synchronizer) => {
                            state.synchronizer = synchronizer;
                        }
                        RpcServerControl::SyncShared(sync_shared) => {
                            state.sync_shared = sync_shared;
                        }
                        RpcServerControl::AlertNotifier(alert_notifier) => {
                            state.alert_notifier = alert_notifier;
                        }
                        RpcServerControl::AlertVerifier(alert_verifier) => {
                            state.alert_verifier = alert_verifier;
                        }
                        RpcServerControl::MinFeeRate(min_fee_rate) => {
                            state.min_fee_rate = min_fee_rate;
                        }
                        RpcServerControl::MinerEnabled(miner_enabled) => {
                            state.miner_enabled = miner_enabled;
                        }
                    }
                }
            })
            .map(|_| Self { sender })
    }

    fn error_handle(err: mpsc::SendError<RpcServerControl>) -> ControlError {
        err.to_string()
    }

    pub fn switch_on(&self) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::On)
            .map_err(Self::error_handle)
    }

    pub fn switch_off(&self) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::Off)
            .map_err(Self::error_handle)
    }

    pub fn reload(&self) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::Reload)
            .map_err(Self::error_handle)
    }

    pub fn update_rpc_config(&self, v: RpcConfig) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::RpcConfig(v))
            .map_err(Self::error_handle)
    }

    pub fn update_indexer_config(&self, v: IndexerConfig) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::IndexerConfig(v))
            .map_err(Self::error_handle)
    }

    pub fn update_shared(&self, v: Option<Shared>) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::Shared(v))
            .map_err(Self::error_handle)
    }

    pub fn update_chain_controller(&self, v: Option<ChainController>) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::ChainController(v))
            .map_err(Self::error_handle)
    }

    pub fn update_network_controller(
        &self,
        v: Option<NetworkController>,
    ) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::NetworkController(v))
            .map_err(Self::error_handle)
    }

    pub fn update_synchronizer(&self, v: Option<Synchronizer>) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::Synchronizer(v))
            .map_err(Self::error_handle)
    }

    pub fn update_sync_shared(&self, v: Option<Arc<SyncShared>>) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::SyncShared(v))
            .map_err(Self::error_handle)
    }

    pub fn update_alert_notifier(
        &self,
        v: Option<Arc<Mutex<AlertNotifier>>>,
    ) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::AlertNotifier(v))
            .map_err(Self::error_handle)
    }

    pub fn update_alert_verifier(&self, v: Option<Arc<AlertVerifier>>) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::AlertVerifier(v))
            .map_err(Self::error_handle)
    }

    pub fn update_min_fee_rate(&self, v: FeeRate) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::MinFeeRate(v))
            .map_err(Self::error_handle)
    }

    pub fn update_miner_enabled(&self, v: bool) -> Result<(), ControlError> {
        self.sender
            .send(RpcServerControl::MinerEnabled(v))
            .map_err(Self::error_handle)
    }
}
