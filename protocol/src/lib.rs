mod builder;
mod convert;
pub mod error;
#[rustfmt::skip]
#[allow(clippy::all)]
#[allow(unused_imports)]
mod protocol_generated;
#[rustfmt::skip]
mod protocol_generated_verifier;

pub use crate::protocol_generated::ckb::protocol::*;
use byteorder::{LittleEndian, ReadBytesExt};
use ckb_hash::new_blake2b;
pub use flatbuffers;
use numext_fixed_hash::H256;
use siphasher::sip::SipHasher;
use std::hash::Hasher;

pub fn get_root<'a, T>(data: &'a [u8]) -> Result<T::Inner, error::Error>
where
    T: flatbuffers::Follow<'a> + 'a,
    T::Inner: flatbuffers_verifier::Verify,
{
    flatbuffers_verifier::get_root::<T>(data).map_err(|_| error::Error::Malformed)
}

pub struct FlatbuffersVectorIterator<'a, T: flatbuffers::Follow<'a> + 'a> {
    vector: flatbuffers::Vector<'a, T>,
    counter: usize,
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> FlatbuffersVectorIterator<'a, T> {
    pub fn new(vector: flatbuffers::Vector<'a, T>) -> Self {
        Self { vector, counter: 0 }
    }
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> Iterator for FlatbuffersVectorIterator<'a, T> {
    type Item = T::Inner;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter < self.vector.len() {
            let result = self.vector.get(self.counter);
            self.counter += 1;
            Some(result)
        } else {
            None
        }
    }
}

pub type ShortTransactionID = [u8; 6];

pub fn short_transaction_id_keys(header_nonce: u64, random_nonce: u64) -> (u64, u64) {
    // blake2b-256(header nonce + random nonce) in little-endian
    let mut block_header_with_nonce_hash = [0; 32];
    let mut blake2b = new_blake2b();
    blake2b.update(&header_nonce.to_le_bytes());
    blake2b.update(&random_nonce.to_le_bytes());
    blake2b.finalize(&mut block_header_with_nonce_hash);

    let key0 = (&block_header_with_nonce_hash[0..8])
        .read_u64::<LittleEndian>()
        .expect("read bound checked, should not fail");
    let key1 = (&block_header_with_nonce_hash[8..16])
        .read_u64::<LittleEndian>()
        .expect("read bound checked, should not fail");

    (key0, key1)
}

pub fn short_transaction_id(key0: u64, key1: u64, witness_hash: &H256) -> ShortTransactionID {
    let mut hasher = SipHasher::new_with_keys(key0, key1);
    hasher.write(witness_hash.as_bytes());
    let siphash_transaction_hash = hasher.finish();

    let siphash_transaction_hash_bytes = siphash_transaction_hash.to_le_bytes();

    let mut short_transaction_id = [0u8; 6];

    short_transaction_id.copy_from_slice(&siphash_transaction_hash_bytes[..6]);

    short_transaction_id
}

#[macro_export]
macro_rules! cast {
    ($expr:expr) => {
        $expr.ok_or_else(|| $crate::error::Error::Malformed)
    };
}
