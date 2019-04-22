use crate::agent::RpcAgentController;
use ckb_core::block::Block as CoreBlock;
use ckb_core::Cycle;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Block, BlockTemplate};
use numext_fixed_hash::H256;
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

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"submit_block","params": [{"header":{}, "uncles":[], "commit_transactions":[], "proposal_transactions":[]}]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "submit_block")]
    fn submit_block(&self, _work_id: String, _data: Block) -> Result<Option<H256>>;
}

pub(crate) struct MinerRpcImpl {
    pub agent_controller: Arc<RpcAgentController>,
}

impl MinerRpc for MinerRpcImpl {
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
        self.agent_controller
            .get_block_template(cycles_limit, bytes_limit, max_version)
            .map_err(|_| Error::internal_error())
    }

    fn submit_block(&self, work_id: String, data: Block) -> Result<Option<H256>> {
        let block: CoreBlock = data.try_into().map_err(|_| Error::parse_error())?;
        Ok(self.agent_controller.submit_block(work_id, block))
    }
}
