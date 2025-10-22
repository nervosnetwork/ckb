//！ckb network compress module

use ckb_logger::debug;
use p2p::bytes::{BufMut, Bytes, BytesMut};
use snap::raw::{Decoder as SnapDecoder, Encoder as SnapEncoder, decompress_len};

use std::io;

pub(crate) const COMPRESSION_SIZE_THRESHOLD: usize = 1024;
const UNCOMPRESS_FLAG: u8 = 0b0000_0000;
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
    protocol_id: p2p::ProtocolId,
}

impl Message {
    /// create from uncompressed raw data
    pub(crate) fn from_raw(data: Bytes, protocol_id: p2p::ProtocolId) -> Self {
        let mut inner = BytesMut::with_capacity(data.len() + 1);
        inner.put_u8(UNCOMPRESS_FLAG);
        inner.put(data);
        Self { inner, protocol_id }
    }

    /// create from compressed data
    pub(crate) fn from_compressed(data: BytesMut, protocol_id: p2p::ProtocolId) -> Self {
        Self {
            inner: data,
            protocol_id,
        }
    }

    /// Compress message
    pub(crate) fn compress(mut self) -> Bytes {
        if self.inner.len() > COMPRESSION_SIZE_THRESHOLD {
            let input = self.inner.split_off(1);
            match SnapEncoder::new().compress_vec(&input) {
                Ok(res) => {
                    debug!(
                        "protocol {} message snappy compress result: raw: {}, compressed: {}, ratio: {:.2}%",
                        self.protocol_id,
                        input.len(),
                        res.len(),
                        (res.len() as f64 / input.len() as f64 * 100.0)
                    );
                    if let Some(metrics) = ckb_metrics::handle() {
                        metrics
                            .ckb_network_compress
                            .with_label_values(&[
                                self.protocol_id.to_string().as_str(),
                                "succeeded",
                                "compressed ratio",
                            ])
                            .observe(res.len() as f64 / input.len() as f64);
                    }
                    if res.len() >= input.len() {
                        // compressed data is larger than or equal to uncompressed data
                        self.inner.unsplit(input);
                    } else {
                        self.inner.extend_from_slice(&res);
                        self.set_compress_flag();
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
                    self.inner.unsplit(input);
                }
            }
        } else if let Some(metrics) = ckb_metrics::handle() {
            metrics
                .ckb_network_not_compress_count
                .with_label_values(&[self.protocol_id.to_string().as_str()])
                .inc();
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
pub fn compress(protocol_id: p2p::ProtocolId) -> impl Fn(Bytes) -> Bytes + 'static + Send {
    move |src: Bytes| Message::from_raw(src, protocol_id).compress()
}

/// Decompress data
pub fn decompress(
    protocol_id: p2p::ProtocolId,
) -> impl Fn(BytesMut) -> Result<Bytes, io::Error> + 'static + Send {
    move |src| Message::from_compressed(src, protocol_id).decompress()
}
