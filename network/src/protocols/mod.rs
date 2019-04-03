pub(crate) mod discovery;
pub(crate) mod feeler;
pub(crate) mod identify;
pub(crate) mod outbound_peer;
pub(crate) mod ping;

use crate::{
    errors::{Error, PeerError},
    peer_store::{Behaviour, Status},
    peers_registry::RegisterResult,
    NetworkState, PeerIndex, ProtocolContext, ProtocolContextMutRef, ServiceControl, SessionInfo,
};
use bytes::Bytes;
use log::{debug, error, info, warn};
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

pub struct CKBProtocol {
    id: ProtocolId,
    // for example: b"/ckb/"
    protocol_name: String,
    // supported version, used to check protocol version
    supported_versions: Vec<ProtocolVersion>,
    handler: Box<dyn CKBProtocolHandler + Send + 'static>,
    network_state: Arc<NetworkState>,
}

impl CKBProtocol {
    pub fn new(
        protocol_name: String,
        id: ProtocolId,
        versions: &[ProtocolVersion],
        handler: Box<dyn CKBProtocolHandler + Send + 'static>,
        network_state: Arc<NetworkState>,
    ) -> Self {
        CKBProtocol {
            id,
            handler,
            network_state,
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

    pub fn protocol_name(&self) -> String {
        self.protocol_name.clone()
    }

    pub fn match_version(&self, version: ProtocolVersion) -> bool {
        self.supported_versions.contains(&version)
    }

    pub fn build(self) -> ProtocolMeta {
        let protocol_name = self.protocol_name();
        let supported_versions = self
            .supported_versions
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>();
        MetaBuilder::default()
            .id(self.id)
            .name(move |_| protocol_name.clone())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH)
                        .new_codec(),
                )
            })
            .support_versions(supported_versions)
            .service_handle(move || {
                let handler = CKBHandler::new(self.id, self.network_state, self.handler);
                ProtocolHandle::Callback(Box::new(handler))
            })
            .build()
    }
}

struct CKBHandler {
    id: ProtocolId,
    network_state: Arc<NetworkState>,
    handler: Box<dyn CKBProtocolHandler + Send + 'static>,
}

impl CKBHandler {
    pub fn new(
        id: ProtocolId,
        network_state: Arc<NetworkState>,
        handler: Box<dyn CKBProtocolHandler + Send + 'static>,
    ) -> CKBHandler {
        CKBHandler {
            id,
            network_state,
            handler,
        }
    }
}

impl ServiceProtocol for CKBHandler {
    fn init(&mut self, context: &mut ProtocolContext) {
        let context = Box::new(DefaultCKBProtocolContext::new(
            self.id,
            Arc::clone(&self.network_state),
            context.control().clone(),
        ));
        self.handler.initialize(context);
    }

