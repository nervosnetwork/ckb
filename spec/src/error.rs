use ckb_error::{Error, ErrorKind};
use failure::{format_err, Error as FailureError, Fail};
use numext_fixed_hash::H256;
use std::convert::TryFrom;

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
        display = "UnmatchedGenesis{{expected: {:#x}, actual: {:#x}}}",
        expected, actual
    )]
    UnmatchedGenesis { expected: H256, actual: H256 },
}

impl<'a> TryFrom<&'a Error> for &'a SpecError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<SpecError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to SpecError"))
    }
}

impl From<SpecError> for Error {
    fn from(error: SpecError) -> Self {
        error.context(ErrorKind::Spec).into()
    }
}
