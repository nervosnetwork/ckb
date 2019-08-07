use ckb_occupied_capacity::Error as CapacityError;
use failure::{Backtrace, Context, Fail};
use std::fmt::{self, Display, Formatter};

mod block;
mod dao;
mod header;
mod internal;
mod out_point;
mod script;
mod spec;
mod transaction;

pub use block::{BlockError, CellbaseError, CommitError, UnclesError};
pub use dao::DaoError;
pub use header::{EpochError, HeaderError, NumberError, PowError, TimestampError};
pub use internal::InternalError;
pub use out_point::{CellOutPoint, OutPoint, OutPointError};
pub use script::ScriptError;
pub use spec::SpecError;
pub use transaction::TransactionError;

#[derive(Fail, Debug, Clone, Eq, PartialEq)]
pub enum ErrorKind {
    #[fail(display = "OutPointError")]
    OutPoint,
    #[fail(display = "TransactionError")]
    Transaction,
    #[fail(display = "ScriptError")]
    Script,
    #[fail(display = "HeaderError")]
    Header,
    #[fail(display = "BlockError")]
    Block,
    #[fail(display = "InternalError")]
    Internal,
    #[fail(display = "DaoError")]
    Dao,
    #[fail(display = "SpecError")]
    Spec,
}

#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(
            f,
            "{}({})",
            self.kind(),
            self.cause().expect("inner cause exist")
        )
    }
}

impl Clone for Error {
    fn clone(&self) -> Self {
        match self.kind() {
            ErrorKind::OutPoint => self.downcast_ref::<OutPointError>().unwrap().clone().into(),
            ErrorKind::Transaction => self
                .downcast_ref::<TransactionError>()
                .unwrap()
                .clone()
                .into(),
            ErrorKind::Internal => self.downcast_ref::<InternalError>().unwrap().clone().into(),
            ErrorKind::Dao => self.downcast_ref::<DaoError>().unwrap().clone().into(),
            ErrorKind::Script => self.downcast_ref::<ScriptError>().unwrap().clone().into(),
            ErrorKind::Header => self.downcast_ref::<HeaderError>().unwrap().clone().into(),
            ErrorKind::Block => self.downcast_ref::<BlockError>().unwrap().clone().into(),
            ErrorKind::Spec => self.downcast_ref::<SpecError>().unwrap().clone().into(),
        }
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Context::new(kind).into()
    }
}

impl From<Context<ErrorKind>> for Error {
    fn from(inner: Context<ErrorKind>) -> Self {
        Self { inner }
    }
}

impl From<OutPointError> for Error {
    fn from(error: OutPointError) -> Self {
        error.context(ErrorKind::OutPoint).into()
    }
}

impl From<TransactionError> for Error {
    fn from(error: TransactionError) -> Self {
        error.context(ErrorKind::Transaction).into()
    }
}

impl From<ScriptError> for Error {
    fn from(error: ScriptError) -> Self {
        error.context(ErrorKind::Script).into()
    }
}

impl From<HeaderError> for Error {
    fn from(error: HeaderError) -> Self {
        error.context(ErrorKind::Header).into()
    }
}

impl From<BlockError> for Error {
    fn from(error: BlockError) -> Self {
        error.context(ErrorKind::Block).into()
    }
}

impl From<InternalError> for Error {
    fn from(error: InternalError) -> Self {
        error.context(ErrorKind::Internal).into()
    }
}

impl From<DaoError> for Error {
    fn from(error: DaoError) -> Self {
        error.context(ErrorKind::Dao).into()
    }
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        error.context(ErrorKind::Spec).into()
    }
}

impl From<CapacityError> for Error {
    fn from(_error: CapacityError) -> Self {
        InternalError::ArithmeticOverflowCapacity.into()
    }
}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        if self.kind() != other.kind() {
            return false;
        }

        match self.kind() {
            ErrorKind::OutPoint => {
                self.downcast_ref::<OutPointError>().unwrap()
                    == other.downcast_ref::<OutPointError>().unwrap()
            }
            ErrorKind::Transaction => {
                self.downcast_ref::<TransactionError>().unwrap()
                    == other.downcast_ref::<TransactionError>().unwrap()
            }
            ErrorKind::Internal => {
                self.downcast_ref::<InternalError>().unwrap()
                    == other.downcast_ref::<InternalError>().unwrap()
            }
            ErrorKind::Dao => {
                self.downcast_ref::<DaoError>().unwrap()
                    == other.downcast_ref::<DaoError>().unwrap()
            }
            ErrorKind::Script => {
                self.downcast_ref::<ScriptError>().unwrap()
                    == other.downcast_ref::<ScriptError>().unwrap()
            }
            ErrorKind::Header => {
                self.downcast_ref::<HeaderError>().unwrap()
                    == other.downcast_ref::<HeaderError>().unwrap()
            }
            ErrorKind::Block => {
                self.downcast_ref::<BlockError>().unwrap()
                    == other.downcast_ref::<BlockError>().unwrap()
            }
            ErrorKind::Spec => {
                self.downcast_ref::<SpecError>().unwrap()
                    == other.downcast_ref::<SpecError>().unwrap()
            }
        }
    }
}

impl Fail for Error {
    fn cause(&self) -> Option<&Fail> {
        self.inner.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.inner.backtrace()
    }
}

impl Error {
    pub fn kind(&self) -> &ErrorKind {
        self.inner.get_context()
    }

    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    /// Transaction error may be caused by different tip between peers if this
    /// method return false, Otherwise we consider the Bad Tx is constructed
    /// deliberately
    pub fn is_bad_tx(&self) -> bool {
        match self.kind() {
            ErrorKind::OutPoint => match self.downcast_ref::<OutPointError>().unwrap() {
                OutPointError::MissingInputCellAndHeader => true,
                _ => false,
            },
            ErrorKind::Transaction => match self.downcast_ref::<TransactionError>().unwrap() {
                TransactionError::DuplicatedDeps
                | TransactionError::MissingInputsOrOutputs
                | TransactionError::OutputOverflowCapacity
                | TransactionError::InvalidSinceFormat => true,
                _ => false,
            },
            ErrorKind::Internal => match self.downcast_ref::<InternalError>().unwrap() {
                InternalError::ArithmeticOverflowCapacity => true,
                _ => false,
            },
            ErrorKind::Dao => match self.downcast_ref::<DaoError>().unwrap() {
                DaoError::InvalidDaoFormat => true,
                _ => false,
            },
            ErrorKind::Script | ErrorKind::Header | ErrorKind::Block | ErrorKind::Spec => false,
        }
    }
}
