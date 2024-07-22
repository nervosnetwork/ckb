#![no_main]

use libfuzzer_sys::fuzz_target;

use ckb_network::bytes::{Bytes, BytesMut};
use ckb_network::compress::{compress, decompress};

fuzz_target!(|data: &[u8]| {
    let raw_data = Bytes::from(data.to_vec());

    let cmp_data = compress(raw_data.clone());
    let demsg = decompress(BytesMut::from(cmp_data.as_ref())).unwrap();
    assert_eq!(raw_data, demsg);
});
