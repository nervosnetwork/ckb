use ckb_chain::chain::ChainController;
use ckb_core::block::Block as CoreBlock;
use ckb_core::Cycle;
use ckb_miner::BlockAssemblerController;
use ckb_network::NetworkController;
use ckb_protocol::RelayMessage;
use ckb_shared::{shared::Shared, store::ChainStore};
use ckb_sync::NetworkProtocol;
use ckb_traits::ChainProvider;
use ckb_verification::{HeaderResolverWrapper, HeaderVerifier, Verifier};
use faketime::unix_time_as_millis;
use flatbuffers::FlatBufferBuilder;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Block, BlockTemplate};
use log::{debug, error};
use numext_fixed_hash::H256;
use std::collections::HashSet;
use std::convert::TryInto;
use std::sync::Arc;

#[rpc]
pub trait MinerRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_block_template","params": ["0x1b1c832d02fdb4339f9868c8a8636c3d9dd10bd53ac7ce99595825bd6beeffb3", 1000, 1000]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_block_template")]
    fn get_block_template(
        &self,
        cycles_limit: Option<String>,
        bytes_limit: Option<String>,
        max_version: Option<u32>,
    ) -> Result<BlockTemplate>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "transactions":[], "proposals":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "submit_block")]
    fn submit_block(&self, _work_id: String, _data: Block) -> Result<Option<H256>>;
}

pub(crate) struct MinerRpcImpl<CS> {
    pub network_controller: NetworkController,
    pub shared: Shared<CS>,
    pub block_assembler: BlockAssemblerController,
    pub chain: ChainController,
}

impl<CS: ChainStore + 'static> MinerRpc for MinerRpcImpl<CS> {
    fn get_block_template(
        &self,
        cycles_limit: Option<String>,
        bytes_limit: Option<String>,
        max_version: Option<u32>,
    ) -> Result<BlockTemplate> {
        let cycles_limit = match cycles_limit {
            Some(c) => Some(c.parse::<Cycle>().map_err(|_| Error::parse_error())?),
            None => None,
        };
        let bytes_limit = match bytes_limit {
            Some(b) => Some(b.parse::<u64>().map_err(|_| Error::parse_error())?),
            None => None,
        };

        self.block_assembler
            .get_block_template(cycles_limit, bytes_limit, max_version)
            .map_err(|_| Error::internal_error())
    }

    fn submit_block(&self, _work_id: String, data: Block) -> Result<Option<H256>> {
        let block: Arc<CoreBlock> = Arc::new(data.try_into().map_err(|_| Error::parse_error())?);
        let resolver = HeaderResolverWrapper::new(block.header(), self.shared.clone());
        let header_verify_ret = {
            let chain_state = self.shared.chain_state().lock();
            let header_verifier = HeaderVerifier::new(
                &*chain_state,
                Arc::clone(&self.shared.consensus().pow_engine()),
            );
            header_verifier.verify(&resolver)
        };
        if header_verify_ret.is_ok() {
            let ret = self.chain.process_block(Arc::clone(&block));
            if ret.is_ok() {
                debug!(target: "miner", "[block_relay] announce new block {} {}", block.header().hash(), unix_time_as_millis());
                // announce new block

                let fbb = &mut FlatBufferBuilder::new();
                let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                fbb.finish(message, None);
                let data = fbb.finished_data().to_vec();
                self.network_controller
                    .broadcast(NetworkProtocol::RELAY.into(), data);
                Ok(Some(block.header().hash().clone()))
            } else {
                let chain_state = self.shared.chain_state().lock();
                error!(target: "rpc", "submit_block process_block {:?}", ret);
                error!(target: "rpc", "proposal table {}", serde_json::to_string(chain_state.proposal_ids().all()).unwrap());
                Ok(None)
            }
        } else {
            debug!(target: "rpc", "submit_block header verifier {:?}", header_verify_ret);
            Ok(None)
        }
    }
}
