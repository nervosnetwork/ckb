#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

use super::{Error, ProtocolId};
use bytes::BufMut;
use bytes::{Buf, IntoBuf};
use bytes::{Bytes, BytesMut};
use futures::sync::mpsc;
use futures::{future, stream, Future, Sink, Stream};
use libp2p::core::{ConnectionUpgrade, Endpoint, Multiaddr};
use snap;
use std::io;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::string::ToString;
use std::vec::IntoIter as VecIntoIter;
use tokio::codec::Decoder;
use tokio::io::{AsyncRead, AsyncWrite};
use unsigned_varint::codec::UviBytes;

pub type ProtocolVersion = u8;

#[derive(Clone)]
pub struct CKBProtocol<T> {
    id: ProtocolId,
    // for example: b"/ckb/"
    base_name: Bytes,
    // supported version, used to check protocol version
    supported_versions: Vec<ProtocolVersion>,
    protocol_handler: T,
}

impl<T> CKBProtocol<T> {
    pub fn new(
        base_name: String,
        protocol_handler: T,
        id: ProtocolId,
        versions: &[ProtocolVersion],
    ) -> Self {
        let mut base_name_bytes = Bytes::from(format!("/{}/", base_name));
        base_name_bytes.extend_from_slice(&id);
        base_name_bytes.extend_from_slice(b"/");
        CKBProtocol {
            base_name: base_name_bytes,
            id,
            supported_versions: {
                let mut versions: Vec<_> = versions.to_vec();
                versions.sort_by(|a, b| b.cmp(a));
                versions.to_vec()
            },
            protocol_handler,
        }
    }
    pub fn protocol_handler(&self) -> &T {
        &self.protocol_handler
    }
    pub fn id(&self) -> ProtocolId {
        self.id
    }
    pub fn base_name(&self) -> Bytes {
        self.base_name.clone()
    }
}

pub struct CKBProtocolOutput<T> {
    pub protocol_handler: T,
    pub protocol_id: ProtocolId,
    pub endpoint: Endpoint,
    pub protocol_version: ProtocolVersion,
    // channel to send outgoing messages
    pub outgoing_msg_channel: mpsc::UnboundedSender<Bytes>,
    // stream used to receive incoming messages
    pub incoming_stream: Box<Stream<Item = Bytes, Error = IoError> + Send>,
}

impl<T, C, Maf> ConnectionUpgrade<C, Maf> for CKBProtocol<T>
where
    C: AsyncRead + AsyncWrite + Send + 'static,
    Maf: Future<Item = Multiaddr, Error = IoError> + Send + 'static,
{
    type NamesIter = VecIntoIter<(Bytes, Self::UpgradeIdentifier)>;
    type UpgradeIdentifier = u8;
    type Output = CKBProtocolOutput<T>;
    type MultiaddrFuture = Maf;
    type Future = future::FutureResult<(Self::Output, Self::MultiaddrFuture), IoError>;

    fn protocol_names(&self) -> Self::NamesIter {
        self.supported_versions
            .iter()
            .map(|version| {
                let num = version.to_string();
                let mut name = self.base_name.clone();
                name.extend_from_slice(num.as_bytes());
                (name, *version)
            }).collect::<Vec<_>>()
            .into_iter()
    }

    fn upgrade(
        self,
        socket: C,
        protocol_version: Self::UpgradeIdentifier,
        endpoint: Endpoint,
        remote_addr: Maf,
    ) -> Self::Future {
        // This channel is used to send outgoing packets to the custom_data
        // for this open substream.
        let (incoming_stream, outgoing_msg_channel) =
            match self.build_handling_stream_from_socket(socket, protocol_version) {
                Ok(result) => result,
                Err(err) => {
                    return {
                        error!(target: "network", "failed to upgrade ckb_protocol");
                        future::err(IoError::new(
                            IoErrorKind::Other,
                            format!("faild to upgrade ckb_protocol, error: {}", err),
                        ))
                    }
                }
            };

        let out = CKBProtocolOutput {
            protocol_handler: self.protocol_handler,
            protocol_id: self.id,
            endpoint,
            protocol_version,
            outgoing_msg_channel,
            incoming_stream,
        };
        trace!(target: "network", "success to upgrade ckb_protocol");

        future::ok((out, remote_addr))
    }
}

