use crate::errors::Error;
use crate::peer_registry::{ConnectionStatus, PeerRegistry};
use crate::peer_store::{
    types::{BannedAddr, MultiaddrExt},
    PeerStore,
};
use crate::protocols::{
    disconnect_message::DisconnectMessageProtocol,
    discovery::{DiscoveryProtocol, DiscoveryService},
    feeler::Feeler,
    identify::IdentifyCallback,
    ping::PingService,
};
use crate::services::{
    dns_seeding::DnsSeedingService, dump_peer_store::DumpPeerStoreService,
    outbound_peer::OutboundPeerService, protocol_type_checker::ProtocolTypeCheckerService,
};
use crate::{
    Behaviour, CKBProtocol, NetworkConfig, Peer, ProtocolId, ProtocolVersion, PublicKey,
    ServiceControl, MAX_FRAME_LENGTH_DISCONNECTMSG, MAX_FRAME_LENGTH_DISCOVERY,
    MAX_FRAME_LENGTH_FEELER, MAX_FRAME_LENGTH_IDENTIFY, MAX_FRAME_LENGTH_PING,
};
use ckb_build_info::Version;
use ckb_logger::{debug, error, info, trace, warn};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_util::{Condvar, Mutex, RwLock};
use futures::{
    channel::{
        mpsc::{self, channel},
        oneshot,
    },
    Future, StreamExt,
};
use ipnetwork::IpNetwork;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    bytes::Bytes,
    context::{ServiceContext, SessionContext},
    error::Error as P2pError,
    multiaddr::{self, Multiaddr},
    secio::{self, PeerId},
    service::{
        ProtocolEvent, ProtocolHandle, Service, ServiceError, ServiceEvent, TargetProtocol,
        TargetSession,
    },
    traits::ServiceHandle,
    utils::extract_peer_id,
    SessionId,
};
use p2p_identify::IdentifyProtocol;
use p2p_ping::PingHandler;
use std::{
    cmp::max,
    collections::{HashMap, HashSet},
    io,
    pin::Pin,
    sync::Arc,
    thread,
    time::{Duration, Instant},
    usize,
};
use tokio::runtime;
use tokio_util::codec::length_delimited;

pub(crate) const PING_PROTOCOL_ID: usize = 0;
pub(crate) const DISCOVERY_PROTOCOL_ID: usize = 1;
pub(crate) const IDENTIFY_PROTOCOL_ID: usize = 2;
pub(crate) const FEELER_PROTOCOL_ID: usize = 3;
pub(crate) const DISCONNECT_MESSAGE_PROTOCOL_ID: usize = 4;

const P2P_SEND_TIMEOUT: Duration = Duration::from_secs(6);
const P2P_TRY_SEND_INTERVAL: Duration = Duration::from_millis(100);
// After 5 minutes we consider this dial hang
const DIAL_HANG_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<ProtocolVersion>,
}

pub struct NetworkState {
    pub(crate) peer_registry: RwLock<PeerRegistry>,
    pub(crate) peer_store: Mutex<PeerStore>,
    /// Node listened addresses
    pub(crate) listened_addrs: RwLock<Vec<Multiaddr>>,
    dialing_addrs: RwLock<HashMap<PeerId, Instant>>,

    pub(crate) protocol_ids: RwLock<HashSet<ProtocolId>>,
    /// Node public addresses,
    /// includes manually public addrs and remote peer observed addrs
    public_addrs: RwLock<HashMap<Multiaddr, u8>>,
    pending_observed_addrs: RwLock<HashSet<Multiaddr>>,
    /// Send disconnect message but not disconnected yet
    disconnecting_sessions: RwLock<HashSet<SessionId>>,
    local_private_key: secio::SecioKeyPair,
    local_peer_id: PeerId,
    bootnodes: Vec<(PeerId, Multiaddr)>,
    pub(crate) config: NetworkConfig,
}

impl NetworkState {
    pub fn from_config(config: NetworkConfig) -> Result<NetworkState, Error> {
        config.create_dir_if_not_exists()?;
        let local_private_key = config.fetch_private_key()?;
        // set max score to public addresses
        let public_addrs: HashMap<Multiaddr, u8> = config
            .listen_addresses
            .iter()
            .chain(config.public_addresses.iter())
            .map(|addr| (addr.to_owned(), std::u8::MAX))
            .collect();
        let peer_store = Mutex::new(PeerStore::load_from_dir(config.peer_store_path())?);
        let bootnodes = config.bootnodes()?;

        let whitelist_peers = config
            .whitelist_peers()?
            .iter()
            .map(|(peer_id, _)| peer_id.to_owned())
            .collect::<Vec<_>>();
        let peer_registry = PeerRegistry::new(
            config.max_inbound_peers(),
            config.max_outbound_peers(),
            config.whitelist_only,
            whitelist_peers,
        );

        Ok(NetworkState {
            peer_store,
            config,
            bootnodes,
            peer_registry: RwLock::new(peer_registry),
            dialing_addrs: RwLock::new(HashMap::default()),
            public_addrs: RwLock::new(public_addrs),
            listened_addrs: RwLock::new(Vec::new()),
            pending_observed_addrs: RwLock::new(HashSet::default()),
            disconnecting_sessions: RwLock::new(HashSet::default()),
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.public_key().peer_id(),
            protocol_ids: RwLock::new(HashSet::default()),
        })
    }

