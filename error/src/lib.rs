#[macro_use]
extern crate enum_display_derive;

mod convert;
mod internal;
pub mod util;

use failure::{Backtrace, Context, Fail};
pub use internal::{InternalError, InternalErrorKind};
use std::fmt::{self, Display};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum ErrorKind {
    OutPoint,
    Transaction,
    SubmitTransaction,
    Script,
    Header,
    Block,
    Internal,
    Dao,
    Spec,
}

#[derive(Debug)]
pub struct Error {
    kind: Context<ErrorKind>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(cause) = self.cause() {
            if f.alternate() {
                write!(f, "{}: {}", self.kind(), cause)
            } else {
                write!(f, "{}({})", self.kind(), cause)
            }
        } else {
            write!(f, "{}", self.kind())
        }
    }
}

impl From<Context<ErrorKind>> for Error {
    fn from(inner: Context<ErrorKind>) -> Self {
        Self { kind: inner }
    }
}

impl Fail for Error {
    fn cause(&self) -> Option<&dyn Fail> {
        self.kind.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.kind.backtrace()
    }
}

impl Error {
    pub fn kind(&self) -> &ErrorKind {
        self.kind.get_context()
    }

    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    pub fn unwrap_cause_or_self(&self) -> &dyn Fail {
        self.cause().unwrap_or(self)
    }
}
