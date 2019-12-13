use ckb_error::{Error, ErrorKind};
use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum SubmitTxError {
    /// The fee rate of transaction is lower than min fee rate
    #[fail(display = "LowFeeRate")]
    LowFeeRate(u64),
    #[fail(display = "ExceededMaximumAncestorsCount")]
    ExceededMaximumAncestorsCount,
}

impl From<SubmitTxError> for Error {
    fn from(error: SubmitTxError) -> Self {
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
