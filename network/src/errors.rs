use std::error;
use std::fmt;
use std::fmt::Display;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};

#[derive(Debug)]
pub struct Error {
    pub error_kind: ErrorKind,
}

#[derive(Debug)]
pub enum ErrorKind {
    PeerNotFound,
    InvalidNewPeer(String),
    ParseAddress,
    BadProtocol,
    TimerRegisterNotAvailable,
    Io(IoError),
    Other(String),
}

impl From<ErrorKind> for Error {
    fn from(e: ErrorKind) -> Error {
        Error { error_kind: e }
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Error {
            error_kind: ErrorKind::Io(err),
        }
    }
}

impl Into<IoError> for Error {
    fn into(self: Error) -> IoError {
        IoError::new(IoErrorKind::Other, self)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        "general error in libp2p"
    }

    fn cause(&self) -> Option<&error::Error> {
        // Generic error, underlying cause isn't tracked.
        None
    }
}
