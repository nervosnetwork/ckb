use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::Block;
use ckb_core::extras::{BlockExt, EpochExt};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use numext_fixed_hash::H256;
use std::sync::Arc;

pub trait ChainProvider: Sync + Send {
    type Store: ChainStore;

    fn store(&self) -> &Arc<Self::Store>;

    fn script_config(&self) -> &ScriptConfig;

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>>;

    fn block_header(&self, hash: &H256) -> Option<Header>;

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>>;

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>>;

    fn block_hash(&self, number: BlockNumber) -> Option<H256>;

    fn block_ext(&self, hash: &H256) -> Option<BlockExt>;

    fn block_number(&self, hash: &H256) -> Option<BlockNumber>;

    fn block(&self, hash: &H256) -> Option<Block>;

    fn genesis_hash(&self) -> &H256;

    fn get_transaction(&self, hash: &H256) -> Option<(Transaction, H256)>;

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header>;

    fn get_block_epoch(&self, hash: &H256) -> Option<EpochExt>;

    fn next_epoch_ext(&self, last_epoch: &EpochExt, header: &Header) -> Option<EpochExt>;

    fn consensus(&self) -> &Consensus;
}
