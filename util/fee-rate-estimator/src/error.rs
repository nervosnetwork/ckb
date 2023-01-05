use std::{fmt, result};
use thiserror::Error;

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("error: {0}")]
    Other(String),
}

pub(crate) type Result<T> = result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid_params<T: fmt::Display>(inner: T) -> Self {
        Self::InvalidParams(inner.to_string())
    }
    pub(crate) fn other<T: fmt::Display>(inner: T) -> Self {
        Self::Other(inner.to_string())
    }
}