    pub(crate) fn report_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        behaviour: Behaviour,
    ) {
        if let Some(peer_id) = self.with_peer_registry(|reg| {
            reg.get_peer(session_id)
                .filter(|peer| !peer.is_whitelist)
                .map(|peer| peer.peer_id.clone())
        }) {
            self.report_peer(p2p_control, &peer_id, behaviour);
        } else {
            debug!(
                "Report {} failed: not in peer registry or it is in the whitelist",
                session_id
            );
        }
    }

    fn report_peer(&self, p2p_control: &ServiceControl, peer_id: &PeerId, behaviour: Behaviour) {
        trace!("report {:?} because {:?}", peer_id, behaviour);
        let report_result = match self.peer_store.lock().report(peer_id, behaviour) {
            Ok(result) => result,
            Err(err) => {
                error!(
                    "Report failed peer_id: {:?} behaviour: {:?} error: {:?}",
                    peer_id, behaviour, err
                );
                return;
            }
        };
        if report_result.is_banned() {
            info!("peer {:?} banned", peer_id);
            if let Some(session_id) = self.peer_registry.read().get_key_by_peer_id(peer_id) {
                if let Err(err) = disconnect_with_message(p2p_control, session_id, "banned") {
                    debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                }
            }
        }
    }

    pub(crate) fn ban_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        duration: Duration,
        reason: String,
    ) {
        if let Some(peer_id) = self.with_peer_registry(|reg| {
            reg.get_peer(session_id)
                .filter(|peer| !peer.is_whitelist)
                .map(|peer| peer.peer_id.clone())
        }) {
            self.ban_peer(p2p_control, &peer_id, duration, reason);
        } else {
            debug!(
                "Ban session({}) failed: not in peer registry or it is in the whitelist",
                session_id
            );
        }
    }

    fn ban_peer(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        duration: Duration,
        reason: String,
    ) {
        info!(
            "Ban peer {:?} for {} seconds, reason: {}",
            peer_id,
            duration.as_secs(),
            reason
        );
        let peer_opt = self.with_peer_registry_mut(|reg| reg.remove_peer_by_peer_id(peer_id));
        if let Some(peer) = peer_opt {
            if let Err(err) = self.peer_store.lock().ban_addr(
                &peer.connected_addr,
                duration.as_millis() as u64,
                reason,
            ) {
                debug!("Failed to ban peer {:?} {:?}", err, peer);
            }
            let message = format!("Ban for {} seconds", duration.as_secs());
            if let Err(err) =
                disconnect_with_message(p2p_control, peer.session_id, message.as_str())
            {
                debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
            }
        }
    }

    pub(crate) fn query_session_id(&self, peer_id: &PeerId) -> Option<SessionId> {
        self.with_peer_registry(|registry| registry.get_key_by_peer_id(peer_id))
    }

    pub(crate) fn accept_peer(
        &self,
        session_context: &SessionContext,
    ) -> Result<Option<Peer>, Error> {
        let peer_id = session_context
            .remote_pubkey
            .as_ref()
            .map(PublicKey::peer_id)
            .expect("Secio must enabled");

        // NOTE: be careful, here easy cause a deadlock,
        //    because peer_store's lock scope across peer_registry's lock scope
        let mut peer_store = self.peer_store.lock();
        let accept_peer_result = {
            self.peer_registry.write().accept_peer(
                peer_id,
                session_context.address.clone(),
                session_context.id,
                session_context.ty,
                &mut peer_store,
            )
        };
        accept_peer_result.map_err(Into::into)
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_registry<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&PeerRegistry) -> T,
    {
        callback(&self.peer_registry.read())
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_registry_mut<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&mut PeerRegistry) -> T,
    {
        callback(&mut self.peer_registry.write())
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_store_mut<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&mut PeerStore) -> T,
    {
        callback(&mut self.peer_store.lock())
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
    }

    pub fn node_id(&self) -> String {
        self.local_private_key().peer_id().to_base58()
    }

    pub(crate) fn public_addrs(&self, count: usize) -> Vec<(Multiaddr, u8)> {
        self.public_addrs
            .read()
            .iter()
            .take(count)
            .map(|(addr, score)| (addr.to_owned(), *score))
            .collect()
    }

    pub(crate) fn vote_listened_addr(&self, addr: Multiaddr, votes: u8) {
        let mut public_addrs = self.public_addrs.write();
        let score = public_addrs.entry(addr).or_default();
        *score = score.saturating_add(votes);
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        self.peer_registry.read().connection_status()
    }

    pub fn public_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        let listened_addrs = self.listened_addrs.read();
        self.public_addrs(max_urls.saturating_sub(listened_addrs.len()))
            .into_iter()
            .filter(|(addr, _)| !listened_addrs.contains(addr))
            .chain(listened_addrs.iter().map(|addr| (addr.to_owned(), 1)))
            .map(|(addr, score)| (self.to_external_url(&addr), score))
            .collect()
    }

    pub(crate) fn add_node(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        address: Multiaddr,
    ) {
        self.dial_identify(p2p_control, peer_id, address);
    }

    fn to_external_url(&self, addr: &Multiaddr) -> String {
        format!("{}/p2p/{}", addr, self.node_id())
    }

    pub fn get_protocol_ids<F: Fn(ProtocolId) -> bool>(&self, filter: F) -> Vec<ProtocolId> {
        self.protocol_ids
            .read()
            .iter()
            .filter(|id| filter(**id))
            .cloned()
            .collect::<Vec<_>>()
    }

    pub(crate) fn can_dial(
        &self,
        peer_id: &PeerId,
        addr: &Multiaddr,
        allow_dial_to_self: bool,
    ) -> bool {
        if !allow_dial_to_self && self.local_peer_id() == peer_id {
            trace!("Do not dial self: {:?}, {}", peer_id, addr);
            return false;
        }
        if self.public_addrs.read().contains_key(&addr) {
            trace!(
                "Do not dial listened address(self): {:?}, {}",
                peer_id,
                addr
            );
            return false;
        }

        let peer_in_registry = self.with_peer_registry(|reg| {
            reg.get_key_by_peer_id(peer_id).is_some() || reg.is_feeler(peer_id)
        });
        if peer_in_registry {
            trace!("Do not dial peer in registry: {:?}, {}", peer_id, addr);
            return false;
        }

        if let Some(dial_started) = self.dialing_addrs.read().get(peer_id) {
            trace!(
                "Do not repeat send dial command to network service: {:?}, {}",
                peer_id,
                addr
            );
            if dial_started.elapsed() > DIAL_HANG_TIMEOUT {
                use sentry::{capture_message, with_scope, Level};
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-network", "dialing-timeout"])),
                    || {
                        capture_message(
                            &format!(
                                "Dialing {:?}, {:?} for more than {} seconds, \
                                 something is wrong in network service",
                                peer_id,
                                addr,
                                DIAL_HANG_TIMEOUT.as_secs(),
                            ),
                            Level::Warning,
                        )
                    },
                );
            }
            return false;
        }

        true
    }

    pub(crate) fn dial_success(&self, peer_id: &PeerId) {
        self.dialing_addrs.write().remove(peer_id);
    }

    pub(crate) fn dial_failed(&self, peer_id: PeerId) {
        self.with_peer_registry_mut(|reg| {
            reg.remove_feeler(&peer_id);
        });
        self.dialing_addrs.write().remove(&peer_id);
    }

    /// Dial
    /// return value indicates the dialing is actually sent or denied.
    fn dial_inner(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        addr: Multiaddr,
        target: TargetProtocol,
        allow_dial_to_self: bool,
    ) -> Result<(), Error> {
        if !self.can_dial(peer_id, &addr, allow_dial_to_self) {
            return Err(Error::Dial(format!(
                "ignore dialing peer_id {:?}, addr {}",
                peer_id, addr
            )));
        }

        let addr = addr.attach_p2p(peer_id)?;
        debug!("dialing {} with {:?}", addr, target);
        p2p_control.dial(addr, target)?;
        self.dialing_addrs
            .write()
            .insert(peer_id.to_owned(), Instant::now());
        Ok(())
    }

    /// Dial just identify protocol
    pub fn dial_identify(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        if let Err(err) = self.dial_inner(
            p2p_control,
            peer_id,
            addr,
            TargetProtocol::Single(IDENTIFY_PROTOCOL_ID.into()),
            false,
        ) {
            debug!("dial_identify error: {}", err);
        }
    }

    /// Dial just feeler protocol
    pub fn dial_feeler(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        if let Err(err) = self.dial_inner(
            p2p_control,
            peer_id,
            addr,
            TargetProtocol::Single(IDENTIFY_PROTOCOL_ID.into()),
            false,
        ) {
            debug!("dial_feeler error {}", err);
        } else {
            self.with_peer_registry_mut(|reg| {
                reg.add_feeler(peer_id.clone());
            });
        }
    }

    /// this method is intent to check observed addr by dial to self
    pub(crate) fn try_dial_observed_addrs(&self, p2p_control: &ServiceControl) {
        let mut pending_observed_addrs = self.pending_observed_addrs.write();
        for addr in pending_observed_addrs.drain() {
            trace!("try dial observed addr: {:?}", addr);
            if let Err(err) = self.dial_inner(
                p2p_control,
                self.local_peer_id(),
                addr,
                TargetProtocol::Single(IDENTIFY_PROTOCOL_ID.into()),
                true,
            ) {
                debug!("try_dial_observed_addrs error {}", err);
            }
        }
    }

    pub fn add_observed_addrs(&self, iter: impl Iterator<Item = Multiaddr>) {
        let mut public_addrs = self.public_addrs.write();
        let mut pending_observed_addrs = self.pending_observed_addrs.write();
        for addr in iter {
            if let Some(score) = public_addrs.get_mut(&addr) {
                *score = score.saturating_add(1);
                trace!(
                    "increase score for exists observed addr: {:?} {}",
                    addr,
                    score
                );
            } else {
                trace!("pending observed addr: {:?}", addr,);
                pending_observed_addrs.insert(addr);
            }
        }
    }
}

