use ckb_error::{Error, ErrorKind};
use ckb_types::packed::Byte32;
use failure::Fail;

#[derive(Fail, Debug, Clone, Eq, PartialEq)]
pub enum SpecError {
    // NOTE: the original name is FileNotFound
    #[fail(display = "NotFoundFile")]
    NotFoundFile(String),

    // NOTE: the original name is ChainNameNotAllowed
    #[fail(display = "NotAllowedChainName: {}", _0)]
    NotAllowedChainName(String),

    // NOTE: the original name GenesisMismatch
    #[fail(
        display = "UnmatchedGenesis{{expected: {}, actual: {}}}",
        expected, actual
    )]
    UnmatchedGenesis { expected: Byte32, actual: Byte32 },
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        error.context(ErrorKind::Spec).into()
    }
}
