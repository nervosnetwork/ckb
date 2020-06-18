mod freezer;
mod freezer_files;
#[cfg(test)]
mod tests;

use ckb_error::{Error, InternalErrorKind};
use std::fmt::{Debug, Display};

fn internal_error<S: Display + Debug + Sync + Send + 'static>(reason: S) -> Error {
    InternalErrorKind::Database.reason(reason).into()
}

pub use freezer::Freezer;
