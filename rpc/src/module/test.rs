use ckb_network::NetworkService;
use ckb_pow::Clicker;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::sync::Arc;

#[rpc]
pub trait IntegrationTestRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_pow_solution","params": [1]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "submit_pow_solution")]
    fn submit_pow_solution(&self, _nonce: u64) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;
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

    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }
}
