//! TODO(doc): @keroro520

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

/// TODO(doc): @keroro520
#[derive(Debug, Clone, Copy, Eq, PartialEq, Display)]
pub enum ErrorKind {
    /// TODO(doc): @keroro520
    OutPoint,
    /// TODO(doc): @keroro520
    Transaction,
    /// TODO(doc): @keroro520
    SubmitTransaction,
    /// TODO(doc): @keroro520
    Script,
    /// TODO(doc): @keroro520
    Header,
    /// TODO(doc): @keroro520
    Block,
    /// TODO(doc): @keroro520
    Internal,
    /// TODO(doc): @keroro520
    Dao,
    /// TODO(doc): @keroro520
    Spec,
}

def_error_base_on_kind!(Error, ErrorKind);

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
