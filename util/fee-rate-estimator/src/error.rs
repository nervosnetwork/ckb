//! FeeEstimator error Definition

use std::{fmt, result};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("error: {0}")]
    Internal(String),
}

pub type Result<T> = result::Result<T, Error>;

impl Error {
    pub(crate) fn invalid_params<T: fmt::Display>(inner: T) -> Self {
        Self::InvalidParams(inner.to_string())
    }
    pub(crate) fn internal<T: fmt::Display>(inner: T) -> Self {
        Self::Internal(inner.to_string())
    }

    /// Return whether internal error，indicating that the service is temporarily unavailable
    pub fn is_internal(&self) -> bool {
        matches!(self, Error::Internal(_))
    }

    /// Return error description
    pub fn description(&self) -> &str {
        match self {
            Error::InvalidParams(desc) => desc,
            Error::Internal(desc) => desc,
        }
    }
}
