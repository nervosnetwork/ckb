//! TODO(doc): @keroro520
mod convert;
mod internal;
pub mod prelude;
pub mod util;

pub use anyhow::Error as AnyError;
use derive_more::Display;
pub use internal::{InternalError, InternalErrorKind, OtherError, SilentError};
use prelude::*;

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
