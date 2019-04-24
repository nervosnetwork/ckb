use crate::errors::Error;
use crate::peer_registry::{ConnectionStatus, PeerRegistry};
use crate::peer_store::{sqlite::SqlitePeerStore, PeerStore, Status};
use crate::protocols::feeler::Feeler;
use crate::protocols::{
    discovery::{DiscoveryProtocol, DiscoveryService},
    identify::IdentifyCallback,
    outbound_peer::OutboundPeerService,
    ping::PingService,
};
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, NetworkConfig, ProtocolId, ProtocolVersion, PublicKey, ServiceControl,
};
use ckb_util::{Mutex, RwLock};
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::mpsc::channel;
use futures::sync::{mpsc, oneshot};
use futures::Future;
use futures::Stream;
use log::{debug, error, info, trace, warn};
use lru_cache::LruCache;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    context::{ServiceContext, SessionContext},
    error::Error as P2pError,
    multiaddr::{self, multihash::Multihash, Multiaddr},
    secio::PeerId,
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
use secio;
use std::boxed::Box;
use std::cmp::max;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::usize;
use stop_handler::{SignalSender, StopHandler};
use tokio::runtime::Runtime;

const PING_PROTOCOL_ID: usize = 0;
const DISCOVERY_PROTOCOL_ID: usize = 1;
const IDENTIFY_PROTOCOL_ID: usize = 2;
const FEELER_PROTOCOL_ID: usize = 3;

const ADDR_LIMIT: u32 = 3;
const FAILED_DIAL_CACHE_SIZE: usize = 100;

type MultiaddrList = Vec<(Multiaddr, u8)>;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<ProtocolVersion>,
}

pub struct NetworkState {
    peer_registry: RwLock<PeerRegistry>,
    peer_store: Mutex<Box<dyn PeerStore>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    // For avoid repeat failed dial
    pub(crate) failed_dials: RwLock<LruCache<PeerId, Instant>>,

