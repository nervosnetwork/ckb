use bigint::H256;
use ckb_core::header::RawHeader;
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_util::RwLock;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    pub new_transactions_threshold: u16,
    pub type_hash: H256,
    pub rpc_url: String,
    pub poll_interval: u64,
    pub max_transactions: usize,
    pub max_proposals: usize,
}

#[derive(Clone, Deserialize, Serialize, Debug)]
pub struct BlockTemplate {
    pub raw_header: RawHeader,
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<Transaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

#[derive(Clone)]
pub struct Shared {
    pub inner: Arc<RwLock<Option<BlockTemplate>>>,
}
