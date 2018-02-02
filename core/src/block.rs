use super::transaction::Transaction;
use bigint::{H256, H512, U256};
use proof::Proof;

pub struct Header {
    /// Parent hash.
    pub parent_hash: H256,
    /// Block timestamp.
    pub timestamp: u64,
    /// Block height.
    pub height: u64,
    /// Transactions root.
    pub transactions_root: H256,
    /// Block difficulty.
    pub difficulty: U256,
    /// block challenge
    pub challenge: H256,
    /// Block proof
    pub proof: Proof,
    /// Block signature
    pub signature: H512,
}

pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
}
