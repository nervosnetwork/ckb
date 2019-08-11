use crate::transaction::OutPoint;
use ckb_error::{Error, ErrorKind};
use failure::{format_err, Error as FailureError, Fail};
use std::convert::TryFrom;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The specified cell is already dead
    // NOTE: the original name is Dead
    #[fail(display = "DeadCell({:?})", _0)]
    DeadCell(OutPoint),

    /// The specified cells is unknown in the chain
    // NOTE: the original name is Unknown
    #[fail(display = "UnknownCells({:?})", _0)]
    UnknownCells(Vec<OutPoint>),

    /// The specified input cell is not-found inside the specified header
    // NOTE: the original name is InvalidHeader
    #[fail(display = "ExclusiveInputCell({:?})", _0)]
    ExclusiveInputCell(OutPoint),

    /// Use the out point as input but not specified the input cell
    // NOTE: the original name is UnspecifiedInputCell
    #[fail(display = "MissingInputCell({:?})", _0)]
    MissingInputCell(OutPoint),

    /// Empty out point, missing the input cell and header
    // NOTE: the original name is Empty
    #[fail(display = "MissingInputCellAndHeader({:?})", _0)]
    MissingInputCellAndHeader(OutPoint),

    /// Unknown the specified header
    #[fail(display = "UnknownHeader({:?})", _0)]
    UnknownHeader(OutPoint),

    /// Input or dep cell reference to a newer cell in the same block
    // NOTE: Maybe replace with `UnknownInputCell`?
    #[fail(display = "OutOfOrder({:?})", _0)]
    OutOfOrder(OutPoint),
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        error.context(ErrorKind::OutPoint).into()
    }
}

impl<'a> TryFrom<&'a Error> for &'a OutPointError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<OutPointError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to OutPointError"))
    }
}
