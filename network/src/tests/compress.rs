use p2p::bytes::{Bytes, BytesMut};

use crate::compress::{COMPRESSION_SIZE_THRESHOLD, Message, compress, decompress};

#[test]
fn test_no_need_compress() {
    let cmp_data = Message::from_raw(Bytes::from("1222"), 1.into()).compress();

    let msg = Message::from_compressed(BytesMut::from(cmp_data.as_ref()), 1.into());

    assert!(!msg.compress_flag());

    let demsg = msg.decompress().unwrap();

    assert_eq!(Bytes::from("1222"), demsg)
}

#[test]
fn test_compress_and_decompress() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let cmp_data = Message::from_raw(raw_data.clone(), 1.into()).compress();

    let msg = Message::from_compressed(BytesMut::from(cmp_data.as_ref()), 1.into());
    assert!(msg.compress_flag());

    let demsg = msg.decompress().unwrap();

    assert_eq!(raw_data, demsg)
}

#[test]
fn test_compress_and_decompress_with_pub_fn() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let cmp_data = compress(1.into())(raw_data.clone());

    let demsg = decompress(1.into())(BytesMut::from(cmp_data.as_ref())).unwrap();

    assert_eq!(raw_data, demsg)
}

#[test]
fn test_invalid_data() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let cmp_data = compress(1.into())(raw_data);

    assert!(decompress(1.into())(BytesMut::from(&cmp_data.as_ref()[1..])).is_err());
    assert!(decompress(1.into())(BytesMut::new()).is_err());
}
