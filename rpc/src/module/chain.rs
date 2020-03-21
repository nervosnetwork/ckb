use crate::error::RPCError;
use ckb_jsonrpc_types::{
    BlockEconomicState, BlockNumber, BlockReward, BlockView, CellOutputWithOutPoint,
    CellWithStatus, EpochNumber, EpochView, HeaderView, OutPoint, TransactionWithStatus,
};
use ckb_logger::{error, warn};
use ckb_reward_calculator::RewardCalculator;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{
    core::{self, cell::CellProvider},
    packed,
    prelude::*,
    H256,
};
use jsonrpc_core::{Error, Result};
use jsonrpc_derive::rpc;

pub const PAGE_SIZE: u64 = 100;

#[rpc(server)]
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
    fn get_live_cell(&self, _out_point: OutPoint, _with_data: bool) -> Result<CellWithStatus>;

    #[rpc(name = "get_tip_block_number")]
    fn get_tip_block_number(&self) -> Result<BlockNumber>;

    #[rpc(name = "get_current_epoch")]
    fn get_current_epoch(&self) -> Result<EpochView>;

    #[rpc(name = "get_epoch_by_number")]
    fn get_epoch_by_number(&self, number: EpochNumber) -> Result<Option<EpochView>>;

    #[rpc(name = "get_cellbase_output_capacity_details")]
    fn get_cellbase_output_capacity_details(&self, _hash: H256) -> Result<Option<BlockReward>>;

    #[rpc(name = "get_block_economic_state")]
    fn get_block_economic_state(&self, _hash: H256) -> Result<Option<BlockEconomicState>>;
}

pub(crate) struct ChainRpcImpl {
    pub shared: Shared,
}

impl ChainRpc for ChainRpcImpl {
    fn get_block(&self, hash: H256) -> Result<Option<BlockView>> {
        let snapshot = self.shared.snapshot();
        if !snapshot.is_main_chain(&hash.pack()) {
            return Ok(None);
        }

        Ok(snapshot.get_block(&hash.pack()).map(Into::into))
    }

