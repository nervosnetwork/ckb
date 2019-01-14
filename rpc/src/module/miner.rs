use ckb_chain::chain::ChainController;
use ckb_core::block::Block;
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkService;
use ckb_protocol::RelayMessage;
use ckb_shared::{index::ChainIndex, shared::Shared};
use ckb_sync::RELAY_PROTOCOL_ID;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_macros::build_rpc_trait;
use jsonrpc_types::BlockTemplate;
use log::debug;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;

build_rpc_trait! {
    pub trait MinerRpc {
        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1000, 1000]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "get_block_template")]
        fn get_block_template(&self, cycles_limit: Option<u64>, bytes_limit:  Option<u64>, max_version: Option<u32>) -> Result<BlockTemplate>;

        // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "commit_transactions":[], "proposal_transactions":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
        #[rpc(name = "submit_block")]
        fn submit_block(&self, _work_id: String, _data: Block) -> Result<H256>;
    }
}

pub(crate) struct MinerRpcImpl<CI> {
    pub network: Arc<NetworkService>,
    pub shared: Shared<CI>,
    pub block_assembler: BlockAssemblerController,
    pub chain: ChainController,
}

impl<CI: ChainIndex + 'static> MinerRpc for MinerRpcImpl<CI> {
    fn get_block_template(
        &self,
        cycles_limit: Option<u64>,
        bytes_limit: Option<u64>,
        max_version: Option<u32>,
    ) -> Result<BlockTemplate> {
        self.block_assembler
            .get_block_template(cycles_limit, bytes_limit, max_version)
            .map_err(|_| Error::internal_error())
    }

    fn submit_block(&self, _work_id: String, data: Block) -> Result<H256> {
        let block = Arc::new(data);
        let ret = self.chain.process_block(Arc::clone(&block));
        if ret.is_ok() {
            // announce new block
            self.network.with_protocol_context(RELAY_PROTOCOL_ID, |nc| {
                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                for peer in nc.connected_peers() {
                    let _ = nc.send(peer, fbb.finished_data().to_vec());
                }
            });
            Ok(block.header().hash().clone())
        } else {
            debug!(target: "rpc", "submit_block process_block {:?}", ret);
            Err(Error::internal_error())
        }
    }
}
