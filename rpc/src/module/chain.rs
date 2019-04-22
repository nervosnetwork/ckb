use crate::agent::RpcAgentController;
use ckb_core::BlockNumber;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Block, CellOutputWithOutPoint, CellWithStatus, Header, OutPoint, Transaction};
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

#[rpc]
pub trait ChainRpc {
    #[rpc(name = "get_block")]
    fn get_block(&self, _hash: H256) -> Result<Option<Block>>;

    #[rpc(name = "get_transaction")]
    fn get_transaction(&self, _hash: H256) -> Result<Option<Transaction>>;

    #[rpc(name = "get_block_hash")]
    fn get_block_hash(&self, _number: String) -> Result<Option<H256>>;

    #[rpc(name = "get_tip_header")]
    fn get_tip_header(&self) -> Result<Header>;

    #[rpc(name = "get_cells_by_lock_hash")]
    fn get_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _from: String,
        _to: String,
    ) -> Result<Vec<CellOutputWithOutPoint>>;

    #[rpc(name = "get_live_cell")]
    fn get_live_cell(&self, _out_point: OutPoint) -> Result<CellWithStatus>;

    #[rpc(name = "get_tip_block_number")]
    fn get_tip_block_number(&self) -> Result<String>;
}

pub(crate) struct ChainRpcImpl {
    pub agent_controller: Arc<RpcAgentController>,
}

impl ChainRpc for ChainRpcImpl {
    fn get_block(&self, hash: H256) -> Result<Option<Block>> {
        Ok(self
            .agent_controller
            .get_block(hash)
            .as_ref()
            .map(Into::into))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<Transaction>> {
        Ok(self
            .agent_controller
            .get_transaction(hash)
            .as_ref()
            .map(Into::into))
    }

    fn get_block_hash(&self, number: String) -> Result<Option<H256>> {
        Ok(self.agent_controller.get_block_hash(
            number
                .parse::<BlockNumber>()
                .map_err(|_| Error::parse_error())?,
        ))
    }

    fn get_tip_header(&self) -> Result<Header> {
        Ok((&self.agent_controller.get_tip_header()).into())
    }

    // TODO: we need to build a proper index instead of scanning every time
    fn get_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        from: String,
        to: String,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        let from = from
            .parse::<BlockNumber>()
            .map_err(|_| Error::parse_error())?;
        let to = to
            .parse::<BlockNumber>()
            .map_err(|_| Error::parse_error())?;
        let result = self
            .agent_controller
            .get_cells_by_lock_hash(lock_hash, from, to);
        Ok(result)
    }

    fn get_live_cell(&self, out_point: OutPoint) -> Result<CellWithStatus> {
        Ok(self
            .agent_controller
            .get_live_cell(out_point.try_into().map_err(|_| Error::parse_error())?)
            .into())
    }

    fn get_tip_block_number(&self) -> Result<String> {
        Ok(self.agent_controller.get_tip_block_number().to_string())
    }
}
