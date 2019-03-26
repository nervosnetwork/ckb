use crate::{PeerId, ServiceContext, SessionContext, SessionId, SessionType};
use bytes::Bytes;
use futures::sync::mpsc::Sender;
use log::{debug, error};
use p2p::{
    builder::MetaBuilder,
    multiaddr::Multiaddr,
    service::{ProtocolHandle, ProtocolMeta},
    traits::ServiceProtocol,
    ProtocolId,
};

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

    pub fn build(&self) -> ProtocolMeta {
        let event_sender = self.event_sender.clone();
        let supported_versions = self
            .supported_versions
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>();
        MetaBuilder::default()
            .id(self.id)
            .support_versions(supported_versions)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(CKBHandler {
                    id: self.id,
                    event_sender,
                }))
            })
            .build()
    }
}

#[derive(Debug)]
pub enum Event {
    Connected(
        PeerId,
        Multiaddr,
        SessionId,
        SessionType,
        ProtocolId,
        Version,
    ),
    Disconnected(PeerId, ProtocolId),
    Received(PeerId, ProtocolId, Bytes),
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
            // TODO: version number should be discussed.
            let parsed_version = version.parse::<u8>().ok();
            if session.remote_pubkey.is_none() || parsed_version.is_none() {
                error!(target: "network", "ckb protocol connected error, addr: {}, protocol:{}, version: {}", session.address, self.id, version);
                control.disconnect(session.id);
                return;
            }
            (
                session
                    .remote_pubkey
                    .as_ref()
                    .map(|pubkey| pubkey.peer_id())
                    .expect("remote_pubkey existence checked"),
                parsed_version.expect("parsed_version existence checked"),
            )
        };
        debug!(target: "network", "ckb protocol connected, addr: {}, protocol: {}, version: {}, peer_id: {:?}", session.address, self.id, version, &peer_id);
        let event = Event::Connected(
            peer_id,
            session.address.clone(),
            session.id,
            session.ty,
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

    fn received(&mut self, _control: &mut ServiceContext, session: &SessionContext, data: Bytes) {
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
