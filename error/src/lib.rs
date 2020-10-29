//! TODO(doc): @keroro520
#[macro_use]
extern crate enum_display_derive;

mod convert;
mod internal;
pub mod util;

use failure::{Backtrace, Context, Fail};
pub use internal::{InternalError, InternalErrorKind};
use std::fmt::{self, Display};

/// TODO(doc): @keroro520
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum ErrorKind {
    /// TODO(doc): @keroro520
    OutPoint,
    /// TODO(doc): @keroro520
    Transaction,
    /// TODO(doc): @keroro520
    SubmitTransaction,
    /// TODO(doc): @keroro520
    Script,
    /// TODO(doc): @keroro520
    Header,
    /// TODO(doc): @keroro520
    Block,
    /// TODO(doc): @keroro520
    Internal,
    /// TODO(doc): @keroro520
    Dao,
    /// TODO(doc): @keroro520
    Spec,
}

/// TODO(doc): @keroro520
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
    /// TODO(doc): @keroro520
    pub fn kind(&self) -> &ErrorKind {
        self.kind.get_context()
    }

    /// TODO(doc): @keroro520
    pub fn downcast_ref<T: Fail>(&self) -> Option<&T> {
        self.cause().and_then(|cause| cause.downcast_ref::<T>())
    }

    /// TODO(doc): @keroro520
    pub fn unwrap_cause_or_self(&self) -> &dyn Fail {
        self.cause().unwrap_or(self)
    }
}
