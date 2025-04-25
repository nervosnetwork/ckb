use std::{fmt, time::Duration};

pub(crate) const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(60 * 60 * 24);

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

    /// Generic rate limit error
    TooManyRequests = 110,

    /// The max TTL is larger than the limit.
    InvalidMaxTTL = 410,
    /// The peer id of `from` peer is invalid.
    InvalidFromPeerId = 411,
    /// The peer id of `to` peer is invalid.
    InvalidToPeerId = 412,
    /// At least one peer id in route is invalid.
    InvalidRoute = 413,
    /// The listen address len is invalid.
    InvalidListenAddrLen = 414,

    /// Ignore messages.
    Ignore = 501,
    /// Failed to broadcast a message.
    BroadcastError = 502,
    /// Failed to broadcast a message.
    ForwardError = 503,
    /// The max TTL is reached.
    ReachedMaxTTL = 504,
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
        let code = self.code() as u16;
        if (400..500).contains(&code) {
            Some(BAD_MESSAGE_BAN_TIME)
        } else {
            None
        }
    }

    /// Whether a warning log should be output.
    pub fn should_warn(&self) -> bool {
        let code = self.code() as u16;
        (500..600).contains(&code)
    }

    /// Returns the status code.
    pub fn code(&self) -> StatusCode {
        self.code
    }
}
