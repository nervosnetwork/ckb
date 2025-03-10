use std::error::Error;
use std::fmt::{self, Debug, Display};

/// Define an enumeration to represent possible error types for the IPC system.
#[derive(Debug, Clone)]
pub enum IpcError {
    /// Incomplete VLQ sequence.
    IncompleteVlqSeq,
    /// Decode VLQ overflow.
    DecodeVlqOverflow,
    /// Read VLQ error.
    ReadVlqError,
    /// Read exact error.
    ReadExactError,
}

impl Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl Error for IpcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}
