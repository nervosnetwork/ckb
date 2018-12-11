mod builder;
mod convert;
#[rustfmt::skip]
#[allow(clippy::all)]
mod protocol_generated;

pub use crate::protocol_generated::ckb::protocol::*;
use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use ckb_util::u64_to_bytes;
use hash::sha3_256;
use numext_fixed_hash::H256;
use siphasher::sip::SipHasher;
use std::hash::Hasher;

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
    // sha3-256(header nonce + random nonce) in little-endian
    let mut bytes = vec![];
    bytes.write_u64::<LittleEndian>(header_nonce).unwrap();
    bytes.write_u64::<LittleEndian>(random_nonce).unwrap();
    let block_header_with_nonce_hash = sha3_256(bytes);

    let key0 = LittleEndian::read_u64(&block_header_with_nonce_hash[0..8]);
    let key1 = LittleEndian::read_u64(&block_header_with_nonce_hash[8..16]);

    (key0, key1)
}

pub fn short_transaction_id(key0: u64, key1: u64, transaction_hash: &H256) -> ShortTransactionID {
    let mut hasher = SipHasher::new_with_keys(key0, key1);
    hasher.write(transaction_hash.as_bytes());
    let siphash_transaction_hash = hasher.finish();

    let siphash_transaction_hash_bytes = u64_to_bytes(siphash_transaction_hash.to_le());

    let mut short_transaction_id = [0u8; 6];

    short_transaction_id.copy_from_slice(&siphash_transaction_hash_bytes[..6]);

    short_transaction_id
}
