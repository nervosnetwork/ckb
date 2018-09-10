use serde_json;
use std::error::Error as StdError;
use std::fmt;

type Cause = Box<StdError + Send + Sync>;

pub struct Error {
    inner: Box<ErrorImpl>,
}

#[derive(Debug, PartialEq, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

struct ErrorImpl {
    kind: Kind,
    cause: Cause,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Kind {
    Future,
    Hyper,
    Parse,
    JsonRpc,
}

impl Error {
    pub(crate) fn new(kind: Kind, cause: Cause) -> Error {
        Error {
            inner: Box::new(ErrorImpl { kind, cause }),
        }
    }

    pub(crate) fn new_future<E: Into<Cause>>(cause: E) -> Error {
        Error::new(Kind::Future, cause.into())
    }

    pub(crate) fn new_hyper<E: Into<Cause>>(cause: E) -> Error {
        Error::new(Kind::Hyper, cause.into())
    }

    pub(crate) fn new_parse<E: Into<Cause>>(cause: E) -> Error {
        Error::new(Kind::Parse, cause.into())
    }

    pub(crate) fn new_jsonrpc<E: Into<Cause>>(cause: E) -> Error {
        Error::new(Kind::JsonRpc, cause.into())
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Error")
            .field("kind", &self.inner.kind)
            .field("cause", &self.inner.cause.description())
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}", self.description(), &self.inner.cause)
    }
}

impl StdError for Error {
    fn description(&self) -> &str {
        match self.inner.kind {
            Kind::Future => "future handle error",
            Kind::Hyper => "http request error",
            Kind::Parse => "data parse error",
            Kind::JsonRpc => "JsonRpc parse error",
        }
    }

    fn cause(&self) -> Option<&StdError> {
        Some(self.inner.cause.as_ref())
    }
}

impl StdError for JsonRpcError {
    fn description(&self) -> &str {
        &self.message
    }

    fn cause(&self) -> Option<&StdError> {
        None
    }
}

impl fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}
