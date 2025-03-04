//! The error type for Tx-pool operations

use ckb_channel::oneshot::RecvError;
use ckb_error::{
    Error, InternalError, InternalErrorKind, OtherError, impl_error_conversion_with_adaptor,
    impl_error_conversion_with_kind, prelude::*,
};
pub use ckb_types::core::tx_pool::Reject;
use std::fmt;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::watch::error::SendError;

/// The error type for block assemble related
#[derive(Error, Debug, PartialEq, Clone, Eq)]
pub enum BlockAssemblerError {
    /// Input is invalid
    #[error("InvalidInput")]
    InvalidInput,
    /// Parameters is invalid
    #[error("InvalidParams {0}")]
    InvalidParams(String),
    /// BlockAssembler is disabled
    #[error("Disabled")]
    Disabled,
    /// BlockAssembler calculate overflow
    #[error("Overflow")]
    Overflow,
}

impl_error_conversion_with_kind!(
    BlockAssemblerError,
    InternalErrorKind::BlockAssembler,
    InternalError
);

impl_error_conversion_with_adaptor!(BlockAssemblerError, InternalError, Error);

pub(crate) fn handle_try_send_error<T>(error: TrySendError<T>) -> (T, OtherError) {
    let e = OtherError::new(format!("TrySendError {error}"));
    let m = match error {
        TrySendError::Full(t) | TrySendError::Closed(t) => t,
    };
    (m, e)
}

pub(crate) fn handle_recv_error(error: RecvError) -> OtherError {
    OtherError::new(format!("RecvError {error}"))
}

pub(crate) fn handle_send_cmd_error<T: fmt::Debug>(error: SendError<T>) -> OtherError {
    OtherError::new(format!("send command fails: {error:?}"))
}
