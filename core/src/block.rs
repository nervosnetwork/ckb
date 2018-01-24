use super::transaction::Transaction;

pub struct Block {
    pub height: u64,
    pub timestamp: u64,
    pub prevous_block: [u8; 32],
    pub merkle_root: [u8; 32],
    pub transactions: Vec<Transaction>,
}
