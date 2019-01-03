use ckb_network::NetworkService;
use ckb_pow::Clicker;
use jsonrpc_core::Result;
use jsonrpc_macros::build_rpc_trait;
use std::sync::Arc;

build_rpc_trait! {
    pub trait IntegrationTestRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_solution","params": [1]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "submit_pow_solution")]
        fn submit_pow_solution(&self, _nonce: u64) -> Result<()>;

        #[rpc(name = "add_node")]
        fn add_node(&self, _node_id: String) -> Result<()>;
    }
}

pub(crate) struct IntegrationTestRpcImpl {
    pub network: Arc<NetworkService>,
    pub test_engine: Arc<Clicker>,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn submit_pow_solution(&self, nonce: u64) -> Result<()> {
        self.test_engine.submit(nonce);
        Ok(())
    }

    fn add_node(&self, _node_id: String) -> Result<()> {
        unimplemented!()
    }
}
