use p2p::bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder, LengthDelimitedCodec};

use crate::compress::{
    COMPRESSION_SIZE_THRESHOLD, LengthDelimitedCodecWithCompress, Message, UNCOMPRESS_FLAG,
    compress, decompress,
};

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
fn test_length_delimited_codec_with_compress() {
    let mut codec_with_compress =
        LengthDelimitedCodecWithCompress::new(true, LengthDelimitedCodec::new(), 1.into());
    let mut codec = LengthDelimitedCodec::new();

    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let mut buf = BytesMut::new();
    codec_with_compress
        .encode(raw_data.clone(), &mut buf)
        .unwrap();

    let cmp_data = compress(raw_data);
    let mut buf_cmp = {
        let mut buf = BytesMut::new();
        codec.encode(cmp_data, &mut buf).unwrap();
        buf
    };

    assert_eq!(buf_cmp, buf);

    let decoded = codec_with_compress.decode(&mut buf).unwrap().unwrap();

    let decoded_cmp = codec.decode(&mut buf_cmp).unwrap().unwrap();

    let decoded_cmp = decompress(decoded_cmp).unwrap();

    assert_eq!(decoded, decoded_cmp);
}

#[test]
fn test_length_delimited_codec_with_no_need_compress() {
    let raw_data = Bytes::from("short data");
    let mut codec_with_compress =
        LengthDelimitedCodecWithCompress::new(true, LengthDelimitedCodec::new(), 1.into());
    let mut codec = LengthDelimitedCodec::new();

    let mut buf = BytesMut::new();
    codec_with_compress
        .encode(raw_data.clone(), &mut buf)
        .unwrap();

    let cmp_data = compress(raw_data);
    let mut buf_cmp = {
        let mut buf = BytesMut::new();
        codec.encode(cmp_data, &mut buf).unwrap();
        buf
    };

    assert_eq!(buf_cmp, buf);

    let decoded = codec_with_compress.decode(&mut buf).unwrap().unwrap();

    let decoded_cmp = codec.decode(&mut buf_cmp).unwrap().unwrap();

    let decoded_cmp = decompress(decoded_cmp).unwrap();

    assert_eq!(decoded, decoded_cmp);
}

#[test]
fn test_length_delimited_codec_with_compress_disabled() {
    let raw_data = Bytes::from(vec![1; COMPRESSION_SIZE_THRESHOLD + 1]);
    let mut codec_with_compress =
        LengthDelimitedCodecWithCompress::new(false, LengthDelimitedCodec::new(), 1.into());
    let mut codec = LengthDelimitedCodec::new();

    let mut buf = BytesMut::new();
    codec_with_compress
        .encode(raw_data.clone(), &mut buf)
        .unwrap();

    let cmp_data = vec![UNCOMPRESS_FLAG]
        .into_iter()
        .chain(raw_data.iter().cloned())
        .collect::<Vec<u8>>();
    let mut buf_cmp = {
        let mut buf = BytesMut::new();
        codec.encode(cmp_data.into(), &mut buf).unwrap();
        buf
    };

    assert_eq!(buf_cmp, buf);

    let decoded = codec_with_compress.decode(&mut buf).unwrap().unwrap();

    let decoded_cmp = codec.decode(&mut buf_cmp).unwrap().unwrap();

    assert_eq!(decoded, decoded_cmp[1..]);
    assert_eq!(UNCOMPRESS_FLAG, decoded_cmp[0]);
}

#[test]
fn test_length_delimited_codec_with_invalid_data() {
    let mut codec_with_compress =
        LengthDelimitedCodecWithCompress::new(true, LengthDelimitedCodec::new(), 1.into());

    let mut buf = BytesMut::from(&[0u8; 4][..]); // invalid data, length less than 2
    assert!(codec_with_compress.decode(&mut buf).is_err());

    let mut buf = BytesMut::from(&[0u8, 0, 0, 1, 0][..]); // invalid data, length 1
    assert!(codec_with_compress.decode(&mut buf).is_err());

    let mut buf = BytesMut::from(&[0u8, 0, 0, 5, 1, 2, 3][..]); // invalid data, length 5 but only 3 bytes
    assert!(codec_with_compress.decode(&mut buf).is_ok());
    assert!(codec_with_compress.decode(&mut buf).unwrap().is_none());
}
