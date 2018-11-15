use bigint::{H256, U256};
use chain_spec::consensus::Consensus;
use ckb_shared::error::SharedError;
use ckb_shared::shared::ChainProvider;
use core::block::Block;
use core::cell::{CellProvider, CellStatus};
use core::extras::BlockExt;
use core::header::{BlockNumber, Header};
use core::transaction::{Capacity, OutPoint, ProposalShortId, Transaction};
use core::transaction_meta::TransactionMeta;
use core::uncle::UncleBlock;
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct DummyChainProvider {
    pub transaction_fees: HashMap<H256, Result<Capacity, SharedError>>,
    pub block_reward: Capacity,
}

impl ChainProvider for DummyChainProvider {
    fn block_reward(&self, _block_number: BlockNumber) -> Capacity {
        self.block_reward
    }

    fn calculate_transaction_fee(
        &self,
        transaction: &Transaction,
    ) -> Result<Capacity, SharedError> {
        self.transaction_fees[&transaction.hash()].clone()
    }

    fn union_proposal_ids_n(&self, _bn: BlockNumber, _n: usize) -> Vec<Vec<ProposalShortId>> {
        panic!("Not implemented!");
    }

    fn block_ext(&self, _hash: &H256) -> Option<BlockExt> {
        panic!("Not implemented!");
    }

    fn genesis_hash(&self) -> H256 {
        panic!("Not implemented!");
    }

    fn block_body(&self, _hash: &H256) -> Option<Vec<Transaction>> {
        panic!("Not implemented!");
    }

    fn block_header(&self, _hash: &H256) -> Option<Header> {
        panic!("Not implemented!");
    }

    fn block_proposal_txs_ids(&self, _hash: &H256) -> Option<Vec<ProposalShortId>> {
        panic!("Not implemented!");
    }

    fn block_hash(&self, _height: u64) -> Option<H256> {
        panic!("Not implemented!");
    }

    fn get_ancestor(&self, _base: &H256, _number: BlockNumber) -> Option<Header> {
        panic!("Not implemented!");
    }

    fn output_root(&self, _hash: &H256) -> Option<H256> {
        panic!("Not implemented!");
    }

    fn block_number(&self, _hash: &H256) -> Option<BlockNumber> {
        panic!("Not implemented!");
    }

    fn uncles(&self, _hash: &H256) -> Option<Vec<UncleBlock>> {
        panic!("Not implemented!");
    }

    fn block(&self, _hash: &H256) -> Option<Block> {
        panic!("Not implemented!");
    }

    fn get_transaction(&self, _hash: &H256) -> Option<Transaction> {
        panic!("Not implemented!");
    }

    fn contain_transaction(&self, _hash: &H256) -> bool {
        panic!("Not implemented!");
    }

    fn get_transaction_meta(&self, _output_root: &H256, _hash: &H256) -> Option<TransactionMeta> {
        panic!("Not implemented!");
    }

    fn get_transaction_meta_at(&self, _hash: &H256, _parent: &H256) -> Option<TransactionMeta> {
        panic!("Not implemented!");
    }

    fn calculate_difficulty(&self, _last: &Header) -> Option<U256> {
        panic!("Not implemented!");
    }

    fn consensus(&self) -> &Consensus {
        panic!("Not implemented!");
    }
}

impl CellProvider for DummyChainProvider {
    fn cell(&self, _o: &OutPoint) -> CellStatus {
        panic!("Not implemented!");
    }

    fn cell_at(&self, _out_point: &OutPoint, _parent: &H256) -> CellStatus {
        panic!("Not implemented!");
    }
}
