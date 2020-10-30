use ckb_error::{Error, ErrorKind};
use ckb_types::packed::Byte32;
use failure::Fail;

/// TODO(doc): @zhangsoledad
#[derive(Fail, Debug, Clone, Eq, PartialEq)]
pub enum SpecError {
    /// TODO(doc): @zhangsoledad
    #[fail(display = "FileNotFound")]
    FileNotFound(String),

    /// TODO(doc): @zhangsoledad
    #[fail(display = "ChainNameNotAllowed: {}", _0)]
    ChainNameNotAllowed(String),

    /// TODO(doc): @zhangsoledad
    #[fail(
        display = "GenesisMismatch(expected: {}, actual: {})",
        expected, actual
    )]
    GenesisMismatch {
        /// TODO(doc): @zhangsoledad
        expected: Byte32,
        /// TODO(doc): @zhangsoledad
        actual: Byte32,
    },
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        error.context(ErrorKind::Spec).into()
    }
}
