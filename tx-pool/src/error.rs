use ckb_error::{Error, ErrorKind};
use ckb_types::packed::Byte32;
use failure::Fail;
use tokio::sync::mpsc::error::TrySendError as TokioTrySendError;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Reject {
    /// The fee rate of transaction is lower than min fee rate
    #[fail(
        display = "Transaction fee rate must >= {} shannons/KB, got: {}",
        _0, _1
    )]
    LowFeeRate(u64, u64),

    #[fail(display = "Transaction exceeded maximum ancestors count limit, try send it later")]
    ExceededMaximumAncestorsCount,

    #[fail(
        display = "Transaction pool exceeded maximum {} limit({}), try send it later",
        _0, _1
    )]
    Full(String, u64),

    #[fail(display = "Transaction({}) already exist in transaction_pool", _0)]
    Duplicated(Byte32),

    #[fail(display = "Malformed {} transaction", _0)]
    Malformed(String),
}

impl From<Reject> for Error {
    fn from(error: Reject) -> Self {
        error.context(ErrorKind::SubmitTransaction).into()
    }
}

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum BlockAssemblerError {
    #[fail(display = "InvalidInput")]
    InvalidInput,
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
    #[fail(display = "Disabled")]
    Disabled,
}

#[derive(Fail, Debug)]
#[fail(display = "TrySendError {}.", _0)]
pub struct TrySendError(String);

pub fn handle_try_send_error<T>(error: TokioTrySendError<T>) -> (T, TrySendError) {
    let e = TrySendError(format!("{}", error));
    let m = match error {
        TokioTrySendError::Full(t) | TokioTrySendError::Closed(t) => t,
    };
    (m, e)
}