    fn get_block_by_number(&self, number: BlockNumber) -> Result<Option<BlockView>> {
        let snapshot = self.shared.snapshot();
        let ret = snapshot
            .get_block_hash(number.into())
            .and_then(|hash| snapshot.get_block(&hash));
        if let Some(ref block) = ret {
            if block.transactions().is_empty() {
                use sentry::{capture_message, with_scope, Level};
                let tip_header_hash = snapshot.tip_header().hash();
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-rpc", "get_block_by_number"])),
                    || {
                        capture_message(
                            &format!(
                                "get_block_by_number transactions is empty {} {} tip_header {}",
                                block.number(),
                                block.hash(),
                                tip_header_hash,
                            ),
                            Level::Warning,
                        )
                    },
                );
                warn!(
                    "get_block_by_number transactions is empty {} {} tip_header {}",
                    block.number(),
                    block.hash(),
                    tip_header_hash,
                );
            }
        };
        Ok(ret.map(Into::into))
    }

    fn get_header(&self, hash: H256) -> Result<Option<HeaderView>> {
        let snapshot = self.shared.snapshot();

        if !snapshot.is_main_chain(&hash.pack()) {
            return Ok(None);
        }

        Ok(snapshot.get_block_header(&hash.pack()).map(Into::into))
    }

    fn get_header_by_number(&self, number: BlockNumber) -> Result<Option<HeaderView>> {
        let snapshot = self.shared.snapshot();
        Ok(snapshot
            .get_block_hash(number.into())
            .and_then(|hash| snapshot.get_block_header(&hash).map(Into::into)))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithStatus>> {
        let hash = hash.pack();
        let id = packed::ProposalShortId::from_tx_hash(&hash);

        let tx = {
            let tx_pool = self.shared.tx_pool_controller();
            let fetch_tx_for_rpc = tx_pool.fetch_tx_for_rpc(id);
            if let Err(e) = fetch_tx_for_rpc {
                error!("send fetch_tx_for_rpc request error {}", e);
                return Err(Error::internal_error());
            };

            fetch_tx_for_rpc.unwrap().map(|(proposed, tx)| {
                if proposed {
                    TransactionWithStatus::with_proposed(tx)
                } else {
                    TransactionWithStatus::with_pending(tx)
                }
            })
        };

        Ok(tx.or_else(|| {
            self.shared
                .snapshot()
                .get_transaction(&hash)
                .map(|(tx, block_hash)| {
                    TransactionWithStatus::with_committed(tx, block_hash.unpack())
                })
        }))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        Ok(self
            .shared
            .snapshot()
            .get_block_hash(number.into())
            .map(|h| h.unpack()))
    }

    fn get_tip_header(&self) -> Result<HeaderView> {
        Ok(self.shared.snapshot().tip_header().clone().into())
    }

    fn get_current_epoch(&self) -> Result<EpochView> {
        Ok(EpochView::from_ext(
            self.shared.snapshot().epoch_ext().pack(),
        ))
    }

    fn get_epoch_by_number(&self, number: EpochNumber) -> Result<Option<EpochView>> {
        let snapshot = self.shared.snapshot();
        Ok(snapshot.get_epoch_index(number.into()).and_then(|hash| {
            snapshot
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
        let from = from.into();
        let to = to.into();
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
                if let Some(transaction_meta) = snapshot.get_tx_meta(&transaction.hash()) {
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
                                capacity: output.capacity().unpack(),
                                lock: output.lock().clone().into(),
                                type_: output.type_().to_opt().map(Into::into),
                                output_data_len: (transaction
                                    .outputs_data()
                                    .get(i)
                                    .expect("verified tx")
                                    .len()
                                    as u64)
                                    .into(),
                                cellbase: transaction_meta.is_cellbase(),
                            });
                        }
                    }
                }
            }
        }
        Ok(result)
    }

    fn get_live_cell(&self, out_point: OutPoint, with_data: bool) -> Result<CellWithStatus> {
        let cell_status = self.shared.snapshot().cell(&out_point.into(), with_data);
        Ok(cell_status.into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        self.get_tip_header().map(|h| h.inner.number)
    }

    fn get_cellbase_output_capacity_details(&self, hash: H256) -> Result<Option<BlockReward>> {
        let snapshot = self.shared.snapshot();

        if !snapshot.is_main_chain(&hash.pack()) {
            return Ok(None);
        }

        Ok(snapshot.get_block_header(&hash.pack()).and_then(|header| {
            snapshot
                .get_block_header(&header.data().raw().parent_hash())
                .and_then(|parent| {
                    if parent.number() < snapshot.consensus().finalization_delay_length() {
                        None
                    } else {
                        RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
                            .block_reward_to_finalize(&parent)
                            .map(|r| r.1.into())
                            .ok()
                    }
                })
        }))
    }

    fn get_block_economic_state(&self, hash: H256) -> Result<Option<BlockEconomicState>> {
        let snapshot = self.shared.snapshot();

        let block_number = if let Some(block_number) = snapshot.get_block_number(&hash.pack()) {
            block_number
        } else {
            return Ok(None);
        };

        let delay_length = snapshot.consensus().finalization_delay_length();
        let finalized_at_number = block_number + delay_length;
        if block_number == 0 || snapshot.tip_number() < finalized_at_number {
            return Ok(None);
        }

        let block_hash = hash.pack();
        let finalized_at = if let Some(block_hash) = snapshot.get_block_hash(finalized_at_number) {
            block_hash
        } else {
            return Ok(None);
        };

        let issuance = if let Some(issuance) = snapshot
            .get_block_epoch_index(&block_hash)
            .and_then(|index| snapshot.get_epoch_ext(&index))
            .and_then(|epoch_ext| {
                let primary = epoch_ext.block_reward(block_number).ok()?;
                let secondary = epoch_ext
                    .secondary_block_issuance(
                        block_number,
                        snapshot.consensus().secondary_epoch_reward(),
                    )
                    .ok()?;
                Some(core::BlockIssuance { primary, secondary })
            }) {
            issuance
        } else {
            return Ok(None);
        };

        let txs_fee = if let Some(txs_fee) =
            snapshot.get_block_ext(&block_hash).and_then(|block_ext| {
                block_ext
                    .txs_fees
                    .iter()
                    .try_fold(core::Capacity::zero(), |acc, tx_fee| acc.safe_add(*tx_fee))
                    .ok()
            }) {
            txs_fee
        } else {
            return Ok(None);
        };

        Ok(snapshot.get_block_header(&block_hash).and_then(|header| {
            RewardCalculator::new(snapshot.consensus(), snapshot.as_ref())
                .block_reward_for_target(&header)
                .ok()
                .map(|(_, block_reward)| core::BlockEconomicState {
                    issuance,
                    miner_reward: block_reward.into(),
                    txs_fee,
                    finalized_at,
                })
                .map(Into::into)
        }))
    }
}
