#![no_main]

use libfuzzer_sys::fuzz_target;

use ckb_network::bytes::{Bytes, BytesMut};
use ckb_network::compress::decompress;

fuzz_target!(|data: &[u8]| {
    let raw_data = Bytes::from(data.to_vec());
    let _demsg = decompress(BytesMut::from(raw_data.as_ref()));
});
