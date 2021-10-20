use p2p::bytes::{Bytes, BytesMut};

use crate::compress::{compress, decompress, Message, COMPRESSION_SIZE_THRESHOLD};

#[test]
fn test_no_need_compress() {
    let cmp_data = Message::from_raw(Bytes::from("1222")).compress();

    let msg = Message::from_compressed(BytesMut::from(cmp_data.as_ref()));

    assert!(!msg.compress_flag());

    let demsg = msg.decompress().unwrap();

    assert_eq!(Bytes::from("1222"), demsg)
}

#[test]
fn test_compress_and_decompress() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let cmp_data = Message::from_raw(raw_data.clone()).compress();

    let msg = Message::from_compressed(BytesMut::from(cmp_data.as_ref()));
    assert!(msg.compress_flag());

    let demsg = msg.decompress().unwrap();

    assert_eq!(raw_data, demsg)
}

#[test]
fn test_compress_and_decompress_with_pub_fn() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let cmp_data = compress(raw_data.clone());

    let demsg = decompress(BytesMut::from(cmp_data.as_ref())).unwrap();

    assert_eq!(raw_data, demsg)
}

#[test]
fn test_invalid_data() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let cmp_data = compress(raw_data);

    assert!(decompress(BytesMut::from(&cmp_data.as_ref()[1..])).is_err());
    assert!(decompress(BytesMut::new()).is_err());
}
