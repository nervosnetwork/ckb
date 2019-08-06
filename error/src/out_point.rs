use failure::Fail;
use numext_fixed_hash::H256;
use std::fmt::{Debug, Formatter, Result as FmtResult};

// NOTE: To avoid circle dependency, we re-define a flatten version of `ckb_core::OutPoint`,
// which is used in `OutPointError` to record error context information.
#[derive(PartialEq, Eq, Clone)]
pub struct OutPoint {
    pub cell: Option<CellOutPoint>,
    pub block_hash: Option<H256>,
}

#[derive(PartialEq, Eq, Clone)]
pub struct CellOutPoint {
    pub tx_hash: H256,
    pub index: u32,
}

impl Debug for CellOutPoint {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        f.debug_struct("CellOutPoint")
            .field("tx_hash", &format_args!("{:#x}", self.tx_hash))
            .field("index", &self.index)
            .finish()
    }
}

impl Debug for OutPoint {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        f.debug_struct("OutPoint")
            .field("cell", &self.cell)
            .field(
                "block_hash",
                &self
                    .block_hash
                    .as_ref()
                    .map(|hash| format!("Some({:#x})", hash))
                    .unwrap_or_else(|| "None".to_owned()),
            )
            .finish()
    }
}

/// Cast `ckb_core::transaction::OutPoint` to `ckb_error::OutPoint`
#[macro_export]
macro_rules! into_eop {
    ($cop:expr) => {{
        let (block_hash, cell) = $cop.to_owned().destruct();
        match cell {
            Some(cell) => {
                let (tx_hash, index) = cell.destruct();
                ckb_error::OutPoint {
                    cell: Some(ckb_error::CellOutPoint { tx_hash, index }),
                    block_hash,
                }
            }
            None => ckb_error::OutPoint {
                cell: None,
                block_hash,
            },
        }
    }};
}

/// Cast `ckb_error::OutPoint` to `ckb_core::transaction::OutPoint`
#[macro_export]
macro_rules! from_eop {
    ($eop:expr) => {{
        let ckb_error::OutPoint { cell, block_hash } = $eop;
        match cell {
            Some(cell) => {
                let ckb_error::CellOutPoint { tx_hash, index } = cell;
                ckb_core::transaction::OutPoint {
                    cell: Some(ckb_core::transaction::CellOutPoint { tx_hash, index }),
                    block_hash,
                }
            }
            None => ckb_core::transaction::OutPoint {
                cell: None,
                block_hash,
            },
        }
    }};
}

impl From<(Option<H256>, Option<(H256, u32)>)> for OutPoint {
    fn from((block_hash, cell): (Option<H256>, Option<(H256, u32)>)) -> Self {
        let cell = cell.map(|c| CellOutPoint {
            tx_hash: c.0,
            index: c.1,
        });
        Self { block_hash, cell }
    }
}

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The specified cell is already dead
    // NOTE: the original name is Dead
    #[fail(display = "Dead input-cell")]
    DeadCell(OutPoint),

    /// The specified cell is unknown in the chain
    // NOTE: the original name is Unknown
    #[fail(display = "Unknown input-cell")]
    UnknownCell(Vec<OutPoint>),

    /// The specified input cell is not-found inside the specified header
    // NOTE: the original name is InvalidHeader
    #[fail(display = "Exclusive input-cell in the specified header")]
    ExclusiveInputCell(OutPoint),

    /// Use the out point as input but not specified the input cell
    // NOTE: the original name is UnspecifiedInputCell
    #[fail(display = "Missing input-cell for input-out-point")]
    MissingInputCell(OutPoint),

    /// Empty out point, missing the input cell and header
    // NOTE: the original name is Empty
    #[fail(display = "Missing input-cell and header")]
    MissingInputCellAndHeader,

    /// Unknown the specified header
    #[fail(display = "Unknown header")]
    UnknownHeader(OutPoint),

    /// Input or dep cell reference to a newer cell in the same block
    // NOTE: Maybe replace with `UnknownInputCell`?
    #[fail(display = "Out of order")]
    OutOfOrder(OutPoint),
}
