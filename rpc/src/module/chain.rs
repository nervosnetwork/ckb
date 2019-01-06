use crate::types::{BlockWithHash, CellOutputWithOutPoint, CellWithStatus, TransactionWithHash};
use ckb_core::cell::CellProvider;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::OutPoint;
use ckb_shared::{
    index::ChainIndex,
    shared::{ChainProvider, Shared},
};
use jsonrpc_core::{Error, Result};
use jsonrpc_macros::build_rpc_trait;
use numext_fixed_hash::H256;

build_rpc_trait! {
    pub trait ChainRpc {
        #[rpc(name = "get_block")]
        fn get_block(&self, _hash: H256) -> Result<Option<BlockWithHash>>;

        #[rpc(name = "get_transaction")]
        fn get_transaction(&self, _hash: H256) -> Result<Option<TransactionWithHash>>;

        #[rpc(name = "get_block_hash")]
        fn get_block_hash(&self, _number: u64) -> Result<Option<H256>>;

        #[rpc(name = "get_tip_header")]
        fn get_tip_header(&self) -> Result<Header>;

        #[rpc(name = "get_cells_by_type_hash")]
        fn get_cells_by_type_hash(
            &self,
            _type_hash: H256,
            _from: BlockNumber,
            _to: BlockNumber
        ) -> Result<Vec<CellOutputWithOutPoint>>;

        #[rpc(name = "get_live_cell")]
        fn get_live_cell(&self, _out_point: OutPoint) -> Result<CellWithStatus>;

        #[rpc(name = "get_tip_block_number")]
        fn get_tip_block_number(&self) -> Result<BlockNumber>;
    }
}

pub(crate) struct ChainRpcImpl<CI> {
    pub shared: Shared<CI>,
}

impl<CI: ChainIndex + 'static> ChainRpc for ChainRpcImpl<CI> {
    fn get_block(&self, hash: H256) -> Result<Option<BlockWithHash>> {
        Ok(self.shared.block(&hash).map(Into::into))
    }

    fn get_transaction(&self, hash: H256) -> Result<Option<TransactionWithHash>> {
        Ok(self.shared.get_transaction(&hash).map(Into::into))
    }

    fn get_block_hash(&self, number: BlockNumber) -> Result<Option<H256>> {
        Ok(self.shared.block_hash(number))
    }

    fn get_tip_header(&self) -> Result<Header> {
        Ok(self.shared.chain_state().read().tip_header().clone())
    }

    // TODO: we need to build a proper index instead of scanning every time
    fn get_cells_by_type_hash(
        &self,
        type_hash: H256,
        from: BlockNumber,
        to: BlockNumber,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        let mut result = Vec::new();
        for block_number in from..=to {
            if let Some(block_hash) = self.shared.block_hash(block_number) {
                let block = self
                    .shared
                    .block(&block_hash)
                    .ok_or_else(Error::internal_error)?;
                let chain_state = self.shared.chain_state().read();
                for transaction in block.commit_transactions() {
                    let transaction_meta = chain_state
                        .txo_set()
                        .get(&transaction.hash())
                        .ok_or_else(Error::internal_error)?;
                    for (i, output) in transaction.outputs().iter().enumerate() {
                        if output.lock == type_hash && (!transaction_meta.is_spent(i)) {
                            result.push(CellOutputWithOutPoint {
                                out_point: OutPoint::new(transaction.hash().clone(), i as u32),
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
        Ok(self.shared.cell(&out_point).into())
    }

    fn get_tip_block_number(&self) -> Result<BlockNumber> {
        Ok(self.shared.chain_state().read().tip_number())
    }
}
