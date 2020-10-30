//! TODO(doc): @zhangsoledad
use ckb_error::{Error, ErrorKind};
use ckb_types::packed::Byte32;
use failure::Fail;
use tokio::sync::mpsc::error::TrySendError as TokioTrySendError;

/// TODO(doc): @zhangsoledad
#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Reject {
    /// The fee rate of transaction is lower than min fee rate
    #[fail(
        display = "Transaction fee rate must >= {} shannons/KB, got: {}",
        _0, _1
    )]
    LowFeeRate(u64, u64),

    /// TODO(doc): @zhangsoledad
    #[fail(display = "Transaction exceeded maximum ancestors count limit, try send it later")]
    ExceededMaximumAncestorsCount,

    /// TODO(doc): @zhangsoledad
    #[fail(
        display = "Transaction pool exceeded maximum {} limit({}), try send it later",
        _0, _1
    )]
    Full(String, u64),

    /// TODO(doc): @zhangsoledad
    #[fail(display = "Transaction({}) already exist in transaction_pool", _0)]
    Duplicated(Byte32),

    /// TODO(doc): @zhangsoledad
    #[fail(display = "Malformed {} transaction", _0)]
    Malformed(String),
}

impl From<Reject> for Error {
    fn from(error: Reject) -> Self {
        error.context(ErrorKind::SubmitTransaction).into()
    }
}

/// TODO(doc): @zhangsoledad
#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum BlockAssemblerError {
    /// TODO(doc): @zhangsoledad
    #[fail(display = "InvalidInput")]
    InvalidInput,
    /// TODO(doc): @zhangsoledad
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
    /// TODO(doc): @zhangsoledad
    #[fail(display = "Disabled")]
    Disabled,
}

/// TODO(doc): @zhangsoledad
#[derive(Fail, Debug)]
#[fail(display = "TrySendError {}.", _0)]
pub struct TrySendError(String);

/// TODO(doc): @zhangsoledad
pub fn handle_try_send_error<T>(error: TokioTrySendError<T>) -> (T, TrySendError) {
    let e = TrySendError(format!("{}", error));
    let m = match error {
        TokioTrySendError::Full(t) | TokioTrySendError::Closed(t) => t,
    };
    (m, e)
}
