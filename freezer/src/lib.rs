//! An append-only archive for immutable chain data, with explicit durable commits.

mod block;
mod controller;
mod format;
mod freezer;
mod freezer_files;
mod payload;
mod reader;
mod storage;
#[cfg(test)]
mod tests;

use ckb_error::{Error, InternalErrorKind};
use std::fmt::{Debug, Display};

fn internal_error<S: Display + Debug + Sync + Send + 'static>(reason: S) -> Error {
    InternalErrorKind::Database.other(reason).into()
}

pub use block::ArchivedBlock;
pub use controller::{FreezerController, FreezerServiceConfig, FreezerServiceStatus};
pub use freezer::Freezer;
pub use freezer_files::FreezerFilesBuilder;
pub use reader::ArchiveRecord;
