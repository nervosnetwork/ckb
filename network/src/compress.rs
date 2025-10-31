//！ckb network compress module

use ckb_logger::debug;
use p2p::bytes::{BufMut, Bytes, BytesMut};
use snap::raw::{Decoder as SnapDecoder, Encoder as SnapEncoder, decompress_len};
use tokio_util::codec::length_delimited;

use std::io;

pub(crate) const COMPRESSION_SIZE_THRESHOLD: usize = 1024;
pub(crate) const UNCOMPRESS_FLAG: u8 = 0b0000_0000;
const COMPRESS_FLAG: u8 = 0b1000_0000;
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

    /// Compress message
    pub(crate) fn compress(mut self) -> Bytes {
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
    pub(crate) fn decompress(mut self) -> Result<Bytes, io::Error> {
        if self.inner.is_empty() {
            Err(io::ErrorKind::InvalidData.into())
        } else if self.compress_flag() {
            match decompress_len(&self.inner[1..]) {
                Ok(decompressed_bytes_len) => {
                    if decompressed_bytes_len > MAX_UNCOMPRESSED_LEN {
                        debug!(
                            "The limit for uncompressed bytes len is exceeded. limit: {}, len: {}",
                            MAX_UNCOMPRESSED_LEN, decompressed_bytes_len
                        );
                        Err(io::ErrorKind::InvalidData.into())
                    } else {
                        let mut buf = vec![0; decompressed_bytes_len];
                        match SnapDecoder::new().decompress(&self.inner[1..], &mut buf) {
                            Ok(_) => Ok(buf.into()),
                            Err(e) => {
                                debug!("snappy decompress error: {:?}", e);
                                Err(io::ErrorKind::InvalidData.into())
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("snappy decompress_len error: {:?}", e);
                    Err(io::ErrorKind::InvalidData.into())
                }
            }
        } else {
            let _ = self.inner.split_to(1);
            Ok(self.inner.freeze())
        }
    }

    pub(crate) fn set_compress_flag(&mut self) {
        self.inner[0] = COMPRESS_FLAG;
    }

    pub(crate) fn compress_flag(&self) -> bool {
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

/// LengthDelimitedCodec with compress support
pub struct LengthDelimitedCodecWithCompress {
    length_delimited: length_delimited::LengthDelimitedCodec,
    enable_compress: bool,
    protocol_id: p2p::ProtocolId,
}

impl LengthDelimitedCodecWithCompress {
    /// Create a new LengthDelimitedCodecWithCompress
    pub fn new(
        enable_compress: bool,
        length_delimited: length_delimited::LengthDelimitedCodec,
        protocol_id: p2p::ProtocolId,
    ) -> Self {
        Self {
            length_delimited,
            enable_compress,
            protocol_id,
        }
    }

    fn process(&self, data: &[u8], flag: u8, dst: &mut BytesMut) -> Result<(), io::Error> {
        let len = data.len() + 1;
        if len > self.length_delimited.max_frame_length() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "data too large",
            ));
        }
        dst.reserve(4 + len);
        dst.put_uint(len as u64, 4);
        dst.put_u8(flag);
        dst.extend_from_slice(data);
        Ok(())
    }
}

impl tokio_util::codec::Encoder<Bytes> for LengthDelimitedCodecWithCompress {
    type Error = io::Error;
    fn encode(&mut self, data: Bytes, dst: &mut BytesMut) -> Result<(), io::Error> {
        if self.enable_compress && data.len() > COMPRESSION_SIZE_THRESHOLD {
            match SnapEncoder::new().compress_vec(&data) {
                Ok(res) => {
                    debug!(
                        "protocol {} message snappy compress result: raw: {}, compressed: {}, ratio: {:.2}%",
                        self.protocol_id,
                        data.len(),
                        res.len(),
                        (res.len() as f64 / data.len() as f64 * 100.0)
                    );
                    if let Some(metrics) = ckb_metrics::handle() {
                        metrics
                            .ckb_network_compress
                            .with_label_values(&[
                                self.protocol_id.to_string().as_str(),
                                "succeeded",
                                "compressed ratio",
                            ])
                            .observe(res.len() as f64 / data.len() as f64);
                    }
                    // compressed data is larger than or equal to uncompressed data
                    if res.len() >= data.len() {
                        self.process(&data, UNCOMPRESS_FLAG, dst)?;
                    } else {
                        self.process(&res, COMPRESS_FLAG, dst)?;
                    }
                }
                Err(e) => {
                    debug!(
                        "protocol {} message snappy compress error: {}",
                        self.protocol_id, e
                    );
                    if let Some(metrics) = ckb_metrics::handle() {
                        metrics
                            .ckb_network_compress
                            .with_label_values(&[
                                self.protocol_id.to_string().as_str(),
                                "failed",
                                "compressed ratio",
                            ])
                            .observe(1.0);
                    }
                    self.process(&data, UNCOMPRESS_FLAG, dst)?;
                }
            }
        } else {
            if let Some(metrics) = ckb_metrics::handle() {
                metrics
                    .ckb_network_not_compress_count
                    .with_label_values(&[self.protocol_id.to_string().as_str()])
                    .inc();
            }
            self.process(&data, UNCOMPRESS_FLAG, dst)?;
        }
        Ok(())
    }
}

impl tokio_util::codec::Decoder for LengthDelimitedCodecWithCompress {
    type Item = BytesMut;
    type Error = io::Error;
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<BytesMut>, io::Error> {
        if src.is_empty() {
            return Ok(None);
        }
        match self.length_delimited.decode(src)? {
            Some(mut data) => {
                if data.len() < 2 {
                    return Err(io::ErrorKind::InvalidData.into());
                }

                if (data[0] & COMPRESS_FLAG) != 0 {
                    match decompress_len(&data[1..]) {
                        Ok(decompressed_bytes_len) => {
                            if decompressed_bytes_len > MAX_UNCOMPRESSED_LEN {
                                debug!(
                                    "The limit for uncompressed bytes len is exceeded. limit: {}, len: {}",
                                    MAX_UNCOMPRESSED_LEN, decompressed_bytes_len
                                );
                                return Err(io::ErrorKind::InvalidData.into());
                            }
                            let mut buf = BytesMut::zeroed(decompressed_bytes_len);
                            match SnapDecoder::new().decompress(&data[1..], &mut buf) {
                                Ok(_) => Ok(Some(buf)),
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
                } else {
                    Ok(Some(data.split_off(1)))
                }
            }
            None => Ok(None),
        }
    }
}
