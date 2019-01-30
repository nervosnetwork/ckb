use build_info::{get_version, Version};
use ckb_network::NetworkService;
use jsonrpc_core::Result;
use jsonrpc_macros::build_rpc_trait;
use jsonrpc_types::{LocalNode, NodeAddress};
use std::sync::Arc;

const MAX_ADDRS: usize = 50;

build_rpc_trait! {
    pub trait NetworkRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"local_node_info","params": []}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "local_node_info")]
        fn local_node_info(&self) -> Result<LocalNode>;
    }
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
}