    protocol_ids: RwLock<FnvHashSet<ProtocolId>>,
    listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    // Send disconnect message but not disconnected yet
    disconnecting_sessions: RwLock<FnvHashSet<SessionId>>,
    local_private_key: secio::SecioKeyPair,
    local_peer_id: PeerId,
    config: NetworkConfig,
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
            failed_dials: RwLock::new(LruCache::new(FAILED_DIAL_CACHE_SIZE)),
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
            debug!(target: "network", "Report {} failed: not in peer registry", session_id);
        }
    }

    pub(crate) fn report_peer(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        behaviour: Behaviour,
    ) {
        trace!(target: "network", "report {:?} because {:?}", peer_id, behaviour);
        if self
            .peer_store
            .lock()
            .report(peer_id, behaviour)
            .is_banned()
        {
            info!(target: "network", "peer {:?} banned", peer_id);
            self.with_peer_registry_mut(|reg| {
                if let Some(session_id) = reg.get_key_by_peer_id(peer_id) {
                    reg.remove_peer(session_id);
                    if let Err(err) = p2p_control.disconnect(session_id) {
                        error!(target: "network", "send message to p2p service error: {:?}", err);
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
            debug!(target: "network", "Ban session({}) failed: not in peer registry", session_id);
        }
    }

    pub(crate) fn ban_peer(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        timeout: Duration,
    ) {
        info!(target: "network", "ban peer {:?} with {:?}", peer_id, timeout);
        self.peer_store.lock().ban_peer(peer_id, timeout);
        self.with_peer_registry_mut(|reg| {
            if let Some(session_id) = reg.get_key_by_peer_id(peer_id) {
                reg.remove_peer(session_id);
                if let Err(err) = p2p_control.disconnect(session_id) {
                    error!(target: "network", "send message to p2p service error: {:?}", err);
                }
            }
        });
    }

    pub(crate) fn query_session_id(&self, peer_id: &PeerId) -> Option<SessionId> {
        let mut target_session_id = None;
        // Create a scope for avoid dead lock
        {
            let peer_registry = self.peer_registry.read();
            for peer in peer_registry.peers().values() {
                if &peer.peer_id == peer_id {
                    target_session_id = Some(peer.session_id);
                }
            }
        }
        target_session_id
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

    // TODO: A workaround method for `add_node` rpc call, need to re-write it after new p2p lib integration.
    pub(crate) fn add_node(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        address: Multiaddr,
    ) {
        self.dial_all(p2p_control, peer_id, address.clone());
        if !self.peer_store.lock().add_discovered_addr(peer_id, address) {
            warn!(target: "network", "add_node failed {:?}", peer_id);
        }
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

    pub fn dial(
        &self,
        p2p_control: &ServiceControl,
        peer_id: &PeerId,
        mut addr: Multiaddr,
        target: DialProtocol,
    ) {
        if !self.listened_addresses.read().contains_key(&addr) {
            match Multihash::from_bytes(peer_id.as_bytes().to_vec()) {
                Ok(peer_id_hash) => {
                    addr.append(multiaddr::Protocol::P2p(peer_id_hash));
                    debug!(target: "network", "dialing {} with {:?}", addr, target);
                    if let Err(err) = p2p_control.dial(addr.clone(), target) {
                        debug!(target: "network", "dial fialed: {:?}", err);
                    }
                }
                Err(err) => {
                    error!(target: "network", "failed to convert peer_id to addr: {}", err);
                }
            }
        }
    }

    /// Dial all protocol except feeler
    pub fn dial_all(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        let ids = self.get_protocol_ids(|id| id != FEELER_PROTOCOL_ID.into());
        self.dial(p2p_control, peer_id, addr, DialProtocol::Multi(ids));
    }

    /// Dial just feeler protocol
    pub fn dial_feeler(&self, p2p_control: &ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        self.dial(
            p2p_control,
            peer_id,
            addr,
            DialProtocol::Single(FEELER_PROTOCOL_ID.into()),
        );
    }
}

pub struct EventHandler {
    network_state: Arc<NetworkState>,
}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, context: &mut ServiceContext, error: ServiceError) {
        match error {
            ServiceError::DialerError {
                ref address,
                ref error,
            } => {
                warn!(target: "network", "DialerError({}) {}", address, error);
                if error == &P2pError::ConnectSelf {
                    debug!(target: "network", "add self address: {:?}", address);
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
                if let Some(peer_id) = extract_peer_id(address) {
                    self.network_state.with_peer_registry_mut(|reg| {
                        reg.remove_feeler(&peer_id);
                    });
                    self.network_state
                        .failed_dials
                        .write()
                        .insert(peer_id, Instant::now());
                }
            }
            ServiceError::ProtocolError {
                id,
                proto_id,
                error,
            } => {
                warn!(target: "network", "ProtocolError({}, {}) {}", id, proto_id, error);
                context.disconnect(id);
            }
            ServiceError::SessionTimeout { session_context } => {
                warn!(
                    target: "network",
                    "SessionTimeout({}, {})",
                    session_context.id,
                    session_context.address,
                );
            }
            ServiceError::MuxerError {
                session_context,
                error,
            } => {
                warn!(
                    target: "network",
                    "MuxerError({}, {}), substream error {}, disconnect it",
                    session_context.id,
                    session_context.address,
                    error,
                );
                context.disconnect(session_context.id)
            }
            _ => {
                warn!(target: "network", "p2p service error: {:?}", error);
            }
        }
    }

    fn handle_event(&mut self, context: &mut ServiceContext, event: ServiceEvent) {
        // When session disconnect update status anyway
        match event {
            ServiceEvent::SessionOpen { session_context } => {
                debug!(
                    target: "network",
                    "SessionOpen({}, {})",
                    session_context.id,
                    session_context.address,
                );
                let peer_id = session_context
                    .remote_pubkey
                    .as_ref()
                    .map(PublicKey::peer_id)
                    .expect("Secio must enabled");
                if self
                    .network_state
                    .with_peer_registry(|reg| reg.is_feeler(&peer_id))
                {
                    debug!(
                        target: "network",
                        "feeler connected {} => {}",
                        session_context.id,
                        session_context.address,
                    );
                } else {
                    match self.network_state.accept_peer(&session_context) {
                        Ok(Some(evicted_peer)) => {
                            info!(
                                target: "network",
                                "evict peer (disonnect it), {} => {}",
                                evicted_peer.session_id,
                                evicted_peer.address,
                            );
                            context.disconnect(evicted_peer.session_id);
                        }
                        Ok(None) => info!(
                            target: "network",
                            "{} open, registry {} success",
                            session_context.id,
                            session_context.address,
                        ),
                        Err(err) => {
                            warn!(
                                target: "network",
                                "registry peer failed {:?} disconnect it, {} => {}",
                                err,
                                session_context.id,
                                session_context.address,
                            );
                            context.disconnect(session_context.id);
                        }
                    }
                }
            }
            ServiceEvent::SessionClose { session_context } => {
                debug!(
                    target: "network",
                    "SessionClose({}, {})",
                    session_context.id,
                    session_context.address,
                );
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
                    info!(
                        target: "network",
                        "{} closed, remove {} from peer_registry",
                        session_context.id,
                        session_context.address,
                    );
                    let peer_id = session_context
                        .remote_pubkey
                        .as_ref()
                        .map(PublicKey::peer_id)
                        .expect("Secio must enabled");
                    self.network_state.with_peer_store_mut(|peer_store| {
                        peer_store.update_status(&peer_id, Status::Disconnected);
                    })
                }
            }
            _ => {
                info!(target: "network", "p2p service event: {:?}", event);
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
                            target: "network",
                            "Invalid session {}, protocol id {}",
                            session_context.id,
                            proto_id,
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
                        target: "network",
                        "disconnect peer({}) already removed from registry",
                        session_context.id
                    );
                    self.network_state
                        .disconnecting_sessions
                        .write()
                        .insert(session_id);
                    context.disconnect(session_id);
                }
            }
            _ => {}
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
    pub fn new(network_state: Arc<NetworkState>, protocols: Vec<CKBProtocol>) -> NetworkService {
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
        let identify_callback = IdentifyCallback::new(Arc::clone(&network_state));
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
            .forever(true)
            .build(event_handler);

        // == Build background service tasks
        let disc_service = DiscoveryService::new(Arc::clone(&network_state), disc_receiver);
        let ping_service = PingService::new(
            Arc::clone(&network_state),
            p2p_service.control().clone(),
            ping_receiver,
        );
        let outbound_peer_service = OutboundPeerService::new(
            Arc::clone(&network_state),
            p2p_service.control().clone(),
            Duration::from_secs(config.connect_outbound_interval_secs),
        );
        let bg_services = vec![
            Box::new(ping_service.for_each(|_| Ok(()))) as Box<_>,
            Box::new(disc_service.for_each(|_| Ok(()))) as Box<_>,
            Box::new(outbound_peer_service.for_each(|_| Ok(()))) as Box<_>,
        ];

        NetworkService {
            p2p_service,
            network_state,
            bg_services,
        }
    }

    pub fn start<S: ToString>(
        mut self,
        thread_name: Option<S>,
    ) -> Result<NetworkController, Error> {
        let config = &self.network_state.config;
        // listen local addresses
        for addr in &config.listen_addresses {
            match self.p2p_service.listen(addr.to_owned()) {
                Ok(listen_address) => {
                    info!(
                        target: "network",
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
                        target: "network",
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
            debug!(target: "network", "dial reserved_peers {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_all(self.p2p_service.control(), &peer_id, addr);
        }

        let bootnodes = self.network_state.with_peer_store(|peer_store| {
            peer_store.bootnodes(max((config.max_outbound_peers / 2) as u32, 1))
        });
        // dial half bootnodes
        for (peer_id, addr) in bootnodes {
            debug!(target: "network", "dial bootnode {:?} {:?}", peer_id, addr);
            self.network_state
                .dial_all(self.p2p_service.control(), &peer_id, addr);
        }
        let p2p_control = self.p2p_service.control().clone();
        let network_state = Arc::clone(&self.network_state);

        // Mainly for test: give a empty thread_name
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }
        let (sender, receiver) = crossbeam_channel::bounded(1);
        let thread = thread_builder
            .spawn(move || {
                let inner_p2p_control = self.p2p_service.control().clone();
                let mut runtime = Runtime::new().expect("Network tokio runtime init failed");
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

                debug!(target: "network", "receiving shutdown signal ...");

                // Recevied stop signal, doing cleanup
                let _ = receiver.recv();
                for peer in self.network_state.peer_registry.read().peers().values() {
                    info!(target: "network", "disconnect peer {}", peer.address);
                    if let Err(err) = inner_p2p_control.disconnect(peer.session_id) {
                        warn!(target: "network", "send disconnect message to p2p error: {:?}", err);
                    }
                }
                // Drop senders to stop all corresponding background task
                drop(bg_signals);
                if let Err(err) = inner_p2p_control.shutdown() {
                    warn!(target: "network", "send shutdown message to p2p error: {:?}", err);
                }

                debug!(target: "network", "Waiting tokio runtime to finish ...");
                runtime.shutdown_on_idle().wait().unwrap();
                debug!(target: "network", "Shutdown network service finished!");
            })
            .expect("Start NetworkService fialed");
        let stop = StopHandler::new(SignalSender::Crossbeam(sender), thread);
        Ok(NetworkController {
            network_state,
            p2p_control,
            stop,
        })
    }
}

#[derive(Clone)]
pub struct NetworkController {
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    stop: StopHandler<()>,
}

impl NetworkController {
    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        self.network_state.external_urls(max_urls)
    }

    pub fn node_id(&self) -> String {
        self.network_state.node_id()
    }

    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        self.network_state
            .add_node(&self.p2p_control, peer_id, address)
    }

    pub fn connected_peers(&self) -> Vec<(PeerId, Peer, MultiaddrList)> {
        let peers = self
            .network_state
            .with_peer_registry(|reg| reg.peers().values().cloned().collect::<Vec<_>>());
        self.network_state.with_peer_store(|peer_store| {
            peers
                .into_iter()
                .map(|peer| {
                    (
                        peer.peer_id.clone(),
                        peer.clone(),
                        peer_store
                            .peer_addrs(&peer.peer_id, ADDR_LIMIT)
                            .unwrap_or_default()
                            .into_iter()
                        // FIXME how to return address score?
                            .map(|address| (address, 1))
                            .collect(),
                    )
                })
                .collect()
        })
    }

    pub fn broadcast(&self, proto_id: ProtocolId, data: Vec<u8>) {
        let session_ids = self.network_state.peer_registry.read().connected_peers();
        if let Err(err) =
            self.p2p_control
                .filter_broadcast(TargetSession::Multi(session_ids), proto_id, data)
        {
            warn!(target: "network", "broadcast message to {} failed: {:?}", proto_id, err);
        }
    }

    pub fn send_message_to(&self, session_id: SessionId, proto_id: ProtocolId, data: Vec<u8>) {
        if let Err(err) = self.p2p_control.send_message_to(session_id, proto_id, data) {
            warn!(target: "network", "send message to {} {} failed: {:?}", session_id, proto_id, err);
        }
    }
}

impl Drop for NetworkController {
    fn drop(&mut self) {
        self.stop.try_send();
    }
}
