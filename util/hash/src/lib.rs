pub use blake2b_rs::{Blake2b, Blake2bBuilder};

pub const BLAKE2B_KEY: &[u8] = &[];
pub const BLAKE2B_LEN: usize = 32;
pub const CKB_HASH_PERSONALIZATION: &[u8] = b"ckb-default-hash";

pub fn new_blake2b() -> Blake2b {
    Blake2bBuilder::new(32)
        .personal(CKB_HASH_PERSONALIZATION)
        .build()
}

pub fn blake2b_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
    let mut result = [0u8; 32];
    let mut blake2b = new_blake2b();
    blake2b.update(s.as_ref());
    blake2b.finalize(&mut result);
    result
}

#[test]
fn empty_blake2b() {
    let actual = blake2b_256([]);
    let expected = "44f4c69744d5f8c55d642062949dcae49bc4e7ef43d388c5a12f42b5633d163e";
    assert_eq!(&faster_hex::hex_string(&actual).unwrap(), expected);
}
