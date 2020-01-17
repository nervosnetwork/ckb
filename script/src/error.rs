use ckb_error::{Error, ErrorKind};
use failure::Fail;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum ScriptError {
    /// The field code_hash in script is invalid
    #[fail(display = "InvalidCodeHash")]
    InvalidCodeHash,

    /// The script consumes too much cycles
    #[fail(display = "ExceededMaximumCycles")]
    ExceededMaximumCycles,

    /// `script.type_hash` hits multiple cells with different data
    #[fail(display = "MultipleMatches")]
    MultipleMatches,

    /// Non-zero exit code returns by script
    #[fail(display = "ValidationFailure({})", _0)]
    ValidationFailure(i8),

    /// Known bugs are detected in transaction script outputs
    #[fail(display = "EncounteredKnownBugs({})", _0)]
    EncounteredKnownBugs(String),
}

impl From<ScriptError> for Error {
    fn from(error: ScriptError) -> Self {
        error.context(ErrorKind::Script).into()
    }
}
