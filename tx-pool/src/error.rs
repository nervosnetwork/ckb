//! The error type for Tx-pool operations

use ckb_error::{
    impl_error_conversion_with_adaptor, impl_error_conversion_with_kind, prelude::*, Error,
    ErrorKind, InternalError, InternalErrorKind, OtherError,
};
use ckb_fee_estimator::FeeRate;
use ckb_types::packed::Byte32;
use tokio::sync::{mpsc::error::TrySendError, oneshot::error::RecvError};

/// TX reject message
#[derive(Error, Debug, PartialEq, Clone, Eq)]
pub enum Reject {
    /// Transaction fee lower than config
    #[error("The min fee rate is {0} shannons/KB, so the transaction fee should be {1} shannons at least, but only got {2}")]
    LowFeeRate(FeeRate, u64, u64),

    /// Transaction exceeded maximum ancestors count limit
    #[error("Transaction exceeded maximum ancestors count limit, try send it later")]
    ExceededMaximumAncestorsCount,

    /// Transaction pool exceeded maximum size or cycles limit,
    #[error("Transaction pool exceeded maximum {0} limit({1}), try send it later")]
    Full(String, u64),

    /// Transaction already exist in transaction_pool
    #[error("Transaction({0}) already exist in transaction_pool")]
    Duplicated(Byte32),

    /// Malformed transaction
    #[error("Malformed {0} transaction")]
    Malformed(String),
}

impl_error_conversion_with_kind!(Reject, ErrorKind::SubmitTransaction, Error);

/// The error type for block assemble related
#[derive(Error, Debug, PartialEq, Clone, Eq)]
pub enum BlockAssemblerError {
    /// Input is invalid
    #[error("InvalidInput")]
    InvalidInput,
    /// Parameters is invalid
    #[error("InvalidParams {0}")]
    InvalidParams(String),
    /// BlockAssembler is disabled
    #[error("Disabled")]
    Disabled,
}

impl_error_conversion_with_kind!(
    BlockAssemblerError,
    InternalErrorKind::BlockAssembler,
    InternalError
);
impl_error_conversion_with_adaptor!(BlockAssemblerError, InternalError, Error);

pub(crate) fn handle_try_send_error<T>(error: TrySendError<T>) -> (T, OtherError) {
    let e = OtherError::new(format!("TrySendError {}", error));
    let m = match error {
        TrySendError::Full(t) | TrySendError::Closed(t) => t,
    };
    (m, e)
}

pub(crate) fn handle_recv_error(error: RecvError) -> OtherError {
    OtherError::new(format!("RecvError {}", error))
}
