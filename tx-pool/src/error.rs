//! The error type for Tx-pool operations

use ckb_error::{Error, ErrorKind};
use ckb_fee_estimator::FeeRate;
use ckb_types::packed::Byte32;
use failure::Fail;
use tokio::sync::mpsc::error::TrySendError as TokioTrySendError;

/// TX reject message
#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Reject {
    /// Transaction fee lower than config
    #[fail(
        display = "The min fee rate is {} shannons/KB, so the transaction fee should be {} shannons at least, but only got {}",
        _0, _1, _2
    )]
    LowFeeRate(FeeRate, u64, u64),

    /// Transaction exceeded maximum ancestors count limit
    #[fail(display = "Transaction exceeded maximum ancestors count limit, try send it later")]
    ExceededMaximumAncestorsCount,

    /// Transaction pool exceeded maximum size or cycles limit,
    #[fail(
        display = "Transaction pool exceeded maximum {} limit({}), try send it later",
        _0, _1
    )]
    Full(String, u64),

    /// Transaction already exist in transaction_pool
    #[fail(display = "Transaction({}) already exist in transaction_pool", _0)]
    Duplicated(Byte32),

    /// Malformed transaction
    #[fail(display = "Malformed {} transaction", _0)]
    Malformed(String),
}

impl From<Reject> for Error {
    fn from(error: Reject) -> Self {
        error.context(ErrorKind::SubmitTransaction).into()
    }
}

/// The error type for block assemble related
#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum BlockAssemblerError {
    /// Input is invalid
    #[fail(display = "InvalidInput")]
    InvalidInput,
    /// Parameters is invalid
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
    /// BlockAssembler is disabled
    #[fail(display = "Disabled")]
    Disabled,
}

#[derive(Fail, Debug)]
#[fail(display = "TrySendError {}.", _0)]
pub(crate) struct TrySendError(String);

pub(crate) fn handle_try_send_error<T>(error: TokioTrySendError<T>) -> (T, TrySendError) {
    let e = TrySendError(format!("{}", error));
    let m = match error {
        TokioTrySendError::Full(t) | TokioTrySendError::Closed(t) => t,
    };
    (m, e)
}
