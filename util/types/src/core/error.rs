use crate::generated::packed::{Byte32, OutPoint};
use ckb_error::{Error, ErrorKind};
use failure::Fail;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The specified cell is already dead
    #[fail(display = "Dead({:?})", _0)]
    Dead(OutPoint),

    /// The specified cells is unknown in the chain
    #[fail(display = "Unknown({:?})", _0)]
    Unknown(Vec<OutPoint>),

    /// The specified input cell is not-found inside the specified header
    #[fail(display = "InvalidHeader({:?})", _0)]
    InvalidHeader(OutPoint),

    /// Use the out point as input but not specified the input cell
    #[fail(display = "UnspecifiedInputCell({:?})", _0)]
    UnspecifiedInputCell(OutPoint),

    /// Empty out point, missing the input cell and header
    #[fail(display = "Empty({:?})", _0)]
    Empty(OutPoint),

    /// Unknown the specified header
    #[fail(display = "UnknownHeader({:?})", _0)]
    UnknownHeader(OutPoint),

    /// Input or dep cell reference to a newer cell in the same block
    // TODO: Maybe replace with `UnknownInputCell`?
    #[fail(display = "OutOfOrder({:?})", _0)]
    OutOfOrder(OutPoint),

    /// The output is referenced as a dep-group output, but the data
    /// is invalid format
    #[fail(display = "InvalidDepGroup({:?})", _0)]
    InvalidDepGroup(OutPoint),

    /// Invalid HeaderDep
    #[fail(display = "InvalidHeaderDep({})", _0)]
    // TODO: This error should be move into HeaderError or TransactionError
    InvalidHeaderDep(Byte32),
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        error.context(ErrorKind::OutPoint).into()
    }
}
