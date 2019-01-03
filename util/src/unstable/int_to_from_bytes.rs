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
/// let bytes = ckb_util::u64_to_bytes(1u64.to_le());
/// assert_eq!(bytes, [1, 0, 0, 0, 0, 0, 0, 0]);
/// ```
/// remove it when feature "int_to_from_bytes" stable
#[inline]
pub fn u64_to_bytes(input: u64) -> [u8; 8] {
    unsafe { ::std::mem::transmute(input) }
}
