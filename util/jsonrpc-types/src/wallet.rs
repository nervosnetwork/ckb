use crate::{BlockNumber, CellOutput};
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

// This is used as return value of get_live_cells_by_lock_hash RPC
#[derive(Serialize, Deserialize)]
pub struct LiveCell {
    pub created_by: TransactionPoint,
    pub cell: CellOutput,
}

// This is used as return value of get_transactions_by_lock_hash RPC
#[derive(Serialize, Deserialize)]
pub struct CellTransaction {
    pub created_by: TransactionPoint,
    pub cell: CellOutput,
    pub cosumed_by: Option<TransactionPoint>,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionPoint {
    pub block_number: BlockNumber,
    pub hash: H256,
    pub index: u32,
}
