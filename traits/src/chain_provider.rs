use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Capacity, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

pub trait ChainProvider: Sync + Send {
    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>>;

    fn block_header(&self, hash: &H256) -> Option<Header>;

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>>;

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>>;

    fn block_hash(&self, number: BlockNumber) -> Option<H256>;

    fn block_ext(&self, hash: &H256) -> Option<BlockExt>;

    fn block_number(&self, hash: &H256) -> Option<BlockNumber>;

    fn block(&self, hash: &H256) -> Option<Block>;

    fn genesis_hash(&self) -> &H256;

    fn get_transaction(&self, hash: &H256) -> Option<Transaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn block_reward(&self, block_number: BlockNumber) -> Capacity;

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header>;

    fn calculate_difficulty(&self, last: &Header) -> Option<U256>;

    fn consensus(&self) -> &Consensus;
}
