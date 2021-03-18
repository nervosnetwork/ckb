use ckb_types::packed as referenced;

mod conversion;
mod generated;

pub(crate) use generated::*;

/// The version of the persisted data.
pub(crate) const VERSION: u32 = 1;
