use ckb_chain::chain::ChainController;
use ckb_core::block::Block as CoreBlock;
use ckb_jsonrpc_types::Block;
use ckb_logger::error;
use ckb_network::NetworkController;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use numext_fixed_hash::H256;
use std::sync::Arc;

#[rpc]
pub trait IntegrationTestRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"add_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS", "/ip4/192.168.2.100/tcp/30002"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "add_node")]
    fn add_node(&self, peer_id: String, address: String) -> Result<()>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"remove_node","params": ["QmUsZHPbjjzU627UZFt4k8j6ycEcNvXRnVGxCPKqwbAfQS"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "remove_node")]
    fn remove_node(&self, peer_id: String) -> Result<()>;

    #[rpc(name = "process_block_without_verify")]
    fn process_block_without_verify(&self, data: Block) -> Result<Option<H256>>;
}

pub(crate) struct IntegrationTestRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
    pub chain: ChainController,
}

impl<CS: ChainStore + 'static> IntegrationTestRpc for IntegrationTestRpcImpl<CS> {
    fn add_node(&self, peer_id: String, address: String) -> Result<()> {
        self.network_controller.add_node(
            &peer_id.parse().expect("invalid peer_id"),
            address.parse().expect("invalid address"),
        );
        Ok(())
    }

    fn remove_node(&self, peer_id: String) -> Result<()> {
        self.network_controller
            .remove_node(&peer_id.parse().expect("invalid peer_id"));
        Ok(())
    }

    fn process_block_without_verify(&self, data: Block) -> Result<Option<H256>> {
        let block: Arc<CoreBlock> = Arc::new(data.into());
        let ret = self.chain.process_block(Arc::clone(&block), false);
        if ret.is_ok() {
            Ok(Some(block.header().hash().to_owned()))
        } else {
            error!("process_block_without_verify error: {:?}", ret);
            Ok(None)
        }
    }
}
