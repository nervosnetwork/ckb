use crate::{peers_registry::Session, PeerId, ServiceContext, SessionContext};
use futures::sync::mpsc::Sender;
use log::{debug, error};
use p2p::{
    multiaddr::Multiaddr,
    traits::{ProtocolMeta, ServiceProtocol},
    ProtocolId,
};
use tokio::codec::LengthDelimitedCodec;

pub type Version = u8;

#[derive(Clone)]
pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    protocol_name: String,
    // supported version, used to check protocol version
    supported_versions: Vec<Version>,
    event_sender: Sender<Event>,
}

impl CKBProtocol {
    pub fn new(
        protocol_name: String,
        id: ProtocolId,
        versions: &[Version],
        event_sender: Sender<Event>,
    ) -> Self {
        CKBProtocol {
            protocol_name: format!("/ckb/{}/", protocol_name).to_string(),
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

    pub fn protocol_name(&self) -> String {
        self.protocol_name.clone()
    }

    pub fn match_version(&self, version: Version) -> bool {
        self.supported_versions.contains(&version)
    }
}

impl ProtocolMeta<LengthDelimitedCodec> for CKBProtocol {
    fn name(&self) -> String {
        self.protocol_name()
    }

    fn id(&self) -> ProtocolId {
        CKBProtocol::id(&self)
    }

    fn codec(&self) -> LengthDelimitedCodec {
        LengthDelimitedCodec::new()
    }

    fn service_handle(&self) -> Option<Box<dyn ServiceProtocol + Send + 'static>> {
        let handler = Box::new(CKBHandler {
            id: self.id(),
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

#[derive(Debug)]
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

impl CKBHandler {
    fn send_event(&mut self, event: Event) {
        if let Err(err) = self.event_sender.try_send(event) {
            error!(target: "network", "ckb protocol send event error : {:?}", err)
        }
    }
}

impl ServiceProtocol for CKBHandler {
    fn init(&mut self, _control: &mut ServiceContext) {}
    fn connected(&mut self, control: &mut ServiceContext, session: &SessionContext, version: &str) {
        let (peer_id, version) = {
            let parsed_version = version.parse::<u8>();
            if session.remote_pubkey.is_none() || parsed_version.is_err() {
                error!(target: "network", "ckb protocol connected error, addr: {}, protocol:{}, version: {}", session.address, self.id, version);
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
        debug!(target: "network", "ckb protocol connected, addr: {}, protocol: {}, version: {}, peer_id: {:?}", session.address, self.id, version, &peer_id);
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
        self.send_event(event);
    }

    fn disconnected(&mut self, _control: &mut ServiceContext, session: &SessionContext) {
        if let Some(peer_id) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
        {
            debug!(target: "network", "ckb protocol disconnect, addr: {}, protocol: {}, peer_id: {:?}", session.address, self.id, &peer_id);
            self.send_event(Event::Disconnected(peer_id, self.id));
        }
    }

    fn received(&mut self, _control: &mut ServiceContext, session: &SessionContext, data: Vec<u8>) {
        if let Some(peer_id) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
        {
            debug!(target: "network", "ckb protocol received, addr: {}, protocol: {}, peer_id: {:?}", session.address, self.id, &peer_id);
            self.send_event(Event::Received(peer_id, self.id, data));
        }
    }
    fn notify(&mut self, _control: &mut ServiceContext, token: u64) {
        self.send_event(Event::Notify(self.id, token));
    }
}
