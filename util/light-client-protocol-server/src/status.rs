use std::{fmt, time::Duration};

use crate::constant;

/// StatusCodes indicate whether a specific operation has been successfully completed.
///
/// The StatusCode element is a 3-digit integer.
///
/// The first digest of the StatusCode defines the class of result:
///   - 1xx: Informational response – the request was received, continuing process.
///   - 2xx: Success - The action requested by the client was received, understood, and accepted.
///   - 4xx: Client errors - The error seems to have been caused by the client.
///   - 5xx: Server errors - The server failed to fulfil a request.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusCode {
    /// OK
    OK = 200,

    /// Malformed protocol message.
    MalformedProtocolMessage = 400,
    /// Unexpected light-client protocol message.
    UnexpectedProtocolMessage = 401,

    /// The request data is incorrect.
    InvalidRequest = 410,
    /// The last block sent from client is invalid.
    InvalidLastBlock = 411,
    /// At least one unconfirmed block sent from client is invalid.
    InvalidUnconfirmedBlock = 412,
    /// The difficulty boundary is not in the provided block range.
    InvaildDifficultyBoundary = 413,

    /// Throws an internal error.
    InternalError = 500,
    /// Throws an error from the network.
    Network = 501,
}

/// Process message status.
#[derive(Clone, Debug, Eq)]
pub struct Status {
    code: StatusCode,
    context: Option<String>,
}

impl PartialEq for Status {
    fn eq(&self, other: &Self) -> bool {
        self.code == other.code
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.context {
            Some(ref context) => write!(f, "{:?}({}): {}", self.code, self.code as u16, context),
            None => write!(f, "{:?}({})", self.code, self.code as u16),
        }
    }
}

impl From<StatusCode> for Status {
    fn from(code: StatusCode) -> Self {
        Self::new::<&str>(code, None)
    }
}

impl StatusCode {
    /// Convert a status code into a status which has a context.
    pub fn with_context<S: ToString>(self, context: S) -> Status {
        Status::new(self, Some(context))
    }
}

impl Status {
    /// Creates a new status.
    pub fn new<T: ToString>(code: StatusCode, context: Option<T>) -> Self {
        Self {
            code,
            context: context.map(|c| c.to_string()),
        }
    }

    /// Returns a `OK` status.
    pub fn ok() -> Self {
        Self::new::<&str>(StatusCode::OK, None)
    }

    /// Whether the code is `OK` or not.
    pub fn is_ok(&self) -> bool {
        self.code == StatusCode::OK
    }

    /// Whether the session should be banned.
    pub fn should_ban(&self) -> Option<Duration> {
        let code = self.code as u16;
        if !(400..500).contains(&code) {
            None
        } else {
            Some(constant::BAD_MESSAGE_BAN_TIME)
        }
    }

    /// Whether a warning log should be output.
    pub fn should_warn(&self) -> bool {
        let code = self.code as u16;
        (500..600).contains(&code)
    }

    /// Returns the status code.
    pub fn code(&self) -> StatusCode {
        self.code
    }
}
