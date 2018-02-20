use super::transaction::Transaction;
use bigint::{H256, H512, U256};
use bincode::serialize;
use hash::sha3_256;
use proof::Proof;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
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
    pub fn hash(&self) -> H256 {
        sha3_256(serialize(self).unwrap()).into()
    }
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct Block {
    pub header: Header,
    pub transactions: Vec<Transaction>,
}

impl Block {
    pub fn hash(&self) -> H256 {
        self.header.hash()
    }

    pub fn validate(&self) -> bool {
        true
    }
}
