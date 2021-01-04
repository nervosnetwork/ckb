//! TODO(doc): @keroro520

use crate::generated::packed::{Byte32, OutPoint};
use ckb_error::{prelude::*, Error, ErrorKind};

/// TODO(doc): @keroro520
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The specified cell is already dead
    #[error("Dead({0:?})")]
    Dead(OutPoint),

    /// The specified cells is unknown in the chain
    #[error("Unknown({0:?})")]
    Unknown(Vec<OutPoint>),

    /// Input or dep cell reference to a newer cell in the same block
    // TODO: Maybe replace with `UnknownInputCell`?
    #[error("OutOfOrder({0:?})")]
    OutOfOrder(OutPoint),

    /// The output is referenced as a dep-group output, but the data
    /// is invalid format
    #[error("InvalidDepGroup({0:?})")]
    InvalidDepGroup(OutPoint),

    // TODO: This error should be move into HeaderError or TransactionError
    /// TODO(doc): @keroro520
    #[error("InvalidHeader({0})")]
    InvalidHeader(Byte32),

    // TODO: This error should be move into HeaderError or TransactionError
    /// TODO(doc): @keroro520
    #[error("ImmatureHeader({0})")]
    ImmatureHeader(Byte32),
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        ErrorKind::OutPoint.because(error)
    }
}
