mod test_accumulate_headers;
mod test_helper;
mod test_mmr;

use crate::Merge;
use blake2b_rs::{Blake2b, Blake2bBuilder};
use bytes::Bytes;

fn new_blake2b() -> Blake2b {
    Blake2bBuilder::new(32).build()
}

#[derive(Eq, PartialEq, Clone, Debug, Default)]
struct NumberHash(pub Bytes);
impl From<u32> for NumberHash {
    fn from(num: u32) -> Self {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&num.to_le_bytes());
        hasher.finalize(&mut hash);
        NumberHash(hash.to_vec().into())
    }
}

struct MergeNumberHash;

impl Merge for MergeNumberHash {
    type Item = NumberHash;
    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> Self::Item {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&lhs.0);
        hasher.update(&rhs.0);
        hasher.finalize(&mut hash);
        NumberHash(hash.to_vec().into())
    }
}
