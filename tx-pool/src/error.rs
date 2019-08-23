use ckb_types::core::cell::UnresolvableError;
use ckb_verification::TransactionError;
use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum BlockAssemblerError {
    #[fail(display = "InvalidInput")]
    InvalidInput,
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
    #[fail(display = "Disabled")]
    Disabled,
}

// TODO document this enum more accurately
/// Enum of errors
#[derive(Debug, Clone, PartialEq, Fail)]
pub enum PoolError {
    /// Unresolvable CellStatus
    #[fail(display = "UnresolvableTransaction {:?}", _0)]
    UnresolvableTransaction(UnresolvableError),
    /// An invalid pool entry caused by underlying tx validation error
    #[fail(display = "InvalidTx {}", _0)]
    InvalidTx(TransactionError),
    /// Transaction pool reach limit, can't accept more transactions
    #[fail(display = "LimitReached")]
    LimitReached,
    /// TimeOut
    #[fail(display = "TimeOut")]
    TimeOut,
    /// BlockNumber is not right
    #[fail(display = "InvalidBlockNumber")]
    InvalidBlockNumber,
    /// Duplicate tx
    #[fail(display = "Tx Duplicate")]
    Duplicate,
    /// tx fee
    #[fail(display = "TxFee {}", _0)]
    TxFee(String),
}

impl PoolError {
    /// Transaction error may be caused by different tip between peers if this method return false,
    /// Otherwise we consider the Bad Tx is constructed intendedly.
    pub fn is_bad_tx(&self) -> bool {
        match self {
            PoolError::InvalidTx(err) => err.is_bad_tx(),
            _ => false,
        }
    }
}
