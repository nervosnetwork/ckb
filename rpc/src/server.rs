use crate::config::Config;
use crate::module::{
    ChainRpc, ChainRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl,
    NetworkRpc, NetworkRpcImpl, PoolRpc, PoolRpcImpl, TraceRpc, TraceRpcImpl,
};
use ckb_chain::chain::ChainController;
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkService;
use ckb_pow::Clicker;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::{Server, ServerBuilder};
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use std::sync::Arc;

pub struct RpcServer {
    server: Server,
}

impl RpcServer {
    pub fn new<CI: ChainIndex + 'static>(
        config: Config,
        network: Arc<NetworkService>,
        shared: Shared<CI>,
        chain: ChainController,
        block_assembler: BlockAssemblerController,
        test_engine: Option<Arc<Clicker>>,
    ) -> RpcServer
    where
        CI: ChainIndex,
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
                PoolRpcImpl {
                    network: Arc::clone(&network),
                    shared: shared.clone(),
                }
                .to_delegate(),
            );
        }

        if config.miner_enable() {
            io.extend_with(
                MinerRpcImpl {
                    shared: shared.clone(),
                    block_assembler,
                    chain,
                    network: Arc::clone(&network),
                }
                .to_delegate(),
            );
        }

        if config.net_enable() {
            io.extend_with(
                NetworkRpcImpl {
                    network: Arc::clone(&network),
                }
                .to_delegate(),
            );
        }

        if config.trace_enable() {
            io.extend_with(
                TraceRpcImpl {
                    network: Arc::clone(&network),
                    shared,
                }
                .to_delegate(),
            );
        }

        if test_engine.is_some() {
            io.extend_with(
                IntegrationTestRpcImpl {
                    network,
                    test_engine: test_engine.expect("pow engine supply"),
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
            .start_http(&config.listen_address.parse().unwrap())
            .expect("Jsonrpc initialize");

        RpcServer { server }
    }

    pub fn close(self) {
        self.server.close()
    }
}
