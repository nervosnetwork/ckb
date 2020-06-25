mod linked_hash_set;
mod shrink_to_fit;

use std::time::Duration;

pub use linked_hash_map::{Entries as LinkedHashMapEntries, LinkedHashMap};
pub use linked_hash_set::LinkedHashSet;

pub use parking_lot::{
    self, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

const TRY_LOCK_TIMEOUT: Duration = Duration::from_secs(300);

pub fn lock_or_panic<T>(data: &Mutex<T>) -> MutexGuard<T> {
    data.try_lock_for(TRY_LOCK_TIMEOUT)
        .expect("please check if reach a deadlock")
}
