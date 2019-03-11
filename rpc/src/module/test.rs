use ckb_network::NetworkService;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use std::sync::Arc;

#[rpc]
pub trait IntegrationTestRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;
}

pub(crate) struct IntegrationTestRpcImpl {
    pub network: Arc<NetworkService>,
}

impl IntegrationTestRpc for IntegrationTestRpcImpl {
    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }
}
