use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum Error {
    #[error("illegal character")]
    IllegalCharacter,
    #[error("unmatched quotes")]
    UnmatchedQuotes,
    #[error("invalid escape")]
    InvalidEscape,

    #[error("insufficient arguments")]
    InsufficientArguments,
    #[error("too many arguments")]
    TooManyArguments,
    #[error("bad argument `{0}`")]
    BadArgument(String),

    #[error("unknown command `{0}`")]
    UnsupportedCommand(String),
    #[error("unknown sub-command `{0}`")]
    UnknownSubCommand(String),

    #[error("failed to send request, since {0}")]
    SendRequest(String),
    #[error("failed to receive response, since {0}")]
    RecvResponse(String),

    #[error("{0}")]
    Custom(String),
}
