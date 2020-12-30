//! The error types to unexpected out-points.

use crate::generated::packed::{Byte32, OutPoint};
use ckb_error::{prelude::*, Error, ErrorKind};

/// Errors due to the fact that the out-point rules are not respected.
#[derive(Error, Debug, PartialEq, Eq, Clone)]
pub enum OutPointError {
    /// The target cell was already dead.
    #[error("Dead({0:?})")]
    Dead(OutPoint),

    /// There are cells which is unknown to the canonical chain.
    #[error("Unknown({0:?})")]
    Unknown(Vec<OutPoint>),

    /// There is an input out-point or dependency out-point which references a newer cell in the
    /// same block.
    #[error("OutOfOrder({0:?})")]
    OutOfOrder(OutPoint),

    /// There is a dependency out-point, which is [`DepGroup`], but its output-data is invalid
    /// format. The expected output-data format for [`DepGroup`] is [`OutPointVec`].
    ///
    /// [`DepGroup`]: ../enum.DepType.html#variant.DepGroup
    /// [`OutPointVec`]: ../../packed/struct.OutPointVec.html
    #[error("InvalidDepGroup({0:?})")]
    InvalidDepGroup(OutPoint),

    /// There is a dependency header that is unknown to the canonical chain.
    #[error("InvalidHeader({0})")]
    InvalidHeader(Byte32),

    /// There is a dependency header that is immature yet.
    #[error("ImmatureHeader({0})")]
    ImmatureHeader(Byte32),
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        ErrorKind::OutPoint.because(error)
    }
}
