use crate::{Error, ErrorKind};
use failure::Fail;
use std::fmt::{self, Display};

#[derive(Fail, Debug)]
pub struct InternalError {
    kind: InternalErrorKind,
    cause: Option<String>,
}

#[derive(Fail, Debug, PartialEq, Eq, Clone, Display)]
pub enum InternalErrorKind {
    /// An arithmetic overflow occurs during capacity calculation,
    /// e.g. `Capacity::safe_add`
    // NOTE: the original name is {Transaction,Block}::CapacityOverflow
    ArithmeticOverflowCapacity,

    /// The transaction_pool is already full
    // NOTE: the original name is LimitReached
    FullTransactionPool,

    /// Persistent data had corrupted
    CorruptedData,

    /// Database exception
    // NOTE: the original name is ckb_db::Error::DBError(String)
    Database,

    /// VM internal error
    VM,

    /// The transaction already exist in pool
    DuplicatedPoolTransaction,

    /// Unknown system error
    System,
}

impl fmt::Display for InternalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(cause) = self.cause() {
            write!(f, "{}({})", self.kind(), cause)
        } else {
            write!(f, "{}", self.kind())
        }
    }
}

impl From<InternalError> for Error {
    fn from(error: InternalError) -> Self {
        error.context(ErrorKind::Internal).into()
    }
}

impl From<InternalErrorKind> for InternalError {
    fn from(kind: InternalErrorKind) -> Self {
        Self { kind, cause: None }
    }
}

impl From<InternalErrorKind> for Error {
    fn from(kind: InternalErrorKind) -> Self {
        InternalError { kind, cause: None }.into()
    }
}

impl InternalErrorKind {
    pub fn cause<S: ToString>(self, reason: S) -> InternalError {
        InternalError {
            kind: self,
            cause: Some(reason.to_string()),
        }
    }
}

impl InternalError {
    pub fn kind(&self) -> &InternalErrorKind {
        &self.kind
    }
}
