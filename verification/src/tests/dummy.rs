use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::extras::{BlockExt, EpochExt};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_traits::ChainProvider;
use numext_fixed_hash::H256;

#[derive(Clone)]
pub struct DummyChainProvider {}

impl ChainProvider for DummyChainProvider {
    fn block_ext(&self, _hash: &H256) -> Option<BlockExt> {
        unimplemented!();
    }

    fn genesis_hash(&self) -> &H256 {
        unimplemented!();
    }

    fn block_body(&self, _hash: &H256) -> Option<Vec<Transaction>> {
        unimplemented!();
    }

    fn block_header(&self, _hash: &H256) -> Option<Header> {
        unimplemented!();
    }

    fn block_proposal_txs_ids(&self, _hash: &H256) -> Option<Vec<ProposalShortId>> {
        unimplemented!();
    }

    fn block_hash(&self, _height: u64) -> Option<H256> {
        unimplemented!();
    }

    fn get_ancestor(&self, _base: &H256, _number: BlockNumber) -> Option<Header> {
        unimplemented!();
    }

    fn block_number(&self, _hash: &H256) -> Option<BlockNumber> {
        unimplemented!();
    }

    fn uncles(&self, _hash: &H256) -> Option<Vec<UncleBlock>> {
        unimplemented!();
    }

    fn block(&self, _hash: &H256) -> Option<Block> {
        unimplemented!();
    }

    fn get_transaction(&self, _hash: &H256) -> Option<(Transaction, H256)> {
        unimplemented!();
    }

    fn contain_transaction(&self, _hash: &H256) -> bool {
        unimplemented!();
    }

    fn get_epoch_ext(&self, _hash: &H256) -> Option<EpochExt> {
        unimplemented!();
    }

    fn next_epoch_ext(&self, _last_epoch: &EpochExt, _header: &Header) -> Option<EpochExt> {
        unimplemented!();
    }

    fn consensus(&self) -> &Consensus {
        unimplemented!();
    }
}

impl CellProvider for DummyChainProvider {
    fn cell(&self, _o: &OutPoint) -> CellStatus {
        unimplemented!();
    }
}
