//! Transaction using Cell.
//! It is similar to Bitcoin Tx https://en.bitcoin.it/wiki/Protocol_documentation#tx
pub struct OutPoint {
    pub hash: [u8; 32],
    pub index: u32,
}

pub enum Lock {
    LockForever,
    LockByScript(Vec<u8>),
}

pub struct TransactionInput {
    pub previous_output: OutPoint,
    // Script used to unlock stake. It can be empty, then the stake and its lock script must be
    // found untouched in the output.
    pub stake_unlock: Vec<u8>,
    // script used to unlock data. It can be empty, then the data and its lock script must be found
    // untouched in the output.
    pub data_unlock: Vec<u8>,
}

pub struct TransactionOutput {
    pub stake: i64,
    pub stake_lock: Lock,
    pub data: Vec<u8>,
    pub data_lock: Lock,
}

pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<TransactionInput>,
    pub outputs: Vec<TransactionOutput>,
}
