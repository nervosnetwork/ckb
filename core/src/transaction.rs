//! Transaction using Cell.
//! It is similar to Bitcoin Tx https://en.bitcoin.it/wiki/Protocol_documentation#tx

pub struct OutPoint {
    // Hash of Transaction
    pub hash: [u8; 32],
    // Index of cell_operations
    pub index: u32,
}

pub enum Lock {
    LockForever,
    LockByScript(Vec<u8>),
}

pub struct CellInput {
    pub previous_output: OutPoint,
    // Depends on whether the operation is Transform or Destroy, this is the proof to transform
    // lock or destroy lock.
    pub unlock: Vec<u8>,
}

pub struct CellOutput {
    pub data: Vec<u8>,
    pub capacity: u64,
    pub transform_lock: Lock,
    pub destroy_lock: Option<Lock>,
}

pub struct CellOperation {
    pub input: Option<CellInput>,
    pub output: Option<CellOutput>,
}

pub struct Transaction {
    pub version: u32,
    pub cell_operations: Vec<CellOperation>,
}
