use crate::transaction::{CellOutPoint, OutPoint};
use ckb_error::{CellOutPoint as ErrorCellOutPoint, OutPoint as ErrorOutPoint};

impl From<OutPoint> for ErrorOutPoint {
    fn from(out_point: OutPoint) -> Self {
        Self {
            cell: out_point.cell.map(Into::into),
            block_hash: out_point.block_hash,
        }
    }
}

impl From<ErrorOutPoint> for OutPoint {
    fn from(out_point: ErrorOutPoint) -> Self {
        Self {
            cell: out_point.cell.map(Into::into),
            block_hash: out_point.block_hash,
        }
    }
}

impl From<CellOutPoint> for ErrorCellOutPoint {
    fn from(cell_out_point: CellOutPoint) -> Self {
        Self {
            tx_hash: cell_out_point.tx_hash,
            index: cell_out_point.index,
        }
    }
}

impl From<ErrorCellOutPoint> for CellOutPoint {
    fn from(cell_out_point: ErrorCellOutPoint) -> Self {
        Self {
            tx_hash: cell_out_point.tx_hash,
            index: cell_out_point.index,
        }
    }
}
