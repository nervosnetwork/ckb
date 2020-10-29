use crate::{Error, ErrorKind};
use failure::{err_msg, Backtrace, Context, Fail};
use std::fmt::{self, Debug, Display};

/// TODO(doc): @keroro520
#[derive(Debug)]
pub struct InternalError {
    kind: Context<InternalErrorKind>,
}

/// TODO(doc): @keroro520
#[derive(Debug, PartialEq, Eq, Clone, Display)]
pub enum InternalErrorKind {
    /// An arithmetic overflow occurs during capacity calculation,
    /// e.g. `Capacity::safe_add`
    CapacityOverflow,

    /// Persistent data had corrupted
    DataCorrupted,

    /// Database exception
    Database,

    /// VM internal error
    VM,

    /// Unknown system error
    System,

    /// The feature is disabled or is conflicted with the configuration
    Config,
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
        InternalError {
            kind: Context::new(kind),
        }
    }
}

impl From<InternalErrorKind> for Error {
    fn from(kind: InternalErrorKind) -> Self {
        Into::<InternalError>::into(kind).into()
    }
}

impl InternalErrorKind {
    /// TODO(doc): @keroro520
    pub fn cause<F: Fail>(self, cause: F) -> InternalError {
        InternalError {
            kind: cause.context(self),
        }
    }

    /// TODO(doc): @keroro520
    pub fn reason<S: Display + Debug + Sync + Send + 'static>(self, reason: S) -> InternalError {
        InternalError {
            kind: err_msg(reason).compat().context(self),
        }
    }
}

impl InternalError {
    /// TODO(doc): @keroro520
    pub fn kind(&self) -> &InternalErrorKind {
        &self.kind.get_context()
    }
}

impl Fail for InternalError {
    fn cause(&self) -> Option<&dyn Fail> {
        self.kind.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        self.kind.backtrace()
    }
}