pub struct EventHandler {
    pub(crate) network_state: Arc<NetworkState>,
    pub(crate) exit_condvar: Arc<(Mutex<()>, Condvar)>,
}

impl EventHandler {
    fn inbound_eviction(&self, context: &mut ServiceContext) {
        if self.network_state.config.bootnode_mode {
            let status = self.network_state.connection_status();

            if status.max_inbound <= status.non_whitelist_inbound.saturating_add(10) {
                for (index, peer) in self
                    .network_state
                    .with_peer_registry(|registry| {
                        registry
                            .peers()
                            .values()
                            .filter(|peer| peer.is_inbound() && !peer.is_whitelist)
                            .map(|peer| peer.session_id)
                            .collect::<Vec<SessionId>>()
                    })
                    .into_iter()
                    .enumerate()
                {
                    if index & 0x1 != 0 {
                        if let Err(err) = disconnect_with_message(
                            context.control(),
                            peer,
                            "bootnode random eviction",
                        ) {
                            debug!("Inbound eviction failed {:?}, error: {:?}", peer, err);
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, context: &mut ServiceContext, error: ServiceError) {
        match error {
            ServiceError::DialerError {
                ref address,
                ref error,
            } => {
                debug!("DialerError({}) {}", address, error);
                if error == &P2pError::ConnectSelf {
                    debug!("dial observed address success: {:?}", address);
                    let addr = address
                        .iter()
                        .filter(|proto| match proto {
                            multiaddr::Protocol::P2P(_) => false,
                            _ => true,
                        })
                        .collect();
                    self.network_state.vote_listened_addr(addr, 1);
                }
                let peer_id = extract_peer_id(address).expect("Secio must enabled");
                self.network_state.dial_failed(peer_id);
            }
            ServiceError::ProtocolError {
                id,
                proto_id,
                error,
            } => {
                debug!("ProtocolError({}, {}) {}", id, proto_id, error);
                let message = format!("ProtocolError id={}", proto_id);
                // Ban because misbehave of remote peer
                if let P2pError::IoError(_) = error {
                    self.network_state.ban_session(
                        &context.control(),
                        id,
                        Duration::from_secs(300),
                        message,
                    );
                } else if let Err(err) =
                    disconnect_with_message(context.control(), id, message.as_str())
                {
                    debug!("Disconnect failed {:?}, error {:?}", id, err);
                }
            }
            ServiceError::SessionTimeout { session_context } => {
                warn!(
                    "SessionTimeout({}, {})",
                    session_context.id, session_context.address,
                );
            }
            ServiceError::MuxerError {
                session_context,
                error,
            } => {
                debug!(
                    "MuxerError({}, {}), substream error {}, disconnect it",
                    session_context.id, session_context.address, error,
                );
            }
            ServiceError::ListenError { address, error } => {
                debug!("ListenError: address={:?}, error={:?}", address, error);
            }
            ServiceError::ProtocolSelectError {
                proto_name,
                session_context,
            } => {
                debug!(
                    "ProtocolSelectError: proto_name={:?}, session_id={}",
                    proto_name, session_context.id,
                );
            }
            ServiceError::SessionBlocked { session_context } => {
                debug!("SessionBlocked: {}", session_context.id);
            }
            ServiceError::ProtocolHandleError { proto_id, error } => {
                debug!("ProtocolHandleError: {:?}, proto_id: {}", error, proto_id);
                use sentry::{capture_message, with_scope, Level};
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-network", "p2p-service-error"])),
                    || {
                        capture_message(
                            &format!("ProtocolHandleError: {:?}, proto_id: {}", error, proto_id),
                            Level::Warning,
                        )
                    },
                );

                if let P2pError::SessionProtoHandleAbnormallyClosed(id) = error {
                    self.network_state.ban_session(
                        &context.control(),
                        id,
                        Duration::from_secs(300),
                        format!("protocol {} panic when process peer message", proto_id),
                    );
                }
                self.exit_condvar.1.notify_all();
            }
        }
    }

    fn handle_event(&mut self, context: &mut ServiceContext, event: ServiceEvent) {
        // When session disconnect update status anyway
        match event {
            ServiceEvent::SessionOpen { session_context } => {
                debug!(
                    "SessionOpen({}, {})",
                    session_context.id, session_context.address,
                );
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(PublicKey::peer_id)
                    .expect("Secio must enabled");

                self.network_state.dial_success(&peer_id);

                self.inbound_eviction(context);

                if self
                    .network_state
                    .with_peer_registry(|reg| reg.is_feeler(&peer_id))
                {
                    debug!(
                        "feeler connected {} => {}",
                        session_context.id, session_context.address,
                    );
                } else {
                    match self.network_state.accept_peer(&session_context) {
                        Ok(Some(evicted_peer)) => {
                            debug!(
                                "evict peer (disonnect it), {} => {}",
                                evicted_peer.session_id, evicted_peer.connected_addr,
                            );
                            if let Err(err) = disconnect_with_message(
                                context.control(),
                                evicted_peer.session_id,
                                "evict because accepted better peer",
                            ) {
                                debug!(
                                    "Disconnect failed {:?}, error: {:?}",
                                    evicted_peer.session_id, err
                                );
                            }
                        }
                        Ok(None) => debug!(
                            "{} open, registry {} success",
                            session_context.id, session_context.address,
                        ),
                        Err(err) => {
                            debug!(
                                "registry peer failed {:?} disconnect it, {} => {}",
                                err, session_context.id, session_context.address,
                            );
                            if let Err(err) = disconnect_with_message(
                                context.control(),
                                session_context.id,
                                "reject peer connection",
                            ) {
                                debug!(
                                    "Disconnect failed {:?}, error: {:?}",
                                    session_context.id, err
                                );
                            }
                        }
                    }
                }
            }
            ServiceEvent::SessionClose { session_context } => {
                debug!(
                    "SessionClose({}, {})",
                    session_context.id, session_context.address,
                );
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(PublicKey::peer_id)
                    .expect("Secio must enabled");

                self.network_state
                    .disconnecting_sessions
                    .write()
                    .remove(&session_context.id);
                let peer_exists = self
                    .network_state
                    .peer_registry
                    .write()
                    .remove_peer(session_context.id)
                    .is_some();
                if peer_exists {
                    debug!(
                        "{} closed, remove {} from peer_registry",
                        session_context.id, session_context.address,
                    );
                    self.network_state.with_peer_store_mut(|peer_store| {
                        peer_store.remove_disconnected_peer(&peer_id);
                    })
                }
            }
            _ => {
                info!("p2p service event: {:?}", event);
            }
        }
    }

    fn handle_proto(&mut self, context: &mut ServiceContext, event: ProtocolEvent) {
        // For special protocols: ping/discovery/identify/disconnect_message
        match event {
            ProtocolEvent::Connected {
                session_context,
                proto_id,
                version,
            } => {
                let peer_not_exists = self.network_state.with_peer_registry_mut(|reg| {
                    reg.get_peer_mut(session_context.id)
                        .map(|peer| {
                            peer.protocols.insert(proto_id, version);
                        })
                        .is_none()
                });
                if peer_not_exists {
                    let peer_id = session_context
                        .remote_pubkey
                        .as_ref()
                        .map(PublicKey::peer_id)
                        .expect("Secio must enabled");
                    if !self
                        .network_state
                        .with_peer_registry(|reg| reg.is_feeler(&peer_id))
                    {
                        warn!(
                            "Invalid session {}, protocol id {}",
                            session_context.id, proto_id,
                        );
                    }
                }
            }
            ProtocolEvent::Disconnected {
                session_context,
                proto_id,
            } => {
                self.network_state.with_peer_registry_mut(|reg| {
                    let _ = reg.get_peer_mut(session_context.id).map(|peer| {
                        peer.protocols.remove(&proto_id);
                    });
                });
            }
            ProtocolEvent::Received {
                session_context, ..
            } => {
                let session_id = session_context.id;
                let peer_not_exists = self.network_state.with_peer_registry_mut(|reg| {
                    reg.get_peer_mut(session_id)
                        .map(|peer| {
                            peer.last_message_time = Some(Instant::now());
                        })
                        .is_none()
                });
                if peer_not_exists
                    && !self
                        .network_state
                        .disconnecting_sessions
                        .read()
                        .contains(&session_id)
                {
                    debug!(
                        "disconnect peer({}) already removed from registry",
                        session_context.id
                    );
                    self.network_state
                        .disconnecting_sessions
                        .write()
                        .insert(session_id);
                    if let Err(err) = disconnect_with_message(
                        context.control(),
                        session_id,
                        "already removed from registry",
                    ) {
                        debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                    }
                }
            }
        }
    }
}

pub struct NetworkService {
    p2p_service: Service<EventHandler>,
    network_state: Arc<NetworkState>,
    // Background services
    bg_services: Vec<Pin<Box<dyn Future<Output = ()> + 'static + Send>>>,
}

impl NetworkService {
    pub fn new(
        network_state: Arc<NetworkState>,
        protocols: Vec<CKBProtocol>,
        required_protocol_ids: Vec<ProtocolId>,
        name: String,
        client_version: String,
        exit_condvar: Arc<(Mutex<()>, Condvar)>,
    ) -> NetworkService {
        let config = &network_state.config;

        // == Build special protocols

        // TODO: how to deny banned node to open those protocols?
        // Ping protocol
        let (ping_sender, ping_receiver) = channel(std::u8::MAX as usize);
        let ping_interval = Duration::from_secs(config.ping_interval_secs);
        let ping_timeout = Duration::from_secs(config.ping_timeout_secs);

        let ping_meta = MetaBuilder::default()
            .id(PING_PROTOCOL_ID.into())
            .name(move |_| "/ckb/ping".to_string())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH_PING)
                        .new_codec(),
                )
            })
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(PingHandler::new(
                    ping_interval,
                    ping_timeout,
                    ping_sender,
                )))
            })
            .build();

        // Discovery protocol
        let (disc_sender, disc_receiver) = mpsc::unbounded();
        let disc_meta = MetaBuilder::default()
            .id(DISCOVERY_PROTOCOL_ID.into())
            .name(move |_| "/ckb/discovery".to_string())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH_DISCOVERY)
                        .new_codec(),
                )
            })
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(
                    DiscoveryProtocol::new(disc_sender.clone())
                        .global_ip_only(!config.discovery_local_address),
                ))
            })
            .build();

        // Identify protocol
        let identify_callback =
            IdentifyCallback::new(Arc::clone(&network_state), name, client_version);
        let identify_meta = MetaBuilder::default()
            .id(IDENTIFY_PROTOCOL_ID.into())
            .name(move |_| "/ckb/identify".to_string())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH_IDENTIFY)
                        .new_codec(),
                )
            })
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(IdentifyProtocol::new(identify_callback)))
            })
            .build();

        // Feeler protocol
        // TODO: versions
        let feeler_meta = MetaBuilder::default()
            .id(FEELER_PROTOCOL_ID.into())
            .name(move |_| "/ckb/flr".to_string())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH_FEELER)
                        .new_codec(),
                )
            })
            .service_handle({
                let network_state = Arc::clone(&network_state);
                move || ProtocolHandle::Both(Box::new(Feeler::new(Arc::clone(&network_state))))
            })
            .build();

        let disconnect_message_meta = MetaBuilder::default()
            .id(DISCONNECT_MESSAGE_PROTOCOL_ID.into())
            .name(move |_| "/ckb/disconnectmsg".to_string())
            .codec(|| {
                Box::new(
                    length_delimited::Builder::new()
                        .max_frame_length(MAX_FRAME_LENGTH_DISCONNECTMSG)
                        .new_codec(),
                )
            })
            .service_handle(move || ProtocolHandle::Both(Box::new(DisconnectMessageProtocol)))
            .build();

        // == Build p2p service struct
        let mut protocol_metas = protocols
            .into_iter()
            .map(CKBProtocol::build)
            .collect::<Vec<_>>();
        protocol_metas.push(feeler_meta);
        protocol_metas.push(disconnect_message_meta);
        protocol_metas.push(ping_meta);
        protocol_metas.push(disc_meta);
        protocol_metas.push(identify_meta);

        let mut service_builder = ServiceBuilder::default();
        for meta in protocol_metas.into_iter() {
            network_state.protocol_ids.write().insert(meta.id());
            service_builder = service_builder.insert_protocol(meta);
        }
        let event_handler = EventHandler {
            network_state: Arc::clone(&network_state),
            exit_condvar,
        };
        let p2p_service = service_builder
            .key_pair(network_state.local_private_key.clone())
            .upnp(config.upnp)
            .forever(true)
            .max_connection_number(1024)
            .build(event_handler);

        // == Build background service tasks
        let disc_service = DiscoveryService::new(
            Arc::clone(&network_state),
            disc_receiver,
            config.discovery_local_address,
        );
        let mut ping_service = PingService::new(
            Arc::clone(&network_state),
            p2p_service.control().to_owned(),
            ping_receiver,
        );
        let dump_peer_store_service = DumpPeerStoreService::new(Arc::clone(&network_state));
        let protocol_type_checker_service = ProtocolTypeCheckerService::new(
            Arc::clone(&network_state),
            p2p_service.control().to_owned(),
            required_protocol_ids,
        );
        let mut bg_services = vec![
            Box::pin(async move {
                loop {
                    if ping_service.next().await.is_none() {
                        break;
                    }
                }
            }) as Pin<Box<_>>,
            Box::pin(disc_service) as Pin<Box<_>>,
            Box::pin(dump_peer_store_service) as Pin<Box<_>>,
            Box::pin(protocol_type_checker_service) as Pin<Box<_>>,
        ];
        if config.outbound_peer_service_enabled() {
            let outbound_peer_service = OutboundPeerService::new(
                Arc::clone(&network_state),
                p2p_service.control().to_owned(),
                Duration::from_secs(config.connect_outbound_interval_secs),
            );
            bg_services.push(Box::pin(outbound_peer_service) as Pin<Box<_>>);
        };

        if config.dns_seeding_service_enabled() {
            let dns_seeding_service =
                DnsSeedingService::new(Arc::clone(&network_state), config.dns_seeds.clone());
            bg_services.push(Box::pin(dns_seeding_service) as Pin<Box<_>>);
        };

        NetworkService {
            p2p_service,
            network_state,
            bg_services,
        }
    }

    pub fn start<S: ToString>(
        self,
        node_version: Version,
        thread_name: Option<S>,
    ) -> Result<NetworkController, Error> {
        let config = self.network_state.config.clone();

        // dial whitelist_nodes
        for (peer_id, addr) in self.network_state.config.whitelist_peers()? {
            debug!("dial whitelist_peers {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), &peer_id, addr);
        }

        // get bootnodes
        // try get addrs from peer_store, if peer_store have no enough addrs then use bootnodes
        let bootnodes = self.network_state.with_peer_store_mut(|peer_store| {
            let count = max((config.max_outbound_peers >> 1) as usize, 1);
            let mut addrs: Vec<_> = peer_store
                .fetch_addrs_to_attempt(count)
                .into_iter()
                .map(|paddr| (paddr.peer_id, paddr.addr))
                .collect();
            addrs.extend(
                self.network_state
                    .bootnodes
                    .iter()
                    .take(count.saturating_sub(addrs.len()))
                    .cloned(),
            );
            addrs
        });

        // dial half bootnodes
        for (peer_id, addr) in bootnodes {
            debug!("dial bootnode {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), &peer_id, addr);
        }

        let p2p_control = self.p2p_service.control().to_owned();
        let network_state = Arc::clone(&self.network_state);

        // Mainly for test: give an empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let (start_sender, start_receiver) = crossbeam_channel::bounded(1);
        let network_state_1 = Arc::clone(&network_state);
        // Main network thread
        let thread = thread_builder
            .spawn(move || {
                let inner_p2p_control = self.p2p_service.control().to_owned();
                let num_threads = max(num_cpus::get(), 4);
                let network_state = Arc::clone(&network_state_1);
                let mut p2p_service = self.p2p_service;
                let mut runtime = runtime::Builder::new()
                    .core_threads(num_threads)
                    .enable_all()
                    .threaded_scheduler()
                    .thread_name("NetworkRuntime-")
                    .build()
                    .expect("Network tokio runtime init failed");
                let handle = runtime.spawn(async move {
                    // listen local addresses
                    for addr in &config.listen_addresses {
                        match p2p_service.listen(addr.to_owned()).await {
                            Ok(listen_address) => {
                                info!(
                                    "Listen on address: {}",
                                    network_state_1.to_external_url(&listen_address)
                                );
                                network_state_1
                                    .listened_addrs
                                    .write()
                                    .push(listen_address.clone());
                            }
                            Err(err) => {
                                warn!(
                                    "listen on address {} failed, due to error: {}",
                                    addr.clone(),
                                    err
                                );
                                start_sender.send(Err(Error::Io(err))).unwrap();
                                return;
                            }
                        };
                    }
                    start_sender.send(Ok(())).unwrap();
                    loop {
                        if p2p_service.next().await.is_none() {
                            break;
                        }
                    }
                });

                // NOTE: for ensure background task finished
                let bg_signals = self
                    .bg_services
                    .into_iter()
                    .map(|bg_service| {
                        let (signal_sender, signal_receiver) = oneshot::channel::<()>();
                        let task = futures::future::select(bg_service, signal_receiver);
                        runtime.spawn(task);
                        signal_sender
                    })
                    .collect::<Vec<_>>();

                debug!("receiving shutdown signal ...");

                // Recevied stop signal, doing cleanup
                let _ = receiver.recv();
                for peer in network_state.peer_registry.read().peers().values() {
                    info!("Disconnect peer {}", peer.connected_addr);
                    if let Err(err) =
                        disconnect_with_message(&inner_p2p_control, peer.session_id, "shutdown")
                    {
                        debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
                    }
                }
                // Drop senders to stop all corresponding background task
                drop(bg_signals);
                if let Err(err) = inner_p2p_control.shutdown() {
                    warn!("send shutdown message to p2p error: {:?}", err);
                }

                debug!("Waiting tokio runtime to finish ...");
                runtime.block_on(handle).unwrap();
                debug!("Shutdown network service finished!");
            })
            .expect("Start NetworkService failed");

        if let Ok(Err(e)) = start_receiver.recv() {
            return Err(e);
        }

        let stop = StopHandler::new(SignalSender::Crossbeam(sender), thread);
        Ok(NetworkController {
            node_version,
            network_state,
            p2p_control,
            stop,
        })
    }
}

#[derive(Clone)]
pub struct NetworkController {
    node_version: Version,
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    stop: StopHandler<()>,
}

impl NetworkController {
    pub fn public_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        self.network_state.public_urls(max_urls)
    }

    pub fn node_version(&self) -> &Version {
        &self.node_version
    }

    pub fn node_id(&self) -> String {
        self.network_state.node_id()
    }

    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        self.network_state
            .add_node(&self.p2p_control, peer_id, address)
    }

    pub fn remove_node(&self, peer_id: &PeerId) {
        if let Some(session_id) = self
            .network_state
            .peer_registry
            .read()
            .get_key_by_peer_id(peer_id)
        {
            if let Err(err) =
                disconnect_with_message(&self.p2p_control, session_id, "disconnect manually")
            {
                debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
            }
        } else {
            error!("Cannot find peer {:?}", peer_id);
        }
    }

    pub fn get_banned_addrs(&self) -> Vec<BannedAddr> {
        self.network_state
            .peer_store
            .lock()
            .ban_list()
            .get_banned_addrs()
    }

    pub fn ban(&self, address: IpNetwork, ban_until: u64, ban_reason: String) -> Result<(), Error> {
        self.network_state
            .peer_store
            .lock()
            .ban_network(address, ban_until, ban_reason)
    }

    pub fn unban(&self, address: &IpNetwork) {
        self.network_state
            .peer_store
            .lock()
            .mut_ban_list()
            .unban_network(address);
    }

    pub fn connected_peers(&self) -> Vec<(PeerId, Peer)> {
        let peers = self
            .network_state
            .with_peer_registry(|reg| reg.peers().values().cloned().collect::<Vec<_>>());
        peers
            .into_iter()
            .map(|peer| (peer.peer_id.clone(), peer))
            .collect()
    }

    fn try_broadcast(
        &self,
        quick: bool,
        target: TargetSession,
        proto_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), P2pError> {
        let now = Instant::now();
        loop {
            let result = if quick {
                self.p2p_control
                    .quick_filter_broadcast(target.clone(), proto_id, data.clone())
            } else {
                self.p2p_control
                    .filter_broadcast(target.clone(), proto_id, data.clone())
            };
            match result {
                Ok(()) => {
                    return Ok(());
                }
                Err(P2pError::IoError(ref err)) if err.kind() == io::ErrorKind::WouldBlock => {
                    if now.elapsed() > P2P_SEND_TIMEOUT {
                        warn!("broadcast message to {} timeout", proto_id);
                        return Err(P2pError::IoError(io::ErrorKind::TimedOut.into()));
                    }
                    thread::sleep(P2P_TRY_SEND_INTERVAL);
                }
                Err(err) => {
                    warn!("broadcast message to {} failed: {:?}", proto_id, err);
                    return Err(err);
                }
            }
        }
    }

    pub fn broadcast(&self, proto_id: ProtocolId, data: Bytes) -> Result<(), P2pError> {
        let session_ids = self.network_state.peer_registry.read().connected_peers();
        let target = TargetSession::Multi(session_ids);
        self.try_broadcast(false, target, proto_id, data)
    }

    pub fn quick_broadcast(&self, proto_id: ProtocolId, data: Bytes) -> Result<(), P2pError> {
        let session_ids = self.network_state.peer_registry.read().connected_peers();
        let target = TargetSession::Multi(session_ids);
        self.try_broadcast(true, target, proto_id, data)
    }

    pub fn send_message_to(
        &self,
        session_id: SessionId,
        proto_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), P2pError> {
        let target = TargetSession::Single(session_id);
        self.try_broadcast(false, target, proto_id, data)
    }
}

impl Drop for NetworkController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}

// Send an optional message before disconnect a peer
pub(crate) fn disconnect_with_message(
    control: &ServiceControl,
    peer_index: SessionId,
    message: &str,
) -> Result<(), P2pError> {
    if !message.is_empty() {
        let data = Bytes::from(message.as_bytes().to_vec());
        // Must quick send, otherwise this message will be dropped.
        control.quick_send_message_to(peer_index, DISCONNECT_MESSAGE_PROTOCOL_ID.into(), data)?;
    }
    control.disconnect(peer_index)
}
