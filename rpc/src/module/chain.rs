use crate::error::RPCError;
use ckb_core::cell::CellProvider;
use ckb_core::{transaction::ProposalShortId, BlockNumber};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{
    Block, CellOutputWithOutPoint, CellWithStatus, Header, OutPoint, TransactionWithStatus,
};
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub const PAGE_SIZE: u64 = 100;

#[rpc]
pub trait ChainRpc {
    #[rpc(name = "get_block")]
    fn get_block(&self, _hash: H256) -> Result<Option<Block>>;

    #[rpc(name = "get_transaction")]
    fn get_transaction(&self, _hash: H256) -> Result<Option<TransactionWithStatus>>;

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

pub(crate) struct ChainRpcImpl<CS> {
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> ChainRpc for ChainRpcImpl<CS> {
    fn get_block(&self, hash: H256) -> Result<Option<Block>> {
        Ok(self.shared.block(&hash).as_ref().map(Into::into))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithStatus>> {
        let id = ProposalShortId::from_tx_hash(&hash);

        let tx = {
            let chan_state = self.shared.chain_state().lock();

            let tx_pool = chan_state.tx_pool();
            tx_pool
                .get_tx_from_staging(&id)
                .map(TransactionWithStatus::with_proposed)
                .or_else(|| tx_pool.get_tx(&id).map(TransactionWithStatus::with_pending))
        };

        Ok(tx.or_else(|| {
            self.shared
                .get_transaction(&hash)
                .map(|(tx, block_hash)| TransactionWithStatus::with_committed(tx, block_hash))
        }))
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
        if from > to {
            return Err(RPCError::custom(
                RPCError::Invalid,
                "from greater than to".to_owned(),
            ));
        } else if to - from > PAGE_SIZE {
            return Err(RPCError::custom(
                RPCError::Invalid,
                "too large page size".to_owned(),
            ));
        }

        for block_number in from..=to {
            let block_hash = self.shared.block_hash(block_number);
            if block_hash.is_none() {
                break;
            }

            let block_hash = block_hash.unwrap();
            let block = self
                .shared
                .block(&block_hash)
                .ok_or_else(Error::internal_error)?;
            for transaction in block.transactions() {
                let transaction_meta = chain_state
                    .cell_set()
                    .get(&transaction.hash())
                    .ok_or_else(Error::internal_error)?;
                for (i, output) in transaction.outputs().iter().enumerate() {
                    if output.lock.hash() == lock_hash && (!transaction_meta.is_dead(i)) {
                        result.push(CellOutputWithOutPoint {
                            out_point: OutPoint {
                                tx_hash: transaction.hash().clone(),
                                index: i as u32,
                            },
                            capacity: output.capacity.to_string(),
                            lock: output.lock.clone().into(),
                        });
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
            .get_cell_status(&(out_point.try_into().map_err(|_| Error::parse_error())?))
            .into())
    }

    fn get_tip_block_number(&self) -> Result<String> {
        Ok(self.shared.chain_state().lock().tip_number().to_string())
    }
}
