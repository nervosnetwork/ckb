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
/// # #[macro_use] extern crate ckb_util;
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

/// Return the memory representation of this u64 as a byte array.
///
/// The target platform’s native endianness is used.
/// Portable code likely wants to use this with [`to_be`] or [`to_le`].
///
/// [`to_be`]: #method.to_be
/// [`to_le`]: #method.to_le
///
/// # Examples
///
/// ```
/// extern crate ckb_util;
/// let bytes = ckb_util::u64_to_bytes(1u64.to_le());
/// assert_eq!(bytes, [1, 0, 0, 0, 0, 0, 0, 0]);
/// ```
/// remove it when feature "int_to_from_bytes" stable
#[inline]
pub fn u64_to_bytes(input: u64) -> [u8; 8] {
    unsafe { ::std::mem::transmute(input) }
}