impl<T> CKBProtocol<T> {
    #[cfg_attr(feature = "cargo-clippy", allow(type_complexity))]
    fn build_handling_stream_from_socket<C>(
        &self,
        socket: C,
        _protocol_version: u8,
    ) -> Result<
        (
            Box<Stream<Item = Bytes, Error = IoError> + Send>,
            mpsc::UnboundedSender<Bytes>,
        ),
        Error,
    >
    where
        C: AsyncWrite + AsyncRead + Send + 'static,
    {
        let (msg_tx, msg_rx) = mpsc::unbounded();

        // Build the sink for outgoing network bytes, and the stream for
        // incoming instructions. `stream` implements `Stream<Item = Message>`.
        enum Message {
            Recv(BytesMut),
            SendData(Bytes),
            Finished,
        }

        let (sink, stream) = {
            let framed = Decoder::framed(UviBytes::default(), socket);
            let msg_rx = msg_rx.map(Message::SendData).map_err(|_err| {
                IoError::new(IoErrorKind::Other, "error when read request from channel")
            });
            let (sink, stream) = framed.split();
            let stream = stream
                .map(Message::Recv)
                .chain(stream::once(Ok(Message::Finished)));
            (sink, msg_rx.select(stream))
        };

        let incoming = Box::new(
            stream::unfold((sink, stream, false), move |(sink, stream, finished)| {
                if finished {
                    return None;
                }

                Some(stream.into_future().map_err(|(err, _)| err).and_then(
                    move |(message, stream)| match message {
                        Some(Message::Recv(compressed_data)) => {
                            if compressed_data.is_empty() {
                                debug!("receive a empty message, ignoring");
                                let f = future::ok((None, (sink, stream, false)));
                                return future::Either::A(f);
                            }
                            // decompress data
                            let mut decompresser = snap::Reader::new(compressed_data.freeze().into_buf().reader());
                            let mut data = vec![].writer();
                            match io::copy(&mut decompresser, &mut data) {
                                Ok(_) => {
                                let out = Some(data.into_inner().into());
                                let f = future::ok((out, (sink, stream, false)));
                                future::Either::A(f)
                                },
                                Err(e) => {
                                    future::Either::A(future::err(e))
                                }
                            }
                        }

                        Some(Message::SendData(data)) => {
                            let mut compressed_data = vec![].writer();
                            let mut compresser = snap::Writer::new(compressed_data);
                            let mut data_buf = data.into_buf();
                            match io::copy(&mut data_buf.reader(), &mut compresser) {
                                Ok(_) => {
                                    match compresser.into_inner() {
                                        Ok(compressed_data) => {
                                            let compressed_data : Bytes = compressed_data.into_inner().into();
                                            let fut = sink
                                                .send(compressed_data)
                                                .map(move |sink| (None, (sink, stream, false)));
                                            future::Either::B(fut)
                                        },
                                                Err(e) => {
                                    future::Either::A(future::err(IoError::new(IoErrorKind::Other, format!("compressed data error {}", e.to_string()))))
                                        }
                                }}
                                Err(e) => {
                                    future::Either::A(future::err(IoError::new(IoErrorKind::Other, format!("error when receive data: {}", e))))
                                }
                            }
                        }

                        Some(Message::Finished) | None => {
                            let f = future::ok((None, (sink, stream, true)));
                            future::Either::A(f)
                        }
                    },
                ))
            }).filter_map(|v| v), // filter will remove non Recv events
        ) as Box<Stream<Item = Bytes, Error = IoError> + Send>;
        Ok((incoming, msg_tx))
    }
}

#[derive(Clone)]
pub struct CKBProtocols<T>(pub Vec<CKBProtocol<T>>);

impl<T> CKBProtocols<T> {
    pub fn find_protocol(&self, protocol_id: ProtocolId) -> Option<&CKBProtocol<T>> {
        self.0.iter().find(|protocol| protocol.id == protocol_id)
    }
}

impl<T> Default for CKBProtocols<T> {
    fn default() -> Self {
        CKBProtocols(Vec::new())
    }
}

impl<T, C, Maf> ConnectionUpgrade<C, Maf> for CKBProtocols<T>
where
    C: AsyncRead + AsyncWrite + Send + 'static,
    Maf: Future<Item = Multiaddr, Error = IoError> + Send + 'static,
{
    type NamesIter = VecIntoIter<(Bytes, Self::UpgradeIdentifier)>;
    type UpgradeIdentifier = (
        usize,
        <CKBProtocol<T> as ConnectionUpgrade<C, Maf>>::UpgradeIdentifier,
    );

    type Output = <CKBProtocol<T> as ConnectionUpgrade<C, Maf>>::Output;
    type MultiaddrFuture = <CKBProtocol<T> as ConnectionUpgrade<C, Maf>>::MultiaddrFuture;
    type Future = <CKBProtocol<T> as ConnectionUpgrade<C, Maf>>::Future;

    fn protocol_names(&self) -> Self::NamesIter {
        self.0
            .iter()
            .enumerate()
            .flat_map(|(n, proto)| {
                ConnectionUpgrade::<C, Maf>::protocol_names(proto)
                    .map(move |(name, id)| (name, (n, id)))
            }).collect::<Vec<_>>()
            .into_iter()
    }

    fn upgrade(
        self,
        socket: C,
        upgrade_identifier: Self::UpgradeIdentifier,
        endpoint: Endpoint,
        remote_addr: Maf,
    ) -> Self::Future {
        let (protocol_index, inner_proto_id) = upgrade_identifier;
        match self.0.into_iter().nth(protocol_index) {
            Some(protocol) => protocol.upgrade(socket, inner_proto_id, endpoint, remote_addr),
            None => future::err(IoError::new(
                IoErrorKind::Other,
                "cant't find ckb_protocol by index".to_string(),
            )),
        }
    }
}
