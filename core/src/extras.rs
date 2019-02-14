use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq, Default, Debug)]
pub struct BlockExt {
    pub received_at: u64,
    pub total_difficulty: U256,
    pub total_uncles_count: u64,
    pub valid: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Debug)]
pub struct TransactionAddress {
    // Block hash
    pub block_hash: H256,
    // Offset of block transaction in serialized bytes
    pub offset: usize,
    pub length: usize,
}
