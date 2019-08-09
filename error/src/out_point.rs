use failure::Fail;
use numext_fixed_hash::H256;
use std::fmt::{Debug, Formatter, Result as FmtResult};

// NOTE: To avoid circle dependency, we re-define another `ckb_core::OutPoint`,
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

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The specified cell is already dead
    // NOTE: the original name is Dead
    #[fail(display = "Dead cell")]
    DeadCell(OutPoint),

    /// The specified cell is unknown in the chain
    // NOTE: the original name is Unknown
    #[fail(display = "Unknown cell")]
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
