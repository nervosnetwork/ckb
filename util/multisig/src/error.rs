//! Multi-signature error.
use failure::Context;

/// Multi-signature error.
#[derive(Debug)]
pub struct Error {
    inner: Context<ErrorKind>,
}

/// Multi-signature error kinds.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Fail)]
pub enum ErrorKind {
    /// The count of signatures should be less than the count of private keys.
    #[fail(display = "The count of sigs should less than pks.")]
    SigCountOverflow,
    /// The count of signatures is less than the threshold.
    #[fail(display = "The count of sigs less than threshold.")]
    SigNotEnough,
    /// The verified signatures count is less than the threshold.
    #[fail(display = "Failed to meet threshold {:?}.", _0)]
    Threshold {
        /// The required count of valid signatures.
        threshold: usize,
        /// The actual count of valid signatures.
        pass_sigs: usize,
    },
}

impl Error {
    /// Gets the error kind.
    pub fn kind(&self) -> ErrorKind {
        *self.inner.get_context()
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Error {
        Error {
            inner: Context::new(kind),
        }
    }
}

impl From<Context<ErrorKind>> for Error {
    fn from(inner: Context<ErrorKind>) -> Error {
        Error { inner }
    }
}
