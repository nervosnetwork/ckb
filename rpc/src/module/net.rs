use build_info::{get_version, Version};
use ckb_network::NetworkService;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::{LocalNode, NodeAddress, RemoteNode};
use std::sync::Arc;

const MAX_ADDRS: usize = 50;

#[rpc]
pub trait NetworkRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_info","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "local_node_info")]
    fn local_node_info(&self) -> Result<LocalNode>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_peers","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_peers")]
    fn get_peers(&self) -> Result<Vec<RemoteNode>>;
}

pub(crate) struct NetworkRpcImpl {
    pub network: Arc<NetworkService>,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_info(&self) -> Result<LocalNode> {
        Ok(LocalNode {
            version: get_version!().to_string(),
            node_id: self.network.node_id(),
            addresses: self
                .network
                .external_urls(MAX_ADDRS)
                .into_iter()
                .map(|(address, score)| NodeAddress { address, score })
                .collect(),
        })
    }

    fn get_peers(&self) -> Result<Vec<RemoteNode>> {
        let peers = self.network.connected_peers();
        Ok(peers
            .into_iter()
            .map(|(peer_id, peer)| RemoteNode {
                version: peer
                    .identify_info
                    .map(|info| info.client_version)
                    .unwrap_or_else(|| "unknown".to_string()),
                node_id: peer_id.to_base58(),
                address: peer.connected_addr.to_string(),
            })
            .collect())
    }
}
