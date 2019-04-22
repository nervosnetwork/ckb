use crate::agent::RpcAgent;
use crate::config::Config;
use crate::module::{
    ChainRpc, ChainRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl,
    NetworkRpc, NetworkRpcImpl, PoolRpc, PoolRpcImpl, TraceRpc, TraceRpcImpl,
};
use ckb_chain::chain::ChainController;
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkController;
use ckb_shared::shared::Shared;
use ckb_shared::store::ChainStore;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::{Server, ServerBuilder};
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use std::sync::Arc;

pub struct RpcServer {
    server: Server,
}

impl RpcServer {
    pub fn new<CS: ChainStore + 'static>(
        config: Config,
        network_controller: NetworkController,
        shared: Shared<CS>,
        chain: ChainController,
        block_assembler: BlockAssemblerController,
    ) -> RpcServer
    where
        CS: ChainStore,
    {
        let rpc_agent = RpcAgent::new(
            network_controller.clone(),
            shared.clone(),
            chain.clone(),
            block_assembler.clone(),
        );
        let agent_controller = Arc::new(rpc_agent.start(Some("RPC agent")));
        let mut io = IoHandler::new();

        if config.chain_enable() {
            io.extend_with(
                ChainRpcImpl {
                    agent_controller: Arc::clone(&agent_controller),
                }
                .to_delegate(),
            );
        }

        if config.pool_enable() {
            io.extend_with(
                PoolRpcImpl {
                    agent_controller: Arc::clone(&agent_controller),
                }
                .to_delegate(),
            );
        }

        if config.miner_enable() {
            io.extend_with(
                MinerRpcImpl {
                    agent_controller: Arc::clone(&agent_controller),
                }
                .to_delegate(),
            );
        }

        if config.net_enable() {
            io.extend_with(
                NetworkRpcImpl {
                    agent_controller: Arc::clone(&agent_controller),
                }
                .to_delegate(),
            );
        }

        if config.trace_enable() {
            io.extend_with(
                TraceRpcImpl {
                    agent_controller: Arc::clone(&agent_controller),
                }
                .to_delegate(),
            );
        }

        if config.integration_test_enable() {
            io.extend_with(
                IntegrationTestRpcImpl {
                    agent_controller: Arc::clone(&agent_controller),
                }
                .to_delegate(),
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
