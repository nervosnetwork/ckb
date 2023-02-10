//! CKB utilities.
//!
//! Collection of frequently used utilities.
mod linked_hash_set;
mod shrink_to_fit;
pub mod strings;

mod long_live_tmp_dir;
#[cfg(test)]
mod tests;

pub use long_live_tmp_dir::long_live_tmp_dir;

pub use linked_hash_map::{Entries as LinkedHashMapEntries, LinkedHashMap};
pub use linked_hash_set::LinkedHashSet;

pub use parking_lot::{
    self, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
