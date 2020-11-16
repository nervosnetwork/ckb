use ckb_error::{Error, ErrorKind};
use ckb_types::packed::Byte32;
use failure::Fail;

/// The error type for Spec operations
#[derive(Fail, Debug, Clone, Eq, PartialEq)]
pub enum SpecError {
    /// The file not found
    #[fail(display = "FileNotFound")]
    FileNotFound(String),

    /// The specified chain name is reserved.
    #[fail(display = "ChainNameNotAllowed: {}", _0)]
    ChainNameNotAllowed(String),

    /// The actual calculated genesis hash is not match with provided
    #[fail(
        display = "GenesisMismatch(expected: {}, actual: {})",
        expected, actual
    )]
    GenesisMismatch {
        /// The provided expected hash
        expected: Byte32,
        /// The actual calculated hash
        actual: Byte32,
    },
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        error.context(ErrorKind::Spec).into()
    }
}
