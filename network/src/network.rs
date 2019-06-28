use crate::errors::Error;
use crate::peer_registry::{ConnectionStatus, PeerRegistry};
use crate::peer_store::{sqlite::SqlitePeerStore, types::PeerAddr, PeerStore, Status};
use crate::protocols::feeler::Feeler;
use crate::protocols::{
    discovery::{DiscoveryProtocol, DiscoveryService},
    identify::IdentifyCallback,
    ping::PingService,
};
use crate::services::{dns_seeding::DnsSeedingService, outbound_peer::OutboundPeerService};
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, NetworkConfig, ProtocolId, ProtocolVersion, PublicKey, ServiceControl,
};
use ckb_build_info::Version;
use ckb_logger::{debug, error, info, trace, warn};
use ckb_stop_handler::{SignalSender, StopHandler};
use ckb_util::{Mutex, RwLock};
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::mpsc::channel;
use futures::sync::{mpsc, oneshot};
use futures::Future;
use futures::Stream;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    bytes::Bytes,
    context::{ServiceContext, SessionContext},
    error::Error as P2pError,
    multiaddr::{self, multihash::Multihash, Multiaddr},
    secio::{self, PeerId},
    service::{
        DialProtocol, ProtocolEvent, ProtocolHandle, Service, ServiceError, ServiceEvent,
        TargetSession,
    },
    traits::ServiceHandle,
    utils::extract_peer_id,
    SessionId,
};
use p2p_identify::IdentifyProtocol;
use p2p_ping::PingHandler;
use std::boxed::Box;
use std::cmp::max;
use std::io;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::usize;
use tokio::runtime;

pub(crate) const PING_PROTOCOL_ID: usize = 0;
pub(crate) const DISCOVERY_PROTOCOL_ID: usize = 1;
pub(crate) const IDENTIFY_PROTOCOL_ID: usize = 2;
pub(crate) const FEELER_PROTOCOL_ID: usize = 3;

const ADDR_LIMIT: u32 = 3;
const P2P_SEND_TIMEOUT: Duration = Duration::from_secs(6);
const P2P_TRY_SEND_INTERVAL: Duration = Duration::from_millis(100);
// After 5 minutes we consider this dial hang
const DIAL_HANG_TIMEOUT: Duration = Duration::from_secs(300);

type MultiaddrList = Vec<(Multiaddr, u8)>;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<ProtocolVersion>,
}

pub struct NetworkState {
    pub(crate) peer_registry: RwLock<PeerRegistry>,
    pub(crate) peer_store: Mutex<Box<dyn PeerStore>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    dialing_addrs: RwLock<FnvHashMap<PeerId, Instant>>,

    pub(crate) protocol_ids: RwLock<FnvHashSet<ProtocolId>>,
    listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    // Send disconnect message but not disconnected yet
    disconnecting_sessions: RwLock<FnvHashSet<SessionId>>,
    local_private_key: secio::SecioKeyPair,
    local_peer_id: PeerId,
    pub(crate) config: NetworkConfig,
}

impl NetworkState {
    pub fn from_config(config: NetworkConfig) -> Result<NetworkState, Error> {
        config.create_dir_if_not_exists()?;
        let local_private_key = config.fetch_private_key()?;
        // set max score to public addresses
        let listened_addresses: FnvHashMap<Multiaddr, u8> = config
            .listen_addresses
            .iter()
            .chain(config.public_addresses.iter())
            .map(|addr| (addr.to_owned(), std::u8::MAX))
            .collect();
        let peer_store: Mutex<Box<dyn PeerStore>> = {
            let mut peer_store =
                SqlitePeerStore::file(config.peer_store_path().to_string_lossy().to_string())?;
            let bootnodes = config.bootnodes()?;
            for (peer_id, addr) in bootnodes {
                peer_store.add_bootnode(peer_id, addr);
            }
            Mutex::new(Box::new(peer_store))
        };

        let reserved_peers = config
            .reserved_peers()?
            .iter()
            .map(|(peer_id, _)| peer_id.to_owned())
            .collect::<Vec<_>>();
        let peer_registry = PeerRegistry::new(
            config.max_inbound_peers(),
            config.max_outbound_peers(),
            config.reserved_only,
            reserved_peers,
        );

        Ok(NetworkState {
            peer_store,
            config,
            peer_registry: RwLock::new(peer_registry),
            dialing_addrs: RwLock::new(FnvHashMap::default()),
            listened_addresses: RwLock::new(listened_addresses),
            original_listened_addresses: RwLock::new(Vec::new()),
            disconnecting_sessions: RwLock::new(FnvHashSet::default()),
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.to_public_key().peer_id(),
            protocol_ids: RwLock::new(FnvHashSet::default()),
        })
    }

