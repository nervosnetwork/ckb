use crate::errors::{Error, ProtocolError};
use crate::{
    peers_registry::Session, PeerId, ServiceContext, SessionContext, SessionId, SessionType,
};
use bytes::BufMut;
use bytes::{Buf, IntoBuf};
use bytes::{Bytes, BytesMut};
use futures::sync::mpsc::{self, Sender};
use futures::{future, stream, Future, Sink, Stream};
use log::{debug, error, info, trace};
use p2p::{
    multiaddr::Multiaddr,
    traits::{ProtocolMeta, ServiceProtocol},
    ProtocolId,
};
use std::io::{self, Error as IoError, ErrorKind as IoErrorKind};
use std::string::ToString;
use std::vec::IntoIter as VecIntoIter;
use tokio::codec::Decoder;
use tokio::codec::LengthDelimitedCodec;
use tokio::io::{AsyncRead, AsyncWrite};

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
    Connected(PeerId, Multiaddr, Session, ProtocolId, Version),
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
    fn connected(&mut self, control: &mut ServiceContext, session: &SessionContext, version: &str) {
        let (peer_id, version) = {
            let parsed_version = version.parse::<u8>();
            if session.remote_pubkey.is_none() || parsed_version.is_err() {
                error!(target: "network", "ckb protocol connected error, addr: {}, version: {}", session.address, version);
                control.disconnect(session.id);
                return;
            }
            (
                session
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .unwrap(),
                parsed_version.unwrap(),
            )
        };
        let event = Event::Connected(
            peer_id,
            session.address.clone(),
            Session {
                id: session.id,
                session_type: session.ty,
            },
            self.id,
            version,
        );
        self.event_sender.try_send(event);
    }

    fn disconnected(&mut self, _control: &mut ServiceContext, session: &SessionContext) {
        if let Some(peer_id) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
        {
            self.event_sender
                .try_send(Event::Disconnected(peer_id, self.id));
        }
    }

    fn received(&mut self, _control: &mut ServiceContext, session: &SessionContext, data: Vec<u8>) {
        if let Some(peer_id) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
        {
            self.event_sender
                .try_send(Event::Received(peer_id, self.id, data));
        }
    }
    fn notify(&mut self, _control: &mut ServiceContext, token: u64) {
        self.event_sender.try_send(Event::Notify(self.id, token));
    }
}
