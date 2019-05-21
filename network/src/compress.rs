use bytes::{Bytes, BytesMut};
use snap::{Decoder as SnapDecoder, Encoder as SnapEncoder};
use tokio::codec::{Decoder, Encoder, LengthDelimitedCodec};

use std::io;

const SKIP_COMPRESS_SIZE: usize = 40 * 1024;

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
    /// Init
    fn init() -> Self {
        Message {
            inner: BytesMut::from(&[0u8][..]),
        }
    }

    /// Compress message
    fn compress(&mut self, input: Bytes) {
        if input.len() > SKIP_COMPRESS_SIZE {
            match SnapEncoder::new().compress_vec(&input) {
                Ok(res) => {
                    self.inner.unsplit(BytesMut::from(res));
                    self.set_compress_flag(true);
                }
                Err(_) => {
                    self.inner.unsplit(BytesMut::from(input));
                    self.set_compress_flag(false);
                }
            }
        } else {
            self.set_compress_flag(false);
            self.inner.unsplit(BytesMut::from(input));
        }
    }

    /// Decompress message
    fn decompress(mut self) -> Option<BytesMut> {
        if self.inner.len() <= 1 {
            None
        } else if self.compress_flag() {
            match SnapDecoder::new().decompress_vec(&self.inner[1..]) {
                Ok(res) => Some(BytesMut::from(res)),
                Err(_) => None,
            }
        } else {
            self.inner.split_to(1);
            Some(self.inner.take())
        }
    }

    fn set_compress_flag(&mut self, flag: bool) {
        let compress_flag = if flag { 0b1000_0000 } else { 0b0000_0000 };
        self.inner[0] = (self.inner[0] & 0b0111_1111) + (compress_flag & 0b1000_0000);
    }

    fn compress_flag(&self) -> bool {
        (self.inner[0] & 0b1000_0000) != 0
    }

    fn into_inner(self) -> Bytes {
        self.inner.freeze()
    }
}

impl From<BytesMut> for Message {
    fn from(src: BytesMut) -> Self {
        Message { inner: src }
    }
}

impl From<Bytes> for Message {
    fn from(src: Bytes) -> Self {
        Message {
            inner: BytesMut::from(src),
        }
    }
}

/// Compress data
pub fn compress(src: Bytes) -> Bytes {
    let mut msg = Message::init();
    msg.compress(src);
    msg.into_inner()
}

/// Decompression structure for Codec
pub struct LengthDelimited(LengthDelimitedCodec);

impl LengthDelimited {
    pub fn new(codec: LengthDelimitedCodec) -> Self {
        LengthDelimited(codec)
    }
}

impl Encoder for LengthDelimited {
    type Item = bytes::Bytes;
    type Error = io::Error;

    fn encode(&mut self, item: Self::Item, dst: &mut bytes::BytesMut) -> Result<(), Self::Error> {
        self.0.encode(item, dst)
    }
}

impl Decoder for LengthDelimited {
    type Item = bytes::BytesMut;
    type Error = io::Error;

    fn decode(&mut self, src: &mut bytes::BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.0
            .decode(src)
            .map(|item| item.and_then(|item| Into::<Message>::into(item).decompress()))
    }
}

#[cfg(test)]
mod test {
    use super::{Bytes, Message, SKIP_COMPRESS_SIZE};

    #[test]
    fn test_no_need_compress() {
        let mut msg = Message::init();
        msg.compress(Bytes::from("1222"));

        assert!(!msg.compress_flag());

        let demsg = msg.decompress().unwrap();

        assert_eq!(Bytes::from("1222"), demsg)
    }

    #[test]
    fn test_compress_and_decompress() {
        let mut msg = Message::init();
        let data = Bytes::from(vec![1; SKIP_COMPRESS_SIZE + 1]);
        msg.compress(data.clone());

        assert!(msg.compress_flag());

        let demsg = msg.decompress().unwrap();

        assert_eq!(data, demsg)
    }

    #[test]
    fn test_compress_and_decompress_use_another_message() {
        let mut msg = Message::init();
        let data = Bytes::from(vec![1; SKIP_COMPRESS_SIZE + 1]);
        msg.compress(data.clone());

        assert!(msg.compress_flag());

        let cmp_msg = msg.into_inner();

        let demsg = Into::<Message>::into(cmp_msg).decompress().unwrap();

        assert_eq!(data, demsg)
    }
}
