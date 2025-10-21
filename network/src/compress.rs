//！ckb network compress module

use ckb_logger::debug;
use p2p::bytes::{BufMut, Bytes, BytesMut};
use snap::raw::{Decoder as SnapDecoder, Encoder as SnapEncoder, decompress_len};

use std::io;

pub(crate) const COMPRESSION_SIZE_THRESHOLD: usize = 1024;
const UNCOMPRESS_FLAG: u8 = 0b0000_0000;
const SNAPPY_FLAG: u8 = 0b1000_0000;
const LZ4_FLAG: u8 = 0b0100_0000;
const MAX_UNCOMPRESSED_LEN: usize = 1 << 23; // 8MB

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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompressionType {
    None,
    Snappy,
    Lz4,
}

impl CompressionType {
    fn from_flag(flag: u8) -> Self {
        match flag & 0b1100_0000 {
            SNAPPY_FLAG => CompressionType::Snappy,
            LZ4_FLAG => CompressionType::Lz4,
            _ => CompressionType::None,
        }
    }

    fn to_flag(self) -> u8 {
        match self {
            CompressionType::None => UNCOMPRESS_FLAG,
            CompressionType::Snappy => SNAPPY_FLAG,
            CompressionType::Lz4 => LZ4_FLAG,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Message {
    inner: BytesMut,
}

impl Message {
    /// create from uncompressed raw data
    pub(crate) fn from_raw(data: Bytes) -> Self {
        let mut inner = BytesMut::with_capacity(data.len() + 1);
        inner.put_u8(UNCOMPRESS_FLAG);
        inner.put(data);
        Self { inner }
    }

    /// create from compressed data
    pub(crate) fn from_compressed(data: BytesMut) -> Self {
        Self { inner: data }
    }

    /// Compress message with specified compression type
    pub(crate) fn compress_with(mut self, compression_type: CompressionType) -> Bytes {
        if self.inner.len() > COMPRESSION_SIZE_THRESHOLD && compression_type != CompressionType::None {
            let input = self.inner.split_off(1);
            let compress_result = match compression_type {
                CompressionType::Snappy => self.compress_snappy(&input),
                CompressionType::Lz4 => self.compress_lz4(&input),
                CompressionType::None => return self.inner.freeze(),
            };

            match compress_result {
                Ok(res) => {
                    self.inner.extend_from_slice(&res);
                    self.set_compression_flag(compression_type);
                }
                Err(e) => {
                    debug!("{:?} compress error: {}", compression_type, e);
                    self.inner.unsplit(input);
                }
            }
        }
        self.inner.freeze()
    }

    /// Compress message with default snappy compression
    pub(crate) fn compress(self) -> Bytes {
        self.compress_with(CompressionType::Snappy)
    }

    /// Compress message in snappy format
    fn compress_snappy(&self, input: &BytesMut) -> Result<Vec<u8>, io::Error> {
        SnapEncoder::new()
            .compress_vec(input)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })
    }

    /// Compress message in lz4 format
    fn compress_lz4(&self, input: &BytesMut) -> Result<Vec<u8>, io::Error> {
        lz4::block::compress(input, None, false)
            .map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })
    }

    /// Decompress message
    pub(crate) fn decompress(mut self) -> Result<Bytes, io::Error> {
        if self.inner.is_empty() {
            return Err(io::ErrorKind::InvalidData.into());
        }

        let compression_type = self.compression_type();

        match compression_type {
            CompressionType::None => {
                let _ = self.inner.split_to(1);
                Ok(self.inner.freeze())
            },
            CompressionType::Snappy => self.decompress_snappy(),
            CompressionType::Lz4 => self.decompress_lz4(),
        }
    }

    /// Decompress message in snappy format
    fn decompress_snappy(&mut self) -> Result<Bytes, io::Error> {
        match decompress_len(&self.inner[1..]) {
            Ok(decompressed_bytes_len) => {
                if decompressed_bytes_len > MAX_UNCOMPRESSED_LEN {
                    debug!(
                        "The limit for uncompressed bytes len is exceeded. limit: {}, len: {}",
                        MAX_UNCOMPRESSED_LEN, decompressed_bytes_len
                    );
                    return Err(io::ErrorKind::InvalidData.into());
                }
                  
                let mut buf = vec![0; decompressed_bytes_len];
                match SnapDecoder::new().decompress(&self.inner[1..], &mut buf) {
                    Ok(_) => Ok(buf.into()),
                    Err(e) => {
                        debug!("snappy decompress error: {:?}", e);
                        Err(io::ErrorKind::InvalidData.into())
                    }
                }
            }
            Err(e) => {
                debug!("snappy decompress_len error: {:?}", e);
                Err(io::ErrorKind::InvalidData.into())
            }
        }
    }

    /// Decompress message in lz4 format
    fn decompress_lz4(&mut self) -> Result<Bytes, io::Error> {
        match lz4::block::decompress(&self.inner[1..], Some(MAX_UNCOMPRESSED_LEN as i32)) {
            Ok(decompressed_data) => {
                if decompressed_data.len() > MAX_UNCOMPRESSED_LEN {
                    debug!(
                        "The limit for uncompressed bytes len is exceeded. limit: {}, len: {}",
                        MAX_UNCOMPRESSED_LEN, decompressed_data.len()
                    );
                    return Err(io::ErrorKind::InvalidData.into());
                }
                Ok(decompressed_data.into())
            }
            Err(e) => {
                debug!("lz4 decompress error: {:?}", e);
                Err(io::ErrorKind::InvalidData.into())
            }
        }
    }

    fn set_compression_flag(&mut self, compression_type: CompressionType) {
        self.inner[0] = compression_type.to_flag();
    }

    pub(crate) fn compression_type(&self) -> CompressionType {
        CompressionType::from_flag(self.inner[0])
    }

    pub(crate) fn compress_flag(&self) -> bool {
        self.compression_type() != CompressionType::None
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

/// Compress data with specified compression type
pub fn compress_with(src: Bytes, compression_type: CompressionType) -> Bytes {
    Message::from_raw(src).compress_with(compression_type)
}
