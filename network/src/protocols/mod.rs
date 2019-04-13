pub(crate) mod discovery;
pub(crate) mod feeler;
pub(crate) mod identify;

use crate::{
    errors::{Error, PeerError},
    peer_store::{Behaviour, Status},
    NetworkState, PeerIndex, ProtocolContext, ProtocolContextMutRef, ServiceControl, SessionInfo,
};
use bytes::Bytes;
use log::{debug, error, info, trace, warn};
use p2p::{
    builder::MetaBuilder,
    service::{ProtocolHandle, ProtocolMeta},
    traits::ServiceProtocol,
    ProtocolId,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::codec::length_delimited;

// Max message frame length: 20MB
const MAX_FRAME_LENGTH: usize = 20 * 1024 * 1024;

pub type ProtocolVersion = u32;

pub trait BackgroundService {
    fn handle(&mut self, network_state: &mut NetworkState);
    fn interval(&self) -> Duration;
}

pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    protocol_name: String,
    // supported version, used to check protocol version
    supported_versions: Vec<ProtocolVersion>,
    handler: Box<dyn CKBProtocolHandler + Send + 'static>,
}

impl CKBProtocol {
    pub fn new<F: Fn() -> Box<dyn CKBProtocolHandler> + Send + 'static>(
        protocol_name: String,
        id: ProtocolId,
        versions: &[ProtocolVersion],
        handler: Box<dyn CKBProtocolHandler + Send + 'static>,
    ) -> Self {
        CKBProtocol {
            id,
            handler,
            protocol_name: format!("/ckb/{}/", protocol_name).to_string(),
            supported_versions: {
                let mut versions: Vec<_> = versions.to_vec();
                versions.sort_by(|a, b| b.cmp(a));
                versions.to_vec()
            },
        }
    }

    pub fn id(&self) -> ProtocolId {
        self.id
    }
    pub fn handler(&self) -> &dyn CKBProtocolHandler {
        self.handler.as_ref()
    }

    pub fn protocol_name(&self) -> &str {
        &self.protocol_name
    }

    pub fn match_version(&self, version: ProtocolVersion) -> bool {
        self.supported_versions.contains(&version)
    }

    pub fn build(&self) -> ProtocolMeta {
        let protocol_name = self.protocol_name().to_owned();
        let supported_versions = self
            .supported_versions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        MetaBuilder::default()
            .id(self.id)
            .name(move |_| protocol_name.to_string())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH)
                        .new_codec(),
                )
            })
            .support_versions(supported_versions)
            .service_handle(move || ProtocolHandle::Event)
            .build()
    }
}

struct CKBHandler {
    id: ProtocolId,
    network_state: Arc<NetworkState>,
    handler: Box<dyn CKBProtocolHandler>,
}

impl CKBHandler {
    pub fn new(
        id: ProtocolId,
        network_state: Arc<NetworkState>,
        handler: Box<dyn CKBProtocolHandler>,
    ) -> CKBHandler {
        CKBHandler {
            id,
            network_state,
            handler,
        }
    }
}

pub trait CKBProtocolContext: Send {
    fn send(&mut self, peer_index: PeerIndex, data: Vec<u8>) -> Result<(), Error>;
    fn send_protocol(
        &mut self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
        data: Vec<u8>,
    ) -> Result<(), Error>;
    // TODO combinate this interface with peer score
    fn report_peer(&mut self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error>;
    fn ban_peer(&mut self, peer_index: PeerIndex, timeout: Duration);
    fn disconnect(&mut self, peer_index: PeerIndex);
    fn register_timer(&self, interval: Duration, token: u64);
    fn session_info(&self, peer_index: PeerIndex) -> Option<SessionInfo>;
    fn protocol_version(
        &self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
    ) -> Option<ProtocolVersion>;
    fn protocol_id(&self) -> ProtocolId;
    fn sessions(&self, peer_indexes: &[PeerIndex]) -> Vec<(PeerIndex, SessionInfo)> {
        peer_indexes
            .iter()
            .filter_map(|peer_index| {
                self.session_info(*peer_index)
                    .and_then(|session| Some((*peer_index, session)))
            })
            .collect()
    }
    fn connected_peers(&self) -> Vec<PeerIndex>;
}

pub(crate) struct DefaultCKBProtocolContext<'a> {
    pub protocol_id: ProtocolId,
    pub network_state: &'a mut NetworkState,
    pub p2p_control: ServiceControl,
}

