//! Underlying error types used over ckb crates.

use std::{error::Error as StdError, fmt, ops::Deref, sync::Arc};

mod convert;
mod internal;
pub mod prelude;
pub mod util;

use derive_more::Display;
pub use internal::{InternalError, InternalErrorKind, OtherError, SilentError};
use prelude::*;

/// A wrapper around a dynamic error type.
#[derive(Debug, Clone)]
pub struct AnyError(Arc<anyhow::Error>);

/// A list specifying categories of ckb error.
///
/// This list is intended to grow over time and it is not recommended to exhaustively match against
/// it.
///
/// It is used with [`Error`].
///
/// [`Error`]: ./struct.Error.html
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum ErrorKind {
    /// It indicates that the underlying error is [`OutPointError`].
    ///
    /// [`OutPointError`]: ../ckb_types/core/error/enum.OutPointError.html
    OutPoint,
    /// It indicates that the underlying error is [`TransactionError`].
    ///
    /// [`TransactionError`]: ../ckb_verification/enum.TransactionError.html
    Transaction,
    /// It indicates that the underlying error is [`Reject`].
    ///
    /// [`Reject`]: ../ckb_tx_pool/error/enum.Reject.html
    SubmitTransaction,
    /// It indicates that the underlying error is [`TransactionScriptError`].
    ///
    /// [`TransactionScriptError`]: ../ckb_script/struct.TransactionScriptError.html
    Script,
    /// It indicates that the underlying error is [`HeaderError`].
    ///
    /// [`HeaderError`]: ../ckb_verification/struct.HeaderError.html
    Header,
    /// It indicates that the underlying error is [`BlockError`]
    ///
    /// [`BlockError`]: ../ckb_verification/struct.BlockError.html
    Block,
    /// It indicates that the underlying error is [`InternalError`]
    ///
    /// [`InternalError`]: ./struct.InternalError.html
    Internal,
    /// It indicates that the underlying error is [`DaoError`]
    ///
    /// [`DaoError`]: ../ckb_types/core/error/enum.OutPointError.html
    Dao,
    /// It indicates that the underlying error is [`SpecError`]
    ///
    /// [`SpecError`]: ../ckb_chain_spec/enum.SpecError.html
    Spec,
}

def_error_base_on_kind!(Error, ErrorKind, "Top-level ckb error type.");

impl<E> From<E> for AnyError
where
    E: StdError + Send + Sync + 'static,
{
    fn from(error: E) -> Self {
        Self(Arc::new(error.into()))
    }
}

impl Deref for AnyError {
    type Target = Arc<anyhow::Error>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for AnyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}
