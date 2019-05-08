use crate::error::RPCError;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::{transaction::ProposalShortId, BlockNumber, EpochNumber};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;
use jsonrpc_types::{
    BlockView, CellOutPoint, CellOutputWithOutPoint, CellWithStatus, EpochExt, HeaderView,
    OutPoint, TransactionWithStatus,
};
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub const PAGE_SIZE: u64 = 100;

#[rpc]
pub trait ChainRpc {
    #[rpc(name = "get_block")]
    fn get_block(&self, _hash: H256) -> Result<Option<BlockView>>;

    #[rpc(name = "get_block_by_number")]
    fn get_block_by_number(&self, _number: String) -> Result<Option<BlockView>>;

    #[rpc(name = "get_transaction")]
    fn get_transaction(&self, _hash: H256) -> Result<Option<TransactionWithStatus>>;

    #[rpc(name = "get_block_hash")]
    fn get_block_hash(&self, _number: String) -> Result<Option<H256>>;

    #[rpc(name = "get_tip_header")]
    fn get_tip_header(&self) -> Result<HeaderView>;

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

    #[rpc(name = "get_current_epoch")]
    fn get_current_epoch(&self) -> Result<EpochExt>;

    #[rpc(name = "get_epoch_by_number")]
    fn get_epoch_by_number(&self, number: String) -> Result<Option<EpochExt>>;
}

pub(crate) struct ChainRpcImpl<CS> {
    pub shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> ChainRpc for ChainRpcImpl<CS> {
    fn get_block(&self, hash: H256) -> Result<Option<BlockView>> {
        Ok(self.shared.block(&hash).as_ref().map(Into::into))
    }

    fn get_block_by_number(&self, number: String) -> Result<Option<BlockView>> {
        Ok(self
            .shared
            .block_hash(
                number
                    .parse::<BlockNumber>()
                    .map_err(|_| Error::parse_error())?,
            )
            .and_then(|hash| self.shared.block(&hash).as_ref().map(Into::into)))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithStatus>> {
        let id = ProposalShortId::from_tx_hash(&hash);

        let tx = {
            let chan_state = self.shared.chain_state().lock();

            let tx_pool = chan_state.tx_pool();
            tx_pool
                .get_tx_from_staging(&id)
                .map(TransactionWithStatus::with_proposed)
                .or_else(|| {
                    tx_pool
                        .get_tx_without_conflict(&id)
                        .map(TransactionWithStatus::with_pending)
                })
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

    fn get_tip_header(&self) -> Result<HeaderView> {
        Ok(self
            .shared
            .store()
            .get_tip_header()
            .as_ref()
            .map(Into::into)
            .expect("tip header exists"))
    }

    fn get_current_epoch(&self) -> Result<EpochExt> {
        Ok(self
            .shared
            .store()
            .get_current_epoch_ext()
            .map(Into::into)
            .expect("current_epoch exists"))
    }

    fn get_epoch_by_number(&self, number: String) -> Result<Option<EpochExt>> {
        Ok(self
            .shared
            .store()
            .get_epoch_index(
                number
                    .parse::<EpochNumber>()
                    .map_err(|_| Error::parse_error())?,
            )
            .and_then(|hash| self.shared.store().get_epoch_ext(&hash).map(Into::into)))
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
                    if output.lock.hash() == lock_hash && transaction_meta.is_dead(i) == Some(false)
                    {
                        result.push(CellOutputWithOutPoint {
                            out_point: OutPoint {
                                cell: Some(CellOutPoint {
                                    tx_hash: transaction.hash().to_owned(),
                                    index: i as u32,
                                }),
                                block_hash: None,
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
        let mut cell_status = self.shared.chain_state().lock().cell(
            &(out_point
                .clone()
                .try_into()
                .map_err(|_| Error::parse_error())?),
        );
        if let CellStatus::Live(ref mut cell_meta) = cell_status {
            if cell_meta.cell_output.is_none() {
                cell_meta.cell_output = Some(
                    self.shared
                        .store()
                        .get_cell_output(&cell_meta.out_point.tx_hash, cell_meta.out_point.index)
                        .expect("live cell must exists"),
                );
            }
        }
        Ok(cell_status.into())
    }

    fn get_tip_block_number(&self) -> Result<String> {
        self.get_tip_header().map(|h| h.inner.number)
    }
}
