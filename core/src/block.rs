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

impl Header {
    pub fn parent_hash(&self) -> &H256 {
        &self.parent_hash
    }
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }
    pub fn height(&self) -> u64 {
        self.height
    }
    pub fn transactions_root(&self) -> &H256 {
        &self.transactions_root
    }
    pub fn difficulty(&self) -> &U256 {
        &self.difficulty
    }
    pub fn challenge(&self) -> &H256 {
        &self.challenge
    }
    pub fn proof(&self) -> &Proof {
        &self.proof
    }
    pub fn signature(&self) -> &H512 {
        &self.signature
    }
}

pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
}
