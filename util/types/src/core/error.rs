//! TODO(doc): @keroro520

use crate::generated::packed::{Byte32, OutPoint};
use ckb_error::{Error, ErrorKind};
use failure::Fail;

/// TODO(doc): @keroro520
#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The specified cell is already dead
    #[fail(display = "Dead({:?})", _0)]
    Dead(OutPoint),

    /// The specified cells is unknown in the chain
    #[fail(display = "Unknown({:?})", _0)]
    Unknown(Vec<OutPoint>),

    /// Input or dep cell reference to a newer cell in the same block
    // TODO: Maybe replace with `UnknownInputCell`?
    #[fail(display = "OutOfOrder({:?})", _0)]
    OutOfOrder(OutPoint),

    /// The output is referenced as a dep-group output, but the data
    /// is invalid format
    #[fail(display = "InvalidDepGroup({:?})", _0)]
    InvalidDepGroup(OutPoint),

    // TODO: This error should be move into HeaderError or TransactionError
    /// TODO(doc): @keroro520
    #[fail(display = "InvalidHeader({})", _0)]
    InvalidHeader(Byte32),

    // TODO: This error should be move into HeaderError or TransactionError
    /// TODO(doc): @keroro520
    #[fail(display = "ImmatureHeader({})", _0)]
    ImmatureHeader(Byte32),
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        error.context(ErrorKind::OutPoint).into()
    }
}