    fn connected(&mut self, mut context: ProtocolContextMutRef, version: &str) {
        let network = &self.network_state;
        let session = context.session;
        let (peer_id, version) = {
            // TODO: version number should be discussed.
            let parsed_version = version.parse::<ProtocolVersion>().ok();
            if session.remote_pubkey.is_none() || parsed_version.is_none() {
                error!(
                    target: "network",
                    "ckb protocol connected error, addr: {}, protocol:{}, version: {}",
                    session.address,
                    self.id,
                    version,
                );
                context.disconnect(session.id);
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
        debug!(
            target: "network",
            "ckb protocol connected, addr: {}, protocol: {}, version: {}, peer_id: {:?}",
            session.address,
            self.id,
            version,
            &peer_id,
        );

        match network.accept_connection(
            peer_id.clone(),
            session.address.clone(),
            session.id,
            session.ty,
            self.id,
            version,
        ) {
            Ok(register_result) => {
                // update status in peer_store
                if let RegisterResult::New(_) = register_result {
                    let mut peer_store = network.peer_store().write();
                    peer_store.report(&peer_id, Behaviour::Connect);
                    peer_store.update_status(&peer_id, Status::Connected);
                }
                // call handler
                self.handler.connected(
                    Box::new(DefaultCKBProtocolContext::new(
                        self.id,
                        Arc::clone(&self.network_state),
                        context.control().clone(),
                    )),
                    register_result.peer_index(),
                )
            }
            Err(err) => {
                network.drop_peer(context.control(), &peer_id);
                info!(
                    target: "network",
                    "reject connection from {} {}, because {:?}",
                    peer_id.to_base58(),
                    session.address,
                    err,
                )
            }
        }
    }

    fn disconnected(&mut self, mut context: ProtocolContextMutRef) {
        let session = context.session;
        if let Some(peer_id) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
        {
            debug!(
                target: "network",
                "ckb protocol disconnect, addr: {}, protocol: {}, peer_id: {:?}",
                session.address,
                self.id,
                &peer_id,
            );

            let network = &self.network_state;
            // update disconnect in peer_store
            if let Some(peer_index) = network.get_peer_index(&peer_id) {
                // call handler
                self.handler.disconnected(
                    Box::new(DefaultCKBProtocolContext::new(
                        self.id,
                        Arc::clone(network),
                        context.control().clone(),
                    )),
                    peer_index,
                );
            }
        }
    }

    fn received(&mut self, mut context: ProtocolContextMutRef, data: Bytes) {
        let session = context.session;
        if let Some((peer_id, _peer_index)) = session
            .remote_pubkey
            .as_ref()
            .map(|pubkey| pubkey.peer_id())
            .and_then(|peer_id| {
                self.network_state
                    .get_peer_index(&peer_id)
                    .map(|peer_index| (peer_id, peer_index))
            })
        {
            debug!(
                target: "network",
                "ckb protocol received, addr: {}, protocol: {}, peer_id: {:?}",
                session.address,
                self.id,
                &peer_id,
            );

            let now = Instant::now();
            let network = &self.network_state;
            network.modify_peer(&peer_id, |peer| {
                peer.last_message_time = Some(now);
            });
            if let Some(peer_index) = network.get_peer_index(&peer_id) {
                self.handler.received(
                    Box::new(DefaultCKBProtocolContext::new(
                        self.id,
                        Arc::clone(network),
                        context.control().clone(),
                    )),
                    peer_index,
                    data,
                )
            }
        } else {
            warn!(target: "network", "can not get peer_id, disconnect it");
            context.disconnect(session.id);
        }
    }

    fn notify(&mut self, context: &mut ProtocolContext, token: u64) {
        let context = Box::new(DefaultCKBProtocolContext::new(
            self.id,
            Arc::clone(&self.network_state),
            context.control().clone(),
        ));
        self.handler.timer_triggered(context, token);
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
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error>;
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration);
    fn disconnect(&self, peer_index: PeerIndex);
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

pub(crate) struct DefaultCKBProtocolContext {
    pub protocol_id: ProtocolId,
    pub network_state: Arc<NetworkState>,
    pub p2p_control: ServiceControl,
}

impl DefaultCKBProtocolContext {
    pub fn new(
        protocol_id: ProtocolId,
        network_state: Arc<NetworkState>,
        p2p_control: ServiceControl,
    ) -> Self {
        DefaultCKBProtocolContext {
            protocol_id,
            network_state,
            p2p_control,
        }
    }
}

impl CKBProtocolContext for DefaultCKBProtocolContext {
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
            .read()
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
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error> {
        debug!(target: "network", "report peer {} behaviour: {:?}", peer_index, behaviour);
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            if self
                .network_state
                .peer_store()
                .write()
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
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration) {
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            self.network_state
                .ban_peer(&mut self.p2p_control.clone(), &peer_id, timeout)
        }
    }
    // disconnect from peer
    fn disconnect(&self, peer_index: PeerIndex) {
        debug!(target: "network", "disconnect peer {}", peer_index);
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            self.network_state
                .drop_peer(&mut self.p2p_control.clone(), &peer_id);
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
    fn initialize(&self, _nc: Box<dyn CKBProtocolContext>);
    fn received(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex, _data: Bytes);
    fn connected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex);
    fn disconnected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex);
    fn timer_triggered(&self, _nc: Box<dyn CKBProtocolContext>, _timer: u64) {}
}
