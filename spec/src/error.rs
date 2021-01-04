use ckb_error::{prelude::*, Error, ErrorKind};
use ckb_types::packed::Byte32;

/// The error type for Spec operations
#[derive(Error, Debug, Clone, Eq, PartialEq)]
pub enum SpecError {
    /// The file not found
    #[error("FileNotFound")]
    FileNotFound(String),

    /// The specified chain name is reserved.
    #[error("ChainNameNotAllowed: {0}")]
    ChainNameNotAllowed(String),

    /// The actual calculated genesis hash is not match with provided
    #[error("GenesisMismatch(expected: {expected}, actual: {actual})")]
    GenesisMismatch {
        /// The provided expected hash
        expected: Byte32,
        /// The actual calculated hash
        actual: Byte32,
    },
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        ErrorKind::Spec.because(error)
    }
}
