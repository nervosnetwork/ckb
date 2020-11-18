//! CKB utilities.
//!
//! Collection of frequently used utilities.
mod linked_hash_set;
mod shrink_to_fit;
pub mod strings;

use std::time::Duration;

pub use linked_hash_map::{Entries as LinkedHashMapEntries, LinkedHashMap};
pub use linked_hash_set::LinkedHashSet;

pub use parking_lot::{
    self, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

/// The timeout that [`lock_or_panic`] waits before it panics.
///
/// It is set to 300 seconds.
///
/// [`lock_or_panic`]: fn.lock_or_panic.html
pub const TRY_LOCK_TIMEOUT: Duration = Duration::from_secs(300);

/// Holds the mutex lock or panics after timeout.
///
/// This is used to panic and restart the app on potential dead lock.
///
/// Try to hold the lock or panic after the timeout [`TRY_LOCK_TIMEOUT`].
///
/// [`TRY_LOCK_TIMEOUT`]: constant.TRY_LOCK_TIMEOUT.html
pub fn lock_or_panic<T>(data: &Mutex<T>) -> MutexGuard<T> {
    data.try_lock_for(TRY_LOCK_TIMEOUT)
        .expect("please check if reach a deadlock")
}
