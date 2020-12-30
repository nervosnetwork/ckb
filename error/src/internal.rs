use crate::{
    def_error_base_on_kind, impl_error_conversion_with_adaptor, impl_error_conversion_with_kind,
};
use derive_more::Display;
use std::fmt;
use thiserror::Error;

/// An error with no reason.
#[derive(Error, Debug, Clone, Copy)]
#[error("no reason is provided")]
pub struct SilentError;

/// An error with only a string as the reason.
#[derive(Error, Debug, Clone)]
#[error("{0}")]
pub struct OtherError(String);

/// A list specifying categories of ckb internal error.
///
/// This list is intended to grow over time and it is not recommended to exhaustively match against it.
///
/// It is used with the [`InternalError`].
///
/// [`InternalError`]: ../ckb_error/struct.InternalError.html
#[derive(Debug, PartialEq, Eq, Clone, Copy, Display)]
pub enum InternalErrorKind {
    /// An arithmetic overflow occurs during capacity calculation, e.g. `Capacity::safe_add`
    CapacityOverflow,

    /// Persistent data had corrupted
    DataCorrupted,

    /// Error occurs during database operations
    Database,

    /// It indicates that the underlying error is [`BlockAssemblerError`]
    ///
    /// [`BlockAssemblerError`]: ../ckb_tx_pool/error/enum.BlockAssemblerError.html
    BlockAssembler,

    /// VM internal error
    VM,

    /// Unknown system error
    System,

    /// The feature is disabled or is conflicted with the configuration
    Config,

    /// Other system error
    Other,
}

def_error_base_on_kind!(InternalError, InternalErrorKind, "Internal error.");

impl_error_conversion_with_kind!(InternalError, crate::ErrorKind::Internal, crate::Error);

impl_error_conversion_with_kind!(OtherError, InternalErrorKind::Other, InternalError);
impl_error_conversion_with_adaptor!(OtherError, InternalError, crate::Error);

impl OtherError {
    /// Creates an error with only a string as the reason.
    pub fn new<T>(reason: T) -> Self
    where
        T: fmt::Display,
    {
        Self(reason.to_string())
    }
}
