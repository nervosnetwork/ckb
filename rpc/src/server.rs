use crate::config::Config;
use crate::module::{
    AlertRpc, AlertRpcImpl, ChainRpc, ChainRpcImpl, ExperimentRpc, ExperimentRpcImpl,
    IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl, NetworkRpc, NetworkRpcImpl,
    PoolRpc, PoolRpcImpl, StatsRpc, StatsRpcImpl,
};
use ckb_chain::chain::ChainController;
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkController;
use ckb_network_alert::{notifier::Notifier as AlertNotifier, verifier::Verifier as AlertVerifier};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_sync::Synchronizer;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::{Server, ServerBuilder};
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use std::sync::Arc;

pub struct RpcServer {
    pub(crate) server: Server,
}

impl RpcServer {
    #[allow(clippy::too_many_arguments)]
    pub fn new<CS: ChainStore + 'static>(
        config: Config,
        network_controller: NetworkController,
        shared: Shared<CS>,
        synchronizer: Synchronizer<CS>,
        chain: ChainController,
        block_assembler: Option<BlockAssemblerController>,
        alert_notifier: Arc<AlertNotifier>,
        alert_verifier: Arc<AlertVerifier>,
    ) -> RpcServer
    where
        CS: ChainStore,
    {
        let mut io = IoHandler::new();

        if config.chain_enable() {
            io.extend_with(
                ChainRpcImpl {
                    shared: shared.clone(),
                }
                .to_delegate(),
            );
        }

        if config.pool_enable() {
            io.extend_with(
                PoolRpcImpl::new(shared.clone(), network_controller.clone()).to_delegate(),
            );
        }

        if config.miner_enable() {
            if let Some(block_assembler) = block_assembler {
                io.extend_with(
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

        if config.net_enable() {
            io.extend_with(
                NetworkRpcImpl {
                    network_controller: network_controller.clone(),
                }
                .to_delegate(),
            );
        }

        if config.stats_enable() {
            io.extend_with(
                StatsRpcImpl {
                    shared: shared.clone(),
                    synchronizer: synchronizer.clone(),
                    alert_notifier: Arc::clone(&alert_notifier),
                }
                .to_delegate(),
            );
        }

        if config.experiment_enable() {
            io.extend_with(
                ExperimentRpcImpl {
                    shared: shared.clone(),
                }
                .to_delegate(),
            );
        }

        if config.integration_test_enable() {
            io.extend_with(
                IntegrationTestRpcImpl {
                    network_controller: network_controller.clone(),
                    shared,
                    chain,
                }
                .to_delegate(),
            );
        }

        if config.alert_enable() {
            io.extend_with(
                AlertRpcImpl::new(alert_verifier, alert_notifier, network_controller).to_delegate(),
            );
        }

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ]))
            .threads(config.threads.unwrap_or_else(num_cpus::get))
            .max_request_body_size(config.max_request_body_size)
            .start_http(
                &config
                    .listen_address
                    .parse()
                    .expect("config listen_address parsed"),
            )
            .expect("Jsonrpc initialize");

        RpcServer { server }
    }

    pub fn close(self) {
        self.server.close()
    }
}
