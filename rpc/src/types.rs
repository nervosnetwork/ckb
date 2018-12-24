use ckb_core::block::Block;
use ckb_core::cell::CellStatus;
use ckb_core::header::Header;
use ckb_core::transaction::{Capacity, CellOutput, OutPoint, Transaction};
use numext_fixed_hash::H256;
use serde_derive::Serialize;

#[derive(Serialize)]
pub struct TransactionWithHash {
    pub hash: H256,
    pub transaction: Transaction,
}

impl From<Transaction> for TransactionWithHash {
    fn from(transaction: Transaction) -> Self {
        Self {
            hash: transaction.hash().clone(),
            transaction,
        }
    }
}

#[derive(Serialize)]
pub struct BlockWithHash {
    pub hash: H256,
    pub header: Header,
    pub transactions: Vec<TransactionWithHash>,
}

impl From<Block> for BlockWithHash {
    fn from(block: Block) -> Self {
        Self {
            header: block.header().clone(),
            transactions: block
                .commit_transactions()
                .iter()
                .map(|tx| tx.clone().into())
                .collect(),
            hash: block.header().hash().clone(),
        }
    }
}

// This is used as return value of get_cells_by_type_hash RPC:
// it contains both OutPoint data used for referencing a cell, as well as
// cell's own data such as lock and capacity
#[derive(Serialize)]
pub struct CellOutputWithOutPoint {
    pub out_point: OutPoint,
    pub capacity: Capacity,
    pub lock: H256,
}

#[derive(Serialize)]
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
            cell,
            status: status.to_string(),
        }
    }
}
