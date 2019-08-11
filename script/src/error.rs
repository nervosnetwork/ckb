use ckb_error::{Error, ErrorKind};
use failure::{format_err, Error as FailureError, Fail};
use std::convert::TryFrom;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum ScriptError {
    /// TODO: Remove this error
    /// This error should never be occured
    // NOTE: the original name is NoScript
    #[fail(display = "MissingScript")]
    MissingScript,

    /// The field code_hash in script is invalid
    #[fail(display = "InvalidCodeHash")]
    InvalidCodeHash,

    /// The script consumes too much cycles
    // NOTE: the original name is ExceededMaximumCycles,
    #[fail(display = "TooMuchCycles")]
    TooMuchCycles,

    /// `script.type_hash` hits multiple cells with different data
    #[fail(display = "MultipleMatches")]
    MultipleMatches,

    /// Non-zero exit code returns by script
    #[fail(display = "ValidationFailure({})", _0)]
    ValidationFailure(i8),
}

impl<'a> TryFrom<&'a Error> for &'a ScriptError {
    type Error = FailureError;
    fn try_from(value: &'a Error) -> Result<Self, Self::Error> {
        value
            .downcast_ref::<ScriptError>()
            .ok_or_else(|| format_err!("failed to downcast ckb_error::Error to HeaderError"))
    }
}

impl From<ScriptError> for Error {
    fn from(error: ScriptError) -> Self {
        error.context(ErrorKind::Script).into()
    }
}
