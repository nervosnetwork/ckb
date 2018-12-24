use ckb_network::NetworkService;
use jsonrpc_core::Result;
use jsonrpc_macros::build_rpc_trait;
use std::sync::Arc;

build_rpc_trait! {
    pub trait NetworkRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"send_transaction","params": [{"version":2, "deps":[], "inputs":[], "outputs":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "local_node_id")]
        fn local_node_id(&self) -> Result<Option<String>>;
    }
}

pub(crate) struct NetworkRpcImpl {
    pub network: Arc<NetworkService>,
}

impl NetworkRpc for NetworkRpcImpl {
    fn local_node_id(&self) -> Result<Option<String>> {
        Ok(self.network.external_url())
    }
}
