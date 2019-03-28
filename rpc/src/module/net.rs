use build_info::{get_version, Version};
use ckb_network::NetworkService;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::{Node, NodeAddress};
use std::sync::Arc;

const MAX_ADDRS: usize = 50;

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
    pub network: Arc<NetworkService>,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_info(&self) -> Result<Node> {
        Ok(Node {
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

    fn get_peers(&self) -> Result<Vec<Node>> {
        let peers = self.network.connected_peers();
        Ok(peers
            .into_iter()
            .map(|(peer_id, peer)| Node {
                version: peer
                    .identify_info
                    .map(|info| info.client_version)
                    .unwrap_or_else(|| "unknown".to_string()),
                node_id: peer_id.to_base58(),
                // TODO how to get correct port and score?
                addresses: vec![NodeAddress {
                    address: peer.connected_addr.to_string(),
                    score: 0,
                }],
            })
            .collect())
    }
}
