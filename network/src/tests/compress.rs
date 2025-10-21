use p2p::bytes::{Bytes, BytesMut};

use crate::compress::{COMPRESSION_SIZE_THRESHOLD, Message, compress, decompress, CompressionType, compress_with};

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

 #[test]  
fn test_compression_types() {  
    let data = Bytes::from(vec![1u8; 2000]);
    
    let snappy_compressed = compress_with(data.clone(), CompressionType::Snappy);
    let snappy_msg = Message::from_compressed(BytesMut::from(snappy_compressed.as_ref()));
    assert_eq!(snappy_msg.compression_type(), CompressionType::Snappy);
    
    let lz4_compressed = compress_with(data.clone(), CompressionType::Lz4);
    let lz4_msg = Message::from_compressed(BytesMut::from(lz4_compressed.as_ref()));
    assert_eq!(lz4_msg.compression_type(), CompressionType::Lz4);
    
    let decompressed_snappy = decompress(BytesMut::from(snappy_compressed.as_ref())).unwrap();
    let decompressed_lz4 = decompress(BytesMut::from(lz4_compressed.as_ref())).unwrap();
    
    assert_eq!(decompressed_snappy, data);
    assert_eq!(decompressed_lz4, data);
}  
