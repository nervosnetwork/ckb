extern crate bigint;
extern crate byteorder;
extern crate ckb_core;
extern crate flatbuffers;
extern crate hash;
extern crate rand;
extern crate siphasher;

mod builder;
mod convert;
#[cfg_attr(rustfmt, rustfmt_skip)]
#[cfg_attr(
    feature = "cargo-clippy",
    allow(clippy, unused_extern_crates)
)]
mod protocol_generated;

pub use protocol_generated::ckb::protocol::*;

use bigint::{H256, H48};
use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use hash::sha3_256;
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

pub type ShortTransactionID = H48;

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
    hasher.write(transaction_hash);
    let siphash_transaction_hash = hasher.finish();

    let mut siphash_transaction_hash_bytes = [0u8; 8];
    LittleEndian::write_u64(
        &mut siphash_transaction_hash_bytes,
        siphash_transaction_hash,
    );

    siphash_transaction_hash_bytes[0..6].into()
}
