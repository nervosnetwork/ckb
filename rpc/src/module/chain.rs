use ckb_core::cell::CellProvider;
use ckb_core::BlockNumber;
use ckb_shared::{shared::Shared, store::ChainStore};
use ckb_traits::ChainProvider;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Block, CellWithStatus, Header, OutPoint, Transaction};
use numext_fixed_hash::H256;
use std::convert::TryInto;

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

    #[rpc(name = "get_live_cell")]
    fn get_live_cell(&self, _out_point: OutPoint) -> Result<CellWithStatus>;

    #[rpc(name = "get_tip_block_number")]
    fn get_tip_block_number(&self) -> Result<String>;
}

pub(crate) struct ChainRpcImpl<CS> {
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> ChainRpc for ChainRpcImpl<CS> {
    fn get_block(&self, hash: H256) -> Result<Option<Block>> {
        Ok(self.shared.block(&hash).as_ref().map(Into::into))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<Transaction>> {
        Ok(self.shared.get_transaction(&hash).as_ref().map(Into::into))
    }

    fn get_block_hash(&self, number: String) -> Result<Option<H256>> {
        Ok(self.shared.block_hash(
            number
                .parse::<BlockNumber>()
                .map_err(|_| Error::parse_error())?,
        ))
    }

    fn get_tip_header(&self) -> Result<Header> {
        Ok(self.shared.chain_state().lock().tip_header().into())
    }

    fn get_live_cell(&self, out_point: OutPoint) -> Result<CellWithStatus> {
        Ok(self
            .shared
            .chain_state()
            .lock()
            .cell(&(out_point.try_into().map_err(|_| Error::parse_error())?))
            .into())
    }

    fn get_tip_block_number(&self) -> Result<String> {
        Ok(self.shared.chain_state().lock().tip_number().to_string())
    }
}
