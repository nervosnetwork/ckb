mod linked_hash_set;

use std::fmt;
use std::time::Duration;

pub use fnv::{FnvBuildHasher, FnvHashMap, FnvHashSet};
pub use linked_hash_map::{Entries as LinkedHashMapEntries, LinkedHashMap};
pub use linked_hash_set::LinkedHashSet;

pub type LinkedFnvHashMap<K, V> = LinkedHashMap<K, V, FnvBuildHasher>;
pub type LinkedFnvHashMapEntries<'a, K, V> = LinkedHashMapEntries<'a, K, V, FnvBuildHasher>;
pub type LinkedFnvHashSet<T> = LinkedHashSet<T, FnvBuildHasher>;

pub use parking_lot::{
    self, Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};

const TRY_LOCK_TIMEOUT: Duration = Duration::from_secs(300);

pub fn lock_or_panic<T>(data: &Mutex<T>) -> MutexGuard<T> {
    data.try_lock_for(TRY_LOCK_TIMEOUT)
        .expect("please check if reach a deadlock")
}

/// Helper macro for reducing boilerplate code for matching `Option` together
/// with early return.
///
/// # Examples
///

/// ```
/// # use ckb_util::try_option;
/// # fn foo() -> Option<u64> {
///     let a = try_option!(Some(4));
///     let b = try_option!(Some(3));
///     None
/// # }
///
/// //The method of quick returning unit
/// # fn bar() {
///     let a = try_option!(Some(4), ());
///     let b = try_option!(Some(3), ());
/// # }
/// ```
#[macro_export]
macro_rules! try_option {
    ($expr:expr) => {
        try_option!($expr, ::std::option::Option::None)
    };
    ($expr:expr, $re:expr) => {
        match $expr {
            ::std::option::Option::Some(val) => val,
            ::std::option::Option::None => return $re,
        }
    };
}

pub struct LowerHexOption<T>(pub Option<T>);

impl<T: fmt::LowerHex> fmt::LowerHex for LowerHexOption<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            Some(ref v) => write!(f, "Some({:x})", v),
            None => write!(f, "None"),
        }
    }
}
