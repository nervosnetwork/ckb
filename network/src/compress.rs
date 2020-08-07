use ckb_logger::debug;
use p2p::bytes::{BufMut, Bytes, BytesMut};
use snap::raw::{Decoder as SnapDecoder, Encoder as SnapEncoder};

use std::io;

const COMPRESSION_SIZE_THRESHOLD: usize = 1024;
const UNCOMPRESS_FLAG: u8 = 0b0000_0000;
const COMPRESS_FLAG: u8 = 0b1000_0000;

/// Compressed decompression structure
///
/// If you want to support multiple compression formats in the future,
/// you can simply think that 0b1000 is in snappy format and 0b0000 is in uncompressed format.
///
/// # Message in Bytes:
///
/// +---------------------------------------------------------------+
/// | Bytes | Type | Function                                       |
/// |-------+------+------------------------------------------------|
/// |   0   |  u1  | Compress: true 1, false 0                      |
/// |       |  u7  | Reserved                                       |
/// +-------+------+------------------------------------------------+
/// |  1~   |      | Payload (Serialized Data with Compress)        |
/// +-------+------+------------------------------------------------+
#[derive(Clone, Debug)]
struct Message {
    inner: BytesMut,
}

impl Message {
    /// create from uncompressed raw data
    fn from_raw(data: Bytes) -> Self {
        let mut inner = BytesMut::with_capacity(data.len() + 1);
        inner.put_u8(UNCOMPRESS_FLAG);
        inner.put(data);
        Self { inner }
    }

    /// create from compressed data
    fn from_compressed(data: BytesMut) -> Self {
        Self { inner: data }
    }

    /// Compress message
    fn compress(mut self) -> Bytes {
        if self.inner.len() > COMPRESSION_SIZE_THRESHOLD {
            let input = self.inner.split_off(1);
            match SnapEncoder::new().compress_vec(&input) {
                Ok(res) => {
                    self.inner.extend_from_slice(&res);
                    self.set_compress_flag();
                }
                Err(e) => {
                    debug!("snappy compress error: {}", e);
                    self.inner.unsplit(input);
                }
            }
        }
        self.inner.freeze()
    }

    /// Decompress message
    fn decompress(mut self) -> Result<Bytes, io::Error> {
        if self.inner.is_empty() {
            Err(io::ErrorKind::InvalidData.into())
        } else if self.compress_flag() {
            match SnapDecoder::new().decompress_vec(&self.inner[1..]) {
                Ok(res) => Ok(Bytes::from(res)),
                Err(e) => {
                    debug!("snappy decompress error: {:?}", e);
                    Err(io::ErrorKind::InvalidData.into())
                }
            }
        } else {
            let _ = self.inner.split_to(1);
            Ok(self.inner.freeze())
        }
    }

    fn set_compress_flag(&mut self) {
        self.inner[0] = COMPRESS_FLAG;
    }

    fn compress_flag(&self) -> bool {
        (self.inner[0] & COMPRESS_FLAG) != 0
    }
}

/// Compress data
pub fn compress(src: Bytes) -> Bytes {
    Message::from_raw(src).compress()
}

/// Decompress data
pub fn decompress(src: BytesMut) -> Result<Bytes, io::Error> {
    Message::from_compressed(src).decompress()
}

#[cfg(test)]
mod test {
    use super::{Bytes, BytesMut, Message, COMPRESSION_SIZE_THRESHOLD};

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
}
