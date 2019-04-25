use build_info::{get_version, Version};
use ckb_network::NetworkController;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::{Node, NodeAddress};

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
    pub network_controller: NetworkController,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_info(&self) -> Result<Node> {
        Ok(Node {
            version: get_version!().to_string(),
            is_outbound: None,
            node_id: self.network_controller.node_id(),
            addresses: self
                .network_controller
                .external_urls(MAX_ADDRS)
                .into_iter()
                .map(|(address, score)| NodeAddress { address, score })
                .collect(),
        })
    }

    fn get_peers(&self) -> Result<Vec<Node>> {
        let peers = self.network_controller.connected_peers();
        Ok(peers
            .into_iter()
            .map(|(peer_id, peer, addresses)| Node {
                is_outbound: Some(peer.is_outbound()),
                version: peer
                    .identify_info
                    .map(|info| info.client_version)
                    .unwrap_or_else(|| "unknown".to_string()),
                node_id: peer_id.to_base58(),
                // TODO how to get correct port and score?
                addresses: addresses
                    .into_iter()
                    .map(|(address, score)| NodeAddress {
                        address: address.to_string(),
                        score,
                    })
                    .collect(),
            })
            .collect())
    }
}
