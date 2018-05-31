use bigint::{H256, U256};

#[derive(Clone, Serialize, Deserialize, PartialEq, Default, Debug)]
pub struct BlockExt {
    pub received_at: u64,
    pub total_difficulty: U256,
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Debug)]
pub struct TransactionAddress {
    // Block hash
    pub block_hash: H256,
    // Index of block transaction
    pub index: u32,
}
