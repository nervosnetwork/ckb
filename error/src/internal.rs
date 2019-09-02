use crate::{Error, ErrorKind};
use failure::{format_err, Error as FailureError, Fail};
use std::convert::TryFrom;
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

impl From<InternalError> for Error {
    fn from(error: InternalError) -> Self {
        error.context(ErrorKind::Internal).into()
    }
}

impl<'a> TryFrom<&'a Error> for &'a InternalError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<InternalError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to InternalError"))
    }
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

impl InternalError {
    pub fn new<S: ToString>(kind: InternalErrorKind, cause: S) -> Self {
        Self {
            kind,
            cause: Some(cause.to_string()),
        }
    }

    pub fn kind(&self) -> &InternalErrorKind {
        &self.kind
    }
}
