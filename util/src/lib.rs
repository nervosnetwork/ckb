extern crate parking_lot;

pub use parking_lot::{
    Condvar, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockUpgradableReadGuard,
    RwLockWriteGuard,
};

/// Helper macro for reducing boilerplate code for matching `Option` together
/// with early return.
///
/// # Examples
///

/// ```
/// # #[macro_use] extern crate nervos_util;
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