    pub(crate) fn report_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        behaviour: Behaviour,
    ) {
        if let Some(peer_id) =
            self.with_peer_registry(|reg| reg.get_peer(session_id).map(|peer| peer.peer_id.clone()))
        {
            self.report_peer(p2p_control, &peer_id, behaviour);
        } else {
            debug!("Report {} failed: not in peer registry", session_id);
        }
    }

    pub(crate) fn report_peer(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        behaviour: Behaviour,
    ) {
        trace!("report {:?} because {:?}", peer_id, behaviour);
        if self
            .peer_store
            .lock()
            .report(peer_id, behaviour)
            .is_banned()
        {
            info!("peer {:?} banned", peer_id);
            self.with_peer_registry_mut(|reg| {
                if let Some(session_id) = reg.get_key_by_peer_id(peer_id) {
                    reg.remove_peer(session_id);
                    if let Err(err) = p2p_control.disconnect(session_id) {
                        debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                    }
                }
            })
        }
    }

    pub(crate) fn ban_session(
        &self,
        p2p_control: &ServiceControl,
        session_id: SessionId,
        timeout: Duration,
    ) {
        if let Some(peer_id) =
            self.with_peer_registry(|reg| reg.get_peer(session_id).map(|peer| peer.peer_id.clone()))
        {
            self.ban_peer(p2p_control, &peer_id, timeout);
        } else {
            debug!("Ban session({}) failed: not in peer registry", session_id);
        }
    }

    pub(crate) fn ban_peer(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        timeout: Duration,
    ) {
        info!("ban peer {:?} with {:?}", peer_id, timeout);
        let peer_opt = self.with_peer_registry_mut(|reg| reg.remove_peer_by_peer_id(peer_id));
        if let Some(peer) = peer_opt {
            self.peer_store.lock().ban_addr(&peer.address, timeout);
            if let Err(err) = p2p_control.disconnect(peer.session_id) {
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
                peer_id.clone(),
                session_context.address.clone(),
                session_context.id,
                session_context.ty,
                peer_store.as_mut(),
            )
        };
        if accept_peer_result.is_ok() {
            peer_store.update_status(&peer_id, Status::Connected);
        }
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
    pub(crate) fn with_peer_store<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&PeerStore) -> T,
    {
        callback(self.peer_store.lock().as_ref())
    }

    // For restrict lock in inner scope
    pub(crate) fn with_peer_store_mut<F, T>(&self, callback: F) -> T
    where
        F: FnOnce(&mut PeerStore) -> T,
    {
        callback(self.peer_store.lock().as_mut())
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
    }

    pub fn node_id(&self) -> String {
        self.local_private_key().to_peer_id().to_base58()
    }

    pub(crate) fn listened_addresses(&self, count: usize) -> Vec<(Multiaddr, u8)> {
        self.listened_addresses
            .read()
            .iter()
            .take(count)
            .map(|(addr, score)| (addr.to_owned(), *score))
            .collect()
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        self.peer_registry.read().connection_status()
    }

    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        let original_listened_addresses = self.original_listened_addresses.read();
        self.listened_addresses(max_urls.saturating_sub(original_listened_addresses.len()))
            .into_iter()
            .filter(|(addr, _)| !original_listened_addresses.contains(addr))
            .chain(
                original_listened_addresses
                    .iter()
                    .map(|addr| (addr.to_owned(), 1)),
            )
            .map(|(addr, score)| (self.to_external_url(&addr), score))
            .collect()
    }

    pub(crate) fn add_node(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        address: Multiaddr,
    ) {
        self.dial_identify(p2p_control, peer_id, address.clone());
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

    pub(crate) fn can_dial(&self, peer_id: &PeerId, addr: &Multiaddr) -> bool {
        if self.local_peer_id() == peer_id {
            trace!("Do not dial self: {:?}, {}", peer_id, addr);
            return false;
        }
        if self.listened_addresses.read().contains_key(&addr) {
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
    pub fn dial(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        mut addr: Multiaddr,
        target: DialProtocol,
    ) -> bool {
        if self.can_dial(peer_id, &addr) {
            match Multihash::from_bytes(peer_id.as_bytes().to_vec()) {
                Ok(peer_id_hash) => {
                    addr.push(multiaddr::Protocol::P2p(peer_id_hash));
                    debug!("dialing {} with {:?}", addr, target);
                    if let Err(err) = p2p_control.dial(addr.clone(), target) {
                        debug!("dial failed: {:?}", err);
                    } else {
                        self.dialing_addrs
                            .write()
                            .insert(peer_id.to_owned(), Instant::now());
                        return true;
                    }
                }
                Err(err) => {
                    error!("failed to convert peer_id to addr: {}", err);
                }
            }
        }
        false
    }

    /// Dial just identify protocol
    pub fn dial_identify(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        self.dial(
            p2p_control,
            peer_id,
            addr,
            DialProtocol::Single(IDENTIFY_PROTOCOL_ID.into()),
        );
    }

    /// Dial just feeler protocol
    pub fn dial_feeler(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        if self.dial(
            p2p_control,
            peer_id,
            addr,
            DialProtocol::Single(FEELER_PROTOCOL_ID.into()),
        ) {
            self.with_peer_registry_mut(|reg| {
                reg.add_feeler(peer_id.clone());
            });
        }
    }
}

pub struct EventHandler {
    pub(crate) network_state: Arc<NetworkState>,
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
                    debug!("add self address: {:?}", address);
                    let addr = address
                        .iter()
                        .filter(|proto| match proto {
                            multiaddr::Protocol::P2p(_) => false,
                            _ => true,
                        })
                        .collect();
                    self.network_state
                        .listened_addresses
                        .write()
                        .insert(addr, std::u8::MAX);
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
                if let Err(err) = context.disconnect(id) {
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
            err => {
                debug!("p2p service error: {:?}", err);
                use sentry::{capture_message, with_scope, Level};
                with_scope(
                    |scope| scope.set_fingerprint(Some(&["ckb-network", "p2p-service-error"])),
                    || capture_message(&format!("p2p service error: {:?}", err), Level::Warning),
                );
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
                                evicted_peer.session_id, evicted_peer.address,
                            );
                            if let Err(err) = context.disconnect(evicted_peer.session_id) {
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
                            if let Err(err) = context.disconnect(session_context.id) {
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

                self.network_state.with_peer_registry_mut(|reg| {
                    reg.remove_feeler(&peer_id);
                });
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
                        peer_store.update_status(&peer_id, Status::Disconnected);
                    })
                }
            }
            _ => {
                info!("p2p service event: {:?}", event);
            }
        }
    }

    fn handle_proto(&mut self, context: &mut ServiceContext, event: ProtocolEvent) {
        // For special protocols: ping/discovery/identify
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
            ProtocolEvent::Disconnected { .. } => {
                // Do nothing
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
                    if let Err(err) = context.disconnect(session_id) {
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
    bg_services: Vec<Box<dyn Future<Item = (), Error = ()> + Send>>,
}

impl NetworkService {
    pub fn new(
        network_state: Arc<NetworkState>,
        protocols: Vec<CKBProtocol>,
        name: String,
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
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(PingHandler::new(
                    ping_interval,
                    ping_timeout,
                    ping_sender.clone(),
                )))
            })
            .build();

        // Discovery protocol
        let (disc_sender, disc_receiver) = mpsc::unbounded();
        let disc_meta = MetaBuilder::default()
            .id(DISCOVERY_PROTOCOL_ID.into())
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(DiscoveryProtocol::new(disc_sender.clone())))
            })
            .build();

        // Identify protocol
        let identify_callback = IdentifyCallback::new(Arc::clone(&network_state), name);
        let identify_meta = MetaBuilder::default()
            .id(IDENTIFY_PROTOCOL_ID.into())
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(IdentifyProtocol::new(identify_callback.clone())))
            })
            .build();

        // Feeler protocol
        // TODO: versions
        let feeler_meta = MetaBuilder::default()
            .id(FEELER_PROTOCOL_ID.into())
            .name(move |_| "/ckb/flr/".to_string())
            .service_handle({
                let network_state = Arc::clone(&network_state);
                move || ProtocolHandle::Both(Box::new(Feeler::new(Arc::clone(&network_state))))
            })
            .build();

        // == Build p2p service struct
        let mut protocol_metas = protocols
            .into_iter()
            .map(CKBProtocol::build)
            .collect::<Vec<_>>();
        protocol_metas.push(feeler_meta);
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
        };
        let p2p_service = service_builder
            .key_pair(network_state.local_private_key.clone())
            .upnp(config.upnp)
            .forever(true)
            .build(event_handler);

        // == Build background service tasks
        let disc_service = DiscoveryService::new(
            Arc::clone(&network_state),
            disc_receiver,
            config.discovery_local_address,
        );
        let ping_service = PingService::new(
            Arc::clone(&network_state),
            p2p_service.control().to_owned(),
            ping_receiver,
        );
        let outbound_peer_service = OutboundPeerService::new(
            Arc::clone(&network_state),
            p2p_service.control().to_owned(),
            Duration::from_secs(config.connect_outbound_interval_secs),
        );
        let dns_seeding_service = DnsSeedingService::new(
            Arc::clone(&network_state),
            network_state.config.dns_seeds.clone(),
        );
        let bg_services = vec![
            Box::new(ping_service.for_each(|_| Ok(()))) as Box<_>,
            Box::new(disc_service) as Box<_>,
            Box::new(outbound_peer_service) as Box<_>,
            Box::new(dns_seeding_service) as Box<_>,
        ];

        NetworkService {
            p2p_service,
            network_state,
            bg_services,
        }
    }

    pub fn start<S: ToString>(
        mut self,
        node_version: Version,
        thread_name: Option<S>,
    ) -> Result<NetworkController, Error> {
        let config = &self.network_state.config;
        // listen local addresses
        for addr in &config.listen_addresses {
            match self.p2p_service.listen(addr.to_owned()) {
                Ok(listen_address) => {
                    info!(
                        "Listen on address: {}",
                        self.network_state.to_external_url(&listen_address)
                    );
                    self.network_state
                        .original_listened_addresses
                        .write()
                        .push(listen_address.clone())
                }
                Err(err) => {
                    warn!(
                        "listen on address {} failed, due to error: {}",
                        addr.clone(),
                        err
                    );
                    return Err(Error::Io(err));
                }
            };
        }

        // dial reserved_nodes
        for (peer_id, addr) in config.reserved_peers()? {
            debug!("dial reserved_peers {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), &peer_id, addr);
        }

        let bootnodes = self.network_state.with_peer_store(|peer_store| {
            peer_store.bootnodes(max((config.max_outbound_peers / 2) as u32, 1))
        });
        // dial half bootnodes
        for (peer_id, addr) in bootnodes {
            debug!("dial bootnode {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_identify(self.p2p_service.control(), &peer_id, addr);
        }
        let p2p_control = self.p2p_service.control().to_owned();
        let network_state = Arc::clone(&self.network_state);

        // Mainly for test: give a empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let (sender, receiver) = crossbeam_channel::bounded(1);
        // Main network thread
        let thread = thread_builder
            .spawn(move || {
                let inner_p2p_control = self.p2p_service.control().to_owned();
                let num_threads = max(num_cpus::get(), 4);
                let mut runtime = runtime::Builder::new()
                    .core_threads(num_threads)
                    .name_prefix("NetworkRuntime-")
                    .build()
                    .expect("Network tokio runtime init failed");
                runtime.spawn(self.p2p_service.for_each(|_| Ok(())));

                // NOTE: for ensure background task finished
                let bg_signals = self
                    .bg_services
                    .into_iter()
                    .map(|bg_service| {
                        let (signal_sender, signal_receiver) = oneshot::channel::<()>();
                        let task = signal_receiver
                            .select2(bg_service)
                            .map(|_| ())
                            .map_err(|_| ());
                        runtime.spawn(task);
                        signal_sender
                    })
                    .collect::<Vec<_>>();

                debug!("receiving shutdown signal ...");

                // Recevied stop signal, doing cleanup
                let _ = receiver.recv();
                for peer in self.network_state.peer_registry.read().peers().values() {
                    info!("Disconnect peer {}", peer.address);
                    if let Err(err) = inner_p2p_control.disconnect(peer.session_id) {
                        debug!("Disconnect failed {:?}, error: {:?}", peer.session_id, err);
                    }
                }
                // Drop senders to stop all corresponding background task
                drop(bg_signals);
                if let Err(err) = inner_p2p_control.shutdown() {
                    warn!("send shutdown message to p2p error: {:?}", err);
                }

                debug!("Waiting tokio runtime to finish ...");
                runtime.shutdown_on_idle().wait().unwrap();
                debug!("Shutdown network service finished!");
            })
            .expect("Start NetworkService failed");
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
    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        self.network_state.external_urls(max_urls)
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
        self.network_state.with_peer_registry_mut(|reg| {
            if let Some(session_id) = reg.get_key_by_peer_id(peer_id) {
                reg.remove_peer(session_id);
                if let Err(err) = self.p2p_control.disconnect(session_id) {
                    debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                }
            } else {
                error!("Cannot find peer {:?}", peer_id);
            }
        })
    }

    pub fn connected_peers(&self) -> Vec<(PeerId, Peer, MultiaddrList)> {
        let peers = self
            .network_state
            .with_peer_registry(|reg| reg.peers().values().cloned().collect::<Vec<_>>());
        self.network_state.with_peer_store(|peer_store| {
            peers
                .into_iter()
                .map(|peer| {
                    // FIXME how to return address score?
                    (
                        peer.peer_id.clone(),
                        peer.clone(),
                        peer_store
                            .peer_addrs(&peer.peer_id, ADDR_LIMIT)
                            .into_iter()
                            .map(|paddr| {
                                let PeerAddr { addr, .. } = paddr;
                                (addr, 1)
                            })
                            .collect(),
                    )
                })
                .collect()
        })
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
        let target = TargetSession::Multi(session_ids.clone());
        self.try_broadcast(false, target, proto_id, data)
    }

    pub fn quick_broadcast(&self, proto_id: ProtocolId, data: Bytes) -> Result<(), P2pError> {
        let session_ids = self.network_state.peer_registry.read().connected_peers();
        let target = TargetSession::Multi(session_ids.clone());
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
