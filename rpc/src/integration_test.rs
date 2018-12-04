use bigint::H256;
use ckb_core::cell::CellProvider;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, Transaction};
use ckb_network::NetworkService;
use ckb_pool::txs_pool::TransactionPoolController;
use ckb_pow::Clicker;
use ckb_protocol::RelayMessage;
use ckb_shared::index::ChainIndex;
use ckb_shared::shared::{ChainProvider, Shared};
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, IoHandler, Result};
use jsonrpc_http_server::ServerBuilder;
use jsonrpc_server_utils::cors::AccessControlAllowOrigin;
use jsonrpc_server_utils::hosts::DomainsValidation;
use server::{ChainRpc, ChainRpcImpl, MinerRpc, MinerRpcImpl, PoolRpc, PoolRpcImpl};
use std::sync::Arc;
use types::Config;

build_rpc_trait! {
    pub trait IntegrationTestRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_solution","params": [1]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "submit_pow_solution")]
        fn submit_pow_solution(&self, u64) -> Result<()>;

        #[rpc(name = "local_node_id")]
        fn local_node_id(&self) -> Result<Option<String>>;

        #[rpc(name = "add_node")]
        fn add_node(&self, String) -> Result<()>;
    }
}

struct IntegrationTestRpcImpl {
    pub network: Arc<NetworkService>,
    pub pow: Arc<Clicker>,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn submit_pow_solution(&self, nonce: u64) -> Result<()> {
        self.pow.submit(nonce);
        Ok(())
    }

    fn local_node_id(&self) -> Result<Option<String>> {
        Ok(self.network.external_url())
    }

    fn add_node(&self, _node_id: String) -> Result<()> {
        unimplemented!()
    }
}

pub struct RpcServer {
    pub config: Config,
}

impl RpcServer {
    pub fn start<CI>(
        &self,
        network: Arc<NetworkService>,
        shared: Shared<CI>,
        tx_pool: TransactionPoolController,
        pow: Arc<Clicker>,
    ) where
        CI: ChainIndex + 'static,
    {
        let mut io = IoHandler::new();
        io.extend_with(
            IntegrationTestRpcImpl {
                network: Arc::clone(&network),
                pow,
            }.to_delegate(),
        );
        io.extend_with(ChainRpcImpl { shared }.to_delegate());
        io.extend_with(PoolRpcImpl { network, tx_pool }.to_delegate());
        io.extend_with(MinerRpcImpl {}.to_delegate());

        let server = ServerBuilder::new(io)
            .cors(DomainsValidation::AllowOnly(vec![
                AccessControlAllowOrigin::Null,
                AccessControlAllowOrigin::Any,
            ])).start_http(&self.config.listen_addr.parse().unwrap())
            .unwrap();

        info!(target: "rpc", "Now listening on {:?}", server.address());
        server.wait();
    }
}
