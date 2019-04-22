use crate::agent::RpcAgentController;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::Node;
use std::sync::Arc;

#[rpc]
pub trait NetworkRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_info","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "local_node_info")]
    fn local_node_info(&self) -> Result<Node>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_peers","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_peers")]
    fn get_peers(&self) -> Result<Vec<Node>>;
}

pub(crate) struct NetworkRpcImpl {
    pub agent_controller: Arc<RpcAgentController>,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_info(&self) -> Result<Node> {
        Ok(self.agent_controller.local_node_info())
    }

    fn get_peers(&self) -> Result<Vec<Node>> {
        let peers = self.agent_controller.get_peers();
        Ok(peers)
    }
}
