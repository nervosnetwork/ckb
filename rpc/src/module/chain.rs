use ckb_core::cell::CellProvider;
use ckb_core::BlockNumber;
use ckb_shared::{index::ChainIndex, shared::Shared};
use ckb_traits::ChainProvider;
use ckb_util::TryInto;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{Block, CellOutputWithOutPoint, CellWithStatus, Header, OutPoint, Transaction};
use numext_fixed_hash::H256;

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

pub(crate) struct ChainRpcImpl<CI> {
    pub shared: Shared<CI>,
}

impl<CI: ChainIndex + 'static> ChainRpc for ChainRpcImpl<CI> {
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

    // TODO: we need to build a proper index instead of scanning every time
    fn get_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        from: String,
        to: String,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        let mut result = Vec::new();
        let chain_state = self.shared.chain_state().lock();
        let from = from
            .parse::<BlockNumber>()
            .map_err(|_| Error::parse_error())?;
        let to = to
            .parse::<BlockNumber>()
            .map_err(|_| Error::parse_error())?;
        for block_number in from..=to {
            if let Some(block_hash) = self.shared.block_hash(block_number) {
                let block = self
                    .shared
                    .block(&block_hash)
                    .ok_or_else(Error::internal_error)?;
                for transaction in block.commit_transactions() {
                    let transaction_meta = chain_state
                        .cell_set()
                        .get(&transaction.hash())
                        .ok_or_else(Error::internal_error)?;
                    for (i, output) in transaction.outputs().iter().enumerate() {
                        if output.lock.hash() == lock_hash && (!transaction_meta.is_dead(i)) {
                            result.push(CellOutputWithOutPoint {
                                out_point: OutPoint {
                                    hash: transaction.hash().clone(),
                                    index: i as u32,
                                },
                                capacity: output.capacity,
                                lock: output.lock.clone(),
                            });
                        }
                    }
                }
            }
        }
        Ok(result)
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
