use crate::config::Config;
use crate::module::{
    ChainRpc, ChainRpcImpl, IntegrationTestRpc, IntegrationTestRpcImpl, MinerRpc, MinerRpcImpl,
    NetworkRpc, NetworkRpcImpl, PoolRpc, PoolRpcImpl, TraceRpc, TraceRpcImpl,
};
use ckb_chain::chain::ChainController;
use ckb_miner::AgentController;
use ckb_network::NetworkService;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_pow::Clicker;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::Shared;
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use log::info;
use std::sync::Arc;

pub struct RpcServer {
    pub config: Config,
}

impl RpcServer {
    pub fn start<CI: ChainIndex + 'static>(
        &self,
        network: Arc<NetworkService>,
        shared: Shared<CI>,
        tx_pool: TransactionPoolController,
        chain: ChainController,
        agent: AgentController,
        test_engine: Option<Arc<Clicker>>,
    ) where
        CI: ChainIndex,
    {
        let mut io = IoHandler::new();

        if self.config.chain_enable() {
            io.extend_with(
                ChainRpcImpl {
                    shared: shared.clone(),
                }
                .to_delegate(),
            );
        }

        if self.config.pool_enable() {
            io.extend_with(
                PoolRpcImpl {
                    network: Arc::clone(&network),
                    tx_pool: tx_pool.clone(),
                }
                .to_delegate(),
            );
        }

        if self.config.miner_enable() {
            io.extend_with(
                MinerRpcImpl {
                    shared,
                    agent,
                    chain,
                    network: Arc::clone(&network),
                }
                .to_delegate(),
            );
        }

        if self.config.net_enable() {
            io.extend_with(
                NetworkRpcImpl {
                    network: Arc::clone(&network),
                }
                .to_delegate(),
            );
        }

        if self.config.trace_enable() {
            io.extend_with(
                TraceRpcImpl {
                    network: Arc::clone(&network),
                    tx_pool,
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
            .threads(self.config.threads.unwrap_or_else(num_cpus::get))
            .max_request_body_size(self.config.max_request_body_size)
            .start_http(&self.config.listen_address.parse().unwrap())
            .unwrap();

        info!(target: "rpc", "Now listening on {:?}", server.address());
        server.wait();
    }
}
