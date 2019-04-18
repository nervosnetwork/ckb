use crate::{BlockNumber, CellOutput};
use ckb_core::cell::CellStatus;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};

// This is used as return value of get_live_cells_by_lock_hash RPC
#[derive(Serialize, Deserialize)]
pub struct LiveCellWithOutPoint {
    pub cell: CellOutput,
    pub out_point: TransactionPoint,
}

// This is used as return value of get_transactions_by_lock_hash RPC
#[derive(Serialize, Deserialize)]
pub struct CellTransaction {
    pub out_point: TransactionPoint,
    pub in_point: Option<TransactionPoint>,
}

#[derive(Serialize, Deserialize)]
pub struct TransactionPoint {
    pub block_number: BlockNumber,
    pub hash: H256,
    pub index: u32,
}

#[derive(Serialize, Deserialize)]
pub struct CellWithStatus {
    pub cell: Option<CellOutput>,
    pub status: String,
}

impl From<CellStatus> for CellWithStatus {
    fn from(status: CellStatus) -> Self {
        let (cell, status) = match status {
            CellStatus::Live(cell) => (Some(cell), "live"),
            CellStatus::Dead => (None, "dead"),
            CellStatus::Unknown => (None, "unknown"),
        };
        Self {
            cell: cell.map(|cell| cell.cell_output.into()),
            status: status.to_string(),
        }
    }
}
