use crate::error::RPCError;
use ckb_jsonrpc_types::{
    BlockNumber, BlockReward, BlockView, Capacity, CellOutputWithOutPoint, CellWithStatus,
    EpochNumber, EpochView, HeaderView, OutPoint, TransactionWithStatus,
};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::ChainProvider;
use ckb_types::{core::cell::CellProvider, packed, prelude::*, H256};
use futures::future::Future;
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;

pub const PAGE_SIZE: u64 = 100;

#[rpc]
pub trait ChainRpc {
    #[rpc(name = "get_block")]
    fn get_block(&self, _hash: H256) -> Result<Option<BlockView>>;

    #[rpc(name = "get_block_by_number")]
    fn get_block_by_number(&self, _number: BlockNumber) -> Result<Option<BlockView>>;

    #[rpc(name = "get_header")]
    fn get_header(&self, _hash: H256) -> Result<Option<HeaderView>>;

    #[rpc(name = "get_header_by_number")]
    fn get_header_by_number(&self, _number: BlockNumber) -> Result<Option<HeaderView>>;

    #[rpc(name = "get_transaction")]
    fn get_transaction(&self, _hash: H256) -> Result<Option<TransactionWithStatus>>;

    #[rpc(name = "get_block_hash")]
    fn get_block_hash(&self, _number: BlockNumber) -> Result<Option<H256>>;

    #[rpc(name = "get_tip_header")]
    fn get_tip_header(&self) -> Result<HeaderView>;

    #[rpc(name = "get_cells_by_lock_hash")]
    fn get_cells_by_lock_hash(
        &self,
        _lock_hash: H256,
        _from: BlockNumber,
        _to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>>;

    #[rpc(name = "get_live_cell")]
    fn get_live_cell(&self, _out_point: OutPoint) -> Result<CellWithStatus>;

    #[rpc(name = "get_tip_block_number")]
    fn get_tip_block_number(&self) -> Result<BlockNumber>;

    #[rpc(name = "get_current_epoch")]
    fn get_current_epoch(&self) -> Result<EpochView>;

    #[rpc(name = "get_epoch_by_number")]
    fn get_epoch_by_number(&self, number: EpochNumber) -> Result<Option<EpochView>>;

    #[rpc(name = "get_cellbase_output_capacity_details")]
    fn get_cellbase_output_capacity_details(&self, _hash: H256) -> Result<Option<BlockReward>>;
}

pub(crate) struct ChainRpcImpl {
    pub shared: Shared,
}

impl ChainRpc for ChainRpcImpl {
    fn get_block(&self, hash: H256) -> Result<Option<BlockView>> {
        Ok(self.shared.store().get_block(&hash.pack()).map(Into::into))
    }

    fn get_block_by_number(&self, number: BlockNumber) -> Result<Option<BlockView>> {
        Ok(self
            .shared
            .store()
            .get_block_hash(number.0)
            .and_then(|hash| self.shared.store().get_block(&hash).map(Into::into)))
    }

    fn get_header(&self, hash: H256) -> Result<Option<HeaderView>> {
        Ok(self
            .shared
            .store()
            .get_block_header(&hash.pack())
            .map(Into::into))
    }

    fn get_header_by_number(&self, number: BlockNumber) -> Result<Option<HeaderView>> {
        Ok(self
            .shared
            .store()
            .get_block_hash(number.0)
            .and_then(|hash| self.shared.store().get_block_header(&hash).map(Into::into)))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithStatus>> {
        let hash = hash.pack();
        let id = packed::ProposalShortId::from_tx_hash(&hash);

        let tx = {
            let tx_pool = self.shared.tx_pool_controller();
            tx_pool
                .fetch_tx_for_rpc(id)
                .unwrap()
                .wait()
                .unwrap()
                .map(|(proposed, tx)| {
                    if proposed {
                        TransactionWithStatus::with_proposed(tx)
                    } else {
                        TransactionWithStatus::with_pending(tx)
                    }
                })
        };

        Ok(tx.or_else(|| {
            self.shared
                .store()
                .get_transaction(&hash)
                .map(|(tx, block_hash)| {
                    TransactionWithStatus::with_committed(tx, block_hash.unpack())
                })
        }))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        Ok(self
            .shared
            .store()
            .get_block_hash(number.0)
            .map(|h| h.unpack()))
    }

    fn get_tip_header(&self) -> Result<HeaderView> {
        Ok(self
            .shared
            .store()
            .get_tip_header()
            .map(Into::into)
            .expect("tip header exists"))
    }

    fn get_current_epoch(&self) -> Result<EpochView> {
        Ok(self
            .shared
            .store()
            .get_current_epoch_ext()
            .map(|ext| EpochView::from_ext(ext.pack()))
            .expect("current_epoch exists"))
    }

    fn get_epoch_by_number(&self, number: EpochNumber) -> Result<Option<EpochView>> {
        Ok(self
            .shared
            .store()
            .get_epoch_index(number.0)
            .and_then(|hash| {
                self.shared
                    .store()
                    .get_epoch_ext(&hash)
                    .map(|ext| EpochView::from_ext(ext.pack()))
            }))
    }

    // TODO: we need to build a proper index instead of scanning every time
    fn get_cells_by_lock_hash(
        &self,
        lock_hash: H256,
        from: BlockNumber,
        to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        let lock_hash = lock_hash.pack();
        let mut result = Vec::new();
        let snapshot = self.shared.snapshot();
        let from = from.0;
        let to = to.0;
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
            let block_hash = snapshot.get_block_hash(block_number);
            if block_hash.is_none() {
                break;
            }

            let block_hash = block_hash.unwrap();
            let block = snapshot
                .get_block(&block_hash)
                .ok_or_else(Error::internal_error)?;
            for transaction in block.transactions() {
                if let Some(transaction_meta) = snapshot.cell_set().get(&transaction.hash()) {
                    for (i, output) in transaction.outputs().into_iter().enumerate() {
                        if output.calc_lock_hash() == lock_hash
                            && transaction_meta.is_dead(i) == Some(false)
                        {
                            let out_point = packed::OutPoint::new_builder()
                                .tx_hash(transaction.hash())
                                .index(i.pack())
                                .build();
                            result.push(CellOutputWithOutPoint {
                                out_point: out_point.into(),
                                block_hash: block_hash.unpack(),
                                capacity: Capacity(output.capacity().unpack()),
                                lock: output.lock().clone().into(),
                            });
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    fn get_live_cell(&self, out_point: OutPoint) -> Result<CellWithStatus> {
        let cell_status = self
            .shared
            .snapshot()
            .cell(&out_point.clone().into(), false);
        Ok(cell_status.into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        self.get_tip_header().map(|h| h.inner.number)
    }

    fn get_cellbase_output_capacity_details(&self, hash: H256) -> Result<Option<BlockReward>> {
        Ok(self
            .shared
            .store()
            .get_block_header(&hash.pack())
            .and_then(|header| {
                self.shared
                    .store()
                    .get_block_header(&header.data().raw().parent_hash())
                    .and_then(|parent| {
                        self.shared
                            .finalize_block_reward(&parent)
                            .map(|r| r.1.into())
                            .ok()
                    })
            }))
    }
}