impl<'a> DefaultCKBProtocolContext<'a> {
    pub fn new(
        protocol_id: ProtocolId,
        network_state: &'a mut NetworkState,
        p2p_control: ServiceControl,
    ) -> Self {
        DefaultCKBProtocolContext {
            protocol_id,
            network_state,
            p2p_control,
        }
    }
}

impl<'a> CKBProtocolContext for DefaultCKBProtocolContext<'a> {
    fn send(&mut self, peer_index: PeerIndex, data: Vec<u8>) -> Result<(), Error> {
        self.send_protocol(peer_index, self.protocol_id, data)
    }
    fn send_protocol(
        &mut self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        let peer_id = self
            .network_state
            .get_peer_id(peer_index)
            .ok_or_else(|| PeerError::IndexNotFound(peer_index))?;

        let session_id = self
            .network_state
            .peers_registry
            .get(&peer_id)
            .ok_or_else(|| PeerError::NotFound(peer_id.to_owned()))
            .and_then(|peer| {
                peer.protocol_version(protocol_id)
                    .ok_or_else(|| PeerError::ProtocolNotFound(peer_id.to_owned(), protocol_id))
                    .map(|_| peer.session_id)
            })?;

        self.p2p_control
            .send_message(session_id, protocol_id, data)
            .map_err(|_| {
                Error::P2P(format!(
                    "error send to peer {:?} protocol {}",
                    peer_id, protocol_id
                ))
            })
    }
    // report peer behaviour
    fn report_peer(&mut self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error> {
        debug!(target: "network", "report peer {} behaviour: {:?}", peer_index, behaviour);
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            if self
                .network_state
                .mut_peer_store()
                .report(&peer_id, behaviour)
                .is_banned()
            {
                self.disconnect(peer_index);
            }
            Ok(())
        } else {
            Err(Error::Peer(PeerError::IndexNotFound(peer_index)))
        }
    }

    // ban peer
    fn ban_peer(&mut self, peer_index: PeerIndex, timeout: Duration) {
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            self.network_state.ban_peer(&peer_id, timeout)
        }
    }
    // disconnect from peer
    fn disconnect(&mut self, peer_index: PeerIndex) {
        debug!(target: "network", "disconnect peer {}", peer_index);
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            self.network_state.disconnect_peer(&peer_id);
        }
    }

    fn register_timer(&self, interval: Duration, token: u64) {
        // TODO: handle channel is full
        if let Err(err) =
            self.p2p_control
                .clone()
                .set_service_notify(self.protocol_id, interval, token)
        {
            error!(target: "network", "register timer error: {:?}", err);
        }
    }

    fn session_info(&self, peer_index: PeerIndex) -> Option<SessionInfo> {
        if let Some(session) = self
            .network_state
            .get_peer_id(peer_index)
            .map(|peer_id| self.network_state.session_info(&peer_id, self.protocol_id))
        {
            session
        } else {
            None
        }
    }

    fn protocol_version(
        &self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
    ) -> Option<ProtocolVersion> {
        if let Some(protocol_version) = self.network_state.get_peer_id(peer_index).map(|peer_id| {
            self.network_state
                .peer_protocol_version(&peer_id, protocol_id)
        }) {
            protocol_version
        } else {
            None
        }
    }

    fn protocol_id(&self) -> ProtocolId {
        self.protocol_id
    }

    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.network_state.peers_indexes()
    }
}

pub trait CKBProtocolHandler: Sync + Send {
    // TODO: Remove (_service: &mut ServiceContext) argument later
    fn initialize(&self, _nc: &mut dyn CKBProtocolContext);
    fn received(&self, _nc: &mut dyn CKBProtocolContext, _peer: PeerIndex, _data: Bytes);
    fn connected(&self, _nc: &mut dyn CKBProtocolContext, _peer: PeerIndex);
    fn disconnected(&self, _nc: &mut dyn CKBProtocolContext, _peer: PeerIndex);
    fn timer_triggered(&self, _nc: &mut dyn CKBProtocolContext, _timer: u64) {}
}
