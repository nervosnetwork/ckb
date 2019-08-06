use failure::Fail;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum ScriptError {
    /// TODO: Remove this error
    /// This error should never be occured
    // NOTE: the original name is NoScript
    #[fail(display = "Missing script")]
    MissingScript,

    /// The field code_hash in script is invalid
    #[fail(display = "Invalid code-hash")]
    InvalidCodeHash,

    /// The script consumes too much cycles
    // NOTE: the original name is ExceededMaximumCycles,
    #[fail(display = "Too much cycles")]
    TooMuchCycles,

    /// `script.type_hash` hits multiple cells with different data
    #[fail(display = "Multiple matches")]
    MultipleMatches,

    /// Non-zero exit code returns by script
    #[fail(display = "ValidationFailure({})", _0)]
    ValidationFailure(i8),
}
