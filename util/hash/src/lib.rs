//! CKB default hash function.
//!
//! CKB uses [blake2b] with following configurations as the default hash function.
//!
//! * output digest size: 32
//! * personalization: ckb-default-hash
//!
//! [blake2b]: https://blake2.net/blake2.pdf
pub use blake2b_rs::{Blake2b, Blake2bBuilder};

#[doc(hidden)]
pub const BLAKE2B_KEY: &[u8] = &[];
/// Output digest size.
pub const BLAKE2B_LEN: usize = 32;
/// Blake2b personalization.
pub const CKB_HASH_PERSONALIZATION: &[u8] = b"ckb-default-hash";
/// The hash output on empty input.
///
/// ## Examples
///
/// ```
/// use ckb_hash::{BLANK_HASH, blake2b_256};
///
/// assert_eq!(BLANK_HASH, blake2b_256(&b""));
/// ```
pub const BLANK_HASH: [u8; 32] = [
    68, 244, 198, 151, 68, 213, 248, 197, 93, 100, 32, 98, 148, 157, 202, 228, 155, 196, 231, 239,
    67, 211, 136, 197, 161, 47, 66, 181, 99, 61, 22, 62,
];

/// Creates a new hasher.
///
/// This can be used to hash inputs incrementally.
///
/// ## Examples
///
/// ```
/// use ckb_hash::new_blake2b;
///
/// let mut hasher = new_blake2b();
/// hasher.update(&b"left|"[..]);
/// hasher.update(&b"right"[..]);
/// let mut result = [0u8; 32];
/// hasher.finalize(&mut result); // Saves hash result
/// ```
pub fn new_blake2b() -> Blake2b {
    Blake2bBuilder::new(32)
        .personal(CKB_HASH_PERSONALIZATION)
        .build()
}

/// Hashes the slice of binary and returns the digest.
///
/// ## Examples
///
/// ```
/// use ckb_hash::blake2b_256;
///
/// let input = b"ckb";
/// let digest = blake2b_256(&input);
/// println!("ckbhash({:?}) = {:?}", input, digest);
/// ```
pub fn blake2b_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
    if s.as_ref().is_empty() {
        return BLANK_HASH;
    }
    inner_blake2b_256(s)
}

fn inner_blake2b_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut blake2b = new_blake2b();
    blake2b.update(s.as_ref());
    blake2b.finalize(&mut result);
    result
}

#[test]
fn empty_blake2b() {
    let actual = inner_blake2b_256([]);
    assert_eq!(actual, BLANK_HASH);
}
