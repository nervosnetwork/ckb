use ckb_error::{Error, ErrorKind};
use ckb_types::packed::Byte32;
use failure::Fail;

#[derive(Fail, Debug, Clone, Eq, PartialEq)]
pub enum SpecError {
    #[fail(display = "FileNotFound")]
    FileNotFound(String),

    #[fail(display = "ChainNameNotAllowed: {}", _0)]
    ChainNameNotAllowed(String),

    #[fail(
        display = "GenesisMismatch(expected: {}, actual: {})",
        expected, actual
    )]
    GenesisMismatch { expected: Byte32, actual: Byte32 },
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        error.context(ErrorKind::Spec).into()
    }
}
