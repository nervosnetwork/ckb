use crate::{BlockNumber, CellOutput, Uint64};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

// This is used as return value of get_live_cells_by_lock_hash RPC
#[derive(Debug, Serialize, Deserialize)]
pub struct LiveCell {
    pub created_by: TransactionPoint,
    pub cell_output: CellOutput,
    pub output_data_len: Uint64,
    pub cellbase: bool,
}

// This is used as return value of get_transactions_by_lock_hash RPC
#[derive(Debug, Serialize, Deserialize)]
pub struct CellTransaction {
    pub created_by: TransactionPoint,
    pub consumed_by: Option<TransactionPoint>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TransactionPoint {
    pub block_number: BlockNumber,
    pub tx_hash: H256,
    pub index: Uint64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LockHashIndexState {
    pub lock_hash: H256,
    pub block_number: BlockNumber,
    pub block_hash: H256,
}
