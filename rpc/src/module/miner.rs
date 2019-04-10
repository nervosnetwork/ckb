use ckb_chain::chain::ChainController;
use ckb_core::block::Block as CoreBlock;
use ckb_miner::BlockAssemblerController;
use ckb_network::{NetworkController, ProtocolId};
use ckb_protocol::RelayMessage;
use ckb_shared::{index::ChainIndex, shared::Shared};
use ckb_sync::NetworkProtocol;
use ckb_traits::ChainProvider;
use ckb_util::TryInto;
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Block, BlockTemplate};
use log::debug;
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::sync::Arc;

#[rpc]
pub trait MinerRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1000, 1000]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_block_template")]
    fn get_block_template(
        &self,
        cycles_limit: Option<u64>,
        bytes_limit: Option<u64>,
        max_version: Option<u32>,
    ) -> Result<BlockTemplate>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "commit_transactions":[], "proposal_transactions":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "submit_block")]
    fn submit_block(&self, _work_id: String, _data: Block) -> Result<Option<H256>>;
}

pub(crate) struct MinerRpcImpl<CI> {
    pub network_controller: NetworkController,
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

    fn submit_block(&self, _work_id: String, data: Block) -> Result<Option<H256>> {
        let block: Arc<CoreBlock> = Arc::new(data.try_into().map_err(|_| Error::parse_error())?);
        let resolver = HeaderResolverWrapper::new(block.header(), self.shared.clone());
        let header_verifier = HeaderVerifier::new(
            self.shared.clone(),
            Arc::clone(&self.shared.consensus().pow_engine()),
        );

        let header_verify_ret = header_verifier.verify(&resolver);
        if header_verify_ret.is_ok() {
            let ret = self.chain.process_block(Arc::clone(&block));
            if ret.is_ok() {
                // announce new block
                self.network_controller.with_protocol_context(
                    NetworkProtocol::RELAY as ProtocolId,
                    |mut nc| {
                        let fbb = &mut FlatBufferBuilder::new();
                        let message =
                            RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                        fbb.finish(message, None);
                        for peer in nc.connected_peers() {
                            let _ = nc.send(peer, fbb.finished_data().to_vec());
                        }
                    },
                );
                Ok(Some(block.header().hash().clone()))
            } else {
                debug!(target: "rpc", "submit_block process_block {:?}", ret);
                Ok(None)
            }
        } else {
            debug!(target: "rpc", "submit_block header verifier {:?}", header_verify_ret);
            Ok(None)
        }
    }
}
