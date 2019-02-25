use crate::errors::{Error, ProtocolError};
use bytes::BufMut;
use bytes::{Buf, IntoBuf};
use bytes::{Bytes, BytesMut};
use futures::sync::mpsc;
use futures::{future, stream, Future, Sink, Stream};
use log::{debug, error, trace};
use p2p::{
    context::{ServiceContext, SessionContext},
    multiaddr::Multiaddr,
    traits::{ProtocolMeta, ServiceProtocol},
    PeerId, ProtocolId, SessionId, SessionType,
};
use std::io::{self, Error as IoError, ErrorKind as IoErrorKind};
use std::string::ToString;
use std::vec::IntoIter as VecIntoIter;
use tokio::codec::Decoder;
use tokio::codec::LengthDelimitedCodec;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::Sender;

pub type Version = u8;

#[derive(Clone)]
pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    base_name: Bytes,
    // supported version, used to check protocol version
    supported_versions: Vec<Version>,
    event_sender: Sender<Event>,
}

impl CKBProtocol {
    pub fn new(
        base_name: String,
        id: ProtocolId,
        versions: &[Version],
        event_sender: Sender<Event>,
    ) -> Self {
        let mut base_name_bytes = Bytes::from(format!("/{}/", base_name));
        base_name_bytes.extend_from_slice(format!("{}", id).as_bytes());
        base_name_bytes.extend_from_slice(b"/");
        CKBProtocol {
            base_name: base_name_bytes,
            id,
            supported_versions: {
                let mut versions: Vec<_> = versions.to_vec();
                versions.sort_by(|a, b| b.cmp(a));
                versions.to_vec()
            },
            event_sender,
        }
    }
    pub fn id(&self) -> ProtocolId {
        self.id
    }
    pub fn base_name(&self) -> Bytes {
        self.base_name.clone()
    }
}

impl ProtocolMeta<LengthDelimitedCodec> for CKBProtocol {
    fn id(&self) -> ProtocolId {
        self.id
    }

    fn codec(&self) -> LengthDelimitedCodec {
        LengthDelimitedCodec::new()
    }

    fn service_handle(&self) -> Option<Box<dyn ServiceProtocol + Send + 'static>> {
        let handler = Box::new(CKBHandler {
            id: self.id,
            event_sender: self.event_sender.clone(),
        });
        Some(handler)
    }

    fn support_versions(&self) -> Vec<String> {
        self.supported_versions
            .iter()
            .map(|v| format!("{}", v))
            .collect()
    }
}

pub enum Event {
    Connected(PeerId, Multiaddr, ProtocolId, SessionType, Version),
    ConnectedError(Multiaddr),
    Disconnected(PeerId, ProtocolId),
    Received(PeerId, ProtocolId, Vec<u8>),
    Notify(ProtocolId, u64),
}

struct CKBHandler {
    id: ProtocolId,
    event_sender: Sender<Event>,
}

impl ServiceProtocol for CKBHandler {
    fn init(&mut self, _control: &mut ServiceContext) {}
    fn connected(
        &mut self,
        _control: &mut ServiceContext,
        session: &SessionContext,
        version: &str,
    ) {
        let event = match session.remote_pubkey {
            Some(ref pubkey) => {
                let peer_id = pubkey.peer_id();
                Event::Connected(
                    peer_id,
                    session.address.clone(),
                    self.id,
                    session.ty,
                    version.parse::<u8>().expect("version"),
                )
            }
            None => Event::ConnectedError(session.address.clone()),
        };
        self.event_sender.try_send(event);
    }

    fn disconnected(&mut self, _control: &mut ServiceContext, session: &SessionContext) {
        let peer_id = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
            .expect("pubkey");
        self.event_sender
            .try_send(Event::Disconnected(peer_id, self.id));
    }

    fn received(&mut self, control: &mut ServiceContext, session: &SessionContext, data: Vec<u8>) {
        let peer_id = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
            .expect("pubkey");
        self.event_sender
            .try_send(Event::Received(peer_id, self.id, data));
    }
    fn notify(&mut self, control: &mut ServiceContext, token: u64) {
        self.event_sender.try_send(Event::Notify(self.id, token));
    }
}
