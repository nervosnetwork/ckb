use crate::errors::Error;
use crate::peer_store::{sqlite::SqlitePeerStore, PeerStore, Status};
use crate::peers_registry::{ConnectionStatus, PeersRegistry, RegisterResult};
use crate::protocols::{
    discovery::{DiscoveryProtocol, DiscoveryService},
    identify::IdentifyCallback,
    outbound_peer::OutboundPeerService,
    ping::PingService,
};
use crate::protocols::{feeler::Feeler, DefaultCKBProtocolContext};
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, CKBProtocolContext, NetworkConfig, PeerIndex, ProtocolId,
    ProtocolVersion, PublicKey, ServiceContext, ServiceControl, SessionId, SessionType,
};
use ckb_util::RwLock;
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::mpsc::channel;
use futures::sync::{mpsc, oneshot};
use futures::Future;
use futures::Stream;
use log::{debug, error, info, warn};
use lru_cache::LruCache;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    error::Error as P2pError,
    multiaddr::{self, multihash::Multihash, Multiaddr},
    secio::PeerId,
    service::{DialProtocol, ProtocolEvent, ProtocolHandle, Service, ServiceError, ServiceEvent},
    traits::ServiceHandle,
    utils::extract_peer_id,
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

const PING_PROTOCOL_ID: ProtocolId = 0;
const DISCOVERY_PROTOCOL_ID: ProtocolId = 1;
const IDENTIFY_PROTOCOL_ID: ProtocolId = 2;
const FEELER_PROTOCOL_ID: ProtocolId = 3;

const ADDR_LIMIT: u32 = 3;
const FAILED_DIAL_CACHE_SIZE: usize = 100;

type MultiaddrList = Vec<(Multiaddr, u8)>;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<ProtocolVersion>,
}

pub struct NetworkState {
    protocol_ids: RwLock<FnvHashSet<ProtocolId>>,
    pub(crate) peers_registry: PeersRegistry,
    peer_store: Arc<dyn PeerStore>,
    listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    // For avoid repeat failed dial
    pub(crate) failed_dials: RwLock<LruCache<PeerId, Instant>>,
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
        let peer_store: Arc<dyn PeerStore> = {
            let peer_store =
                SqlitePeerStore::file(config.peer_store_path().to_string_lossy().to_string())?;
            let bootnodes = config.bootnodes()?;
            for (peer_id, addr) in bootnodes {
                peer_store.add_bootnode(peer_id, addr);
            }
            Arc::new(peer_store)
        };

        let reserved_peers = config
            .reserved_peers()?
            .iter()
            .map(|(peer_id, _)| peer_id.to_owned())
            .collect::<Vec<_>>();
        let peers_registry = PeersRegistry::new(
            Arc::clone(&peer_store),
            config.max_inbound_peers(),
            config.max_outbound_peers(),
            config.reserved_only,
            reserved_peers,
        );

        Ok(NetworkState {
            peer_store,
            config,
            peers_registry,
            failed_dials: RwLock::new(LruCache::new(FAILED_DIAL_CACHE_SIZE)),
            listened_addresses: RwLock::new(listened_addresses),
            original_listened_addresses: RwLock::new(Vec::new()),
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.to_public_key().peer_id(),
            protocol_ids: RwLock::new(FnvHashSet::default()),
        })
    }

    pub fn report(&self, peer_id: &PeerId, behaviour: Behaviour) {
        info!(target: "network", "report {:?} because {:?}", peer_id, behaviour);
        self.peer_store.report(peer_id, behaviour);
    }

    pub fn drop_peer(&self, p2p_control: &mut ServiceControl, peer_id: &PeerId) {
        debug!(target: "network", "drop peer {:?}", peer_id);
        if let Some(peer) = self.peers_registry.drop_peer(&peer_id) {
            if let Err(err) = p2p_control.disconnect(peer.session_id) {
                error!(target: "network", "disconnect peer error {:?}", err);
            }
        }
    }

    pub fn drop_all(&self, p2p_control: &mut ServiceControl) {
        debug!(target: "network", "drop all connections...");
        let mut peer_ids = Vec::new();
        {
            for (peer_id, peer) in self.peers_registry.peers_guard().read().iter() {
                peer_ids.push(peer_id.clone());
                if let Err(err) = p2p_control.disconnect(peer.session_id) {
                    error!(target: "network", "disconnect peer error {:?}", err);
                }
            }
        }
        self.peers_registry.drop_all();

        let peer_store = self.peer_store();
        for peer_id in peer_ids {
            if peer_store.peer_status(&peer_id) != Status::Disconnected {
                peer_store.update_status(&peer_id, Status::Disconnected);
            }
        }
    }

    pub fn local_peer_id(&self) -> &PeerId {
        &self.local_peer_id
    }

    pub(crate) fn listened_addresses(&self, count: usize) -> Vec<(Multiaddr, u8)> {
        let listened_addresses = self.listened_addresses.read();
        listened_addresses
            .iter()
            .take(count)
            .map(|(addr, score)| (addr.to_owned(), *score))
            .collect()
    }

    pub(crate) fn get_peer_index(&self, peer_id: &PeerId) -> Option<PeerIndex> {
        self.peers_registry
            .peers_guard()
            .read()
            .get(&peer_id)
            .map(|peer| peer.peer_index)
    }

    pub(crate) fn get_peer_id(&self, peer_index: PeerIndex) -> Option<PeerId> {
        self.peers_registry.get_peer_id(peer_index)
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        self.peers_registry.connection_status()
    }

    pub(crate) fn modify_peer<F>(&self, peer_id: &PeerId, f: F)
    where
        F: FnOnce(&mut Peer) -> (),
    {
        self.peers_registry.modify_peer(peer_id, f);
    }

    pub(crate) fn peers_indexes(&self) -> Vec<PeerIndex> {
        self.peers_registry.connected_peers_indexes()
    }

    pub(crate) fn ban_peer(
        &self,
        p2p_control: &mut ServiceControl,
        peer_id: &PeerId,
        timeout: Duration,
    ) {
        self.drop_peer(p2p_control, peer_id);
        self.peer_store.ban_peer(peer_id, timeout);
    }

    pub(crate) fn peer_store(&self) -> &Arc<dyn PeerStore> {
        &self.peer_store
    }

    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
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

    pub fn node_id(&self) -> String {
        self.local_private_key().to_peer_id().to_base58()
    }

    // A workaround method for `add_node` rpc call, need to re-write it after new p2p lib integration.
    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        if !self.peer_store().add_discovered_addr(peer_id, address) {
            warn!(target: "network", "add_node failed {:?}", peer_id);
        }
    }

    fn to_external_url(&self, addr: &Multiaddr) -> String {
        format!("{}/p2p/{}", addr, self.node_id())
    }

    pub(crate) fn accept_connection(
        &self,
        peer_id: PeerId,
        connected_addr: Multiaddr,
        session_id: SessionId,
        session_type: SessionType,
        protocol_id: ProtocolId,
        protocol_version: ProtocolVersion,
    ) -> Result<RegisterResult, Error> {
        self.peers_registry.accept_connection(
            peer_id,
            connected_addr,
            session_id,
            session_type,
            protocol_id,
            protocol_version,
        )
    }

    pub fn peer_protocol_version(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
    ) -> Option<ProtocolVersion> {
        self.peers_registry
            .peers_guard()
            .read()
            .get(peer_id)
            .and_then(|peer| peer.protocol_version(protocol_id))
    }

    pub fn session_info(&self, peer_id: &PeerId, protocol_id: ProtocolId) -> Option<SessionInfo> {
        self.peers_registry
            .peers_guard()
            .read()
            .get(peer_id)
            .map(|peer| {
                let protocol_version = peer.protocol_version(protocol_id);
                SessionInfo {
                    peer: peer.clone(),
                    protocol_version,
                }
            })
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
        p2p_control: &mut ServiceControl,
        peer_id: &PeerId,
        mut addr: Multiaddr,
        target: DialProtocol,
    ) {
        if !self.listened_addresses.read().contains_key(&addr) {
            match Multihash::from_bytes(peer_id.as_bytes().to_vec()) {
                Ok(peer_id_hash) => {
                    addr.append(multiaddr::Protocol::P2p(peer_id_hash));
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
    pub fn dial_all(&self, p2p_control: &mut ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        let ids = self.get_protocol_ids(|id| id != FEELER_PROTOCOL_ID);
        self.dial(p2p_control, peer_id, addr, DialProtocol::Multi(ids));
    }

    /// Dial just feeler protocol
    pub fn dial_feeler(&self, p2p_control: &mut ServiceControl, peer_id: &PeerId, addr: Multiaddr) {
        self.dial(
            p2p_control,
            peer_id,
            addr,
            DialProtocol::Single(FEELER_PROTOCOL_ID),
        );
    }
}

pub struct EventHandler {
    network_state: Arc<NetworkState>,
}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, context: &mut ServiceContext, error: ServiceError) {
        warn!(target: "network", "p2p service error: {:?}", error);
        match error {
            ServiceError::DialerError {
                ref address,
                ref error,
            } => {
                debug!(target: "network", "add self address: {:?}", address);
                if error == &P2pError::ConnectSelf {
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
                    self.network_state
                        .failed_dials
                        .write()
                        .insert(peer_id, Instant::now());
                }
            }
            ServiceError::ProtocolError { id, .. } => {
                if let Err(err) = context.control().disconnect(id) {
                    warn!(target: "network", "send disconnect task(session_id={}) failed, error={:?}", id, err);
                }
            }
            ServiceError::MuxerError {
                session_context, ..
            } => {
                if let Err(err) = context.control().disconnect(session_context.id) {
                    warn!(target: "network", "send disconnect task(session_id={}) failed, error={:?}", session_context.id, err);
                }
            }
            _ => {}
        }
    }

    fn handle_event(&mut self, context: &mut ServiceContext, event: ServiceEvent) {
        info!(target: "network", "p2p service event: {:?}", event);
        // When session disconnect update status anyway
        if let ServiceEvent::SessionClose { session_context } = event {
            let peer_id = session_context
                .remote_pubkey
                .as_ref()
                .map(PublicKey::peer_id)
                .expect("Secio must enabled");

            let peer_store = self.network_state.peer_store();
            if peer_store.peer_status(&peer_id) == Status::Connected {
                peer_store.update_status(&peer_id, Status::Disconnected);
            }
            self.network_state.drop_peer(context.control(), &peer_id);
        }
    }

    fn handle_proto(&mut self, context: &mut ServiceContext, event: ProtocolEvent) {
        // For special protocols: ping/discovery/identify
        if let ProtocolEvent::Connected {
            session_context,
            proto_id,
            version,
        } = event
        {
            let peer_id = session_context
                .remote_pubkey
                .as_ref()
                .map(PublicKey::peer_id)
                .expect("Secio must enabled");
            if let Ok(parsed_version) = version.parse::<ProtocolVersion>() {
                match self.network_state.accept_connection(
                    peer_id.clone(),
                    session_context.address.clone(),
                    session_context.id,
                    session_context.ty,
                    proto_id,
                    parsed_version,
                ) {
                    Ok(register_result) => {
                        // update status in peer_store
                        if let RegisterResult::New(_) = register_result {
                            let peer_store = self.network_state.peer_store();
                            peer_store.update_status(&peer_id, Status::Connected);
                        }
                    }
                    Err(err) => {
                        self.network_state.drop_peer(context.control(), &peer_id);
                        info!(
                            target: "network",
                            "reject connection from {} {}, because {:?}",
                            peer_id.to_base58(),
                            session_context.address,
                            err,
                        )
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
    pub fn new(network_state: Arc<NetworkState>, protocols: Vec<CKBProtocol>) -> NetworkService {
        let config = &network_state.config;

        // == Build special protocols

        // TODO: how to deny banned node to open those protocols?
        // Ping protocol
        let (ping_sender, ping_receiver) = channel(std::u8::MAX as usize);
        let ping_interval = Duration::from_secs(config.ping_interval_secs);
        let ping_timeout = Duration::from_secs(config.ping_timeout_secs);

        let ping_meta = MetaBuilder::default()
            .id(PING_PROTOCOL_ID)
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
            .id(DISCOVERY_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(DiscoveryProtocol::new(disc_sender.clone())))
            })
            .build();

        // Identify protocol
        let identify_callback = IdentifyCallback::new(Arc::clone(&network_state));
        let identify_meta = MetaBuilder::default()
            .id(IDENTIFY_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Both(Box::new(IdentifyProtocol::new(identify_callback.clone())))
            })
            .build();

        // Feeler protocol
        let feeler_protocol = CKBProtocol::new(
            "flr".to_string(),
            FEELER_PROTOCOL_ID,
            &[1][..],
            || Box::new(Feeler {}),
            Arc::clone(&network_state),
        );

        // == Build p2p service struct
        let mut protocol_metas = protocols
            .into_iter()
            .map(CKBProtocol::build)
            .collect::<Vec<_>>();
        protocol_metas.push(feeler_protocol.build());
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
        let mut p2p_service = service_builder
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

        let bootnodes = self
            .network_state
            .peer_store()
            .bootnodes(max((config.max_outbound_peers / 2) as u32, 1))
            .clone();
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
                let mut p2p_control_thread = self.p2p_service.control().clone();
                let mut runtime = Runtime::new().expect("Network tokio runtime init failed");
                runtime.spawn(self.p2p_service.for_each(|_| Ok(())));

                // NOTE: for ensure background task finished
                let mut bg_signals = Vec::new();
                for bg_service in self.bg_services.into_iter() {
                    let (signal_sender, signal_receiver) = oneshot::channel::<()>();
                    bg_signals.push(signal_sender);
                    runtime.spawn(
                        signal_receiver
                            .select2(bg_service)
                            .map(|_| ())
                            .map_err(|_| ()),
                    );
                }

                debug!(target: "network", "Shuting down network service");

                // Recevied stop signal, doing cleanup
                let _ = receiver.recv();
                self.network_state.drop_all(&mut p2p_control_thread);
                for signal in bg_signals.into_iter() {
                    let _ = signal.send(());
                }

                // TODO: not that gracefully shutdown, will output below error message:
                //       "terminate called after throwing an instance of 'std::system_error'"
                runtime.shutdown_now();
                debug!(target: "network", "Already shutdown network service");
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
    #[inline]
    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        self.network_state.external_urls(max_urls)
    }

    #[inline]
    pub fn node_id(&self) -> String {
        self.network_state.node_id()
    }

    #[inline]
    pub fn add_node(&self, peer_id: &PeerId, address: Multiaddr) {
        self.network_state.add_node(peer_id, address)
    }

    pub fn connected_peers(&self) -> Vec<(PeerId, Peer, MultiaddrList)> {
        let peer_store = self.network_state.peer_store();

        self.network_state
            .peers_registry
            .peers_guard()
            .read()
            .iter()
            .map(|(peer_id, peer)| {
                (
                    peer_id.clone(),
                    peer.clone(),
                    peer_store
                        .peer_addrs(peer_id, ADDR_LIMIT)
                        .unwrap_or_default()
                        .into_iter()
                    // FIXME how to return address score?
                        .map(|address| (address, 1))
                        .collect(),
                )
            })
            .collect()
    }

    pub fn with_protocol_context<F, T>(&self, protocol_id: ProtocolId, f: F) -> T
    where
        F: FnOnce(Box<dyn CKBProtocolContext>) -> T,
    {
        let context = Box::new(DefaultCKBProtocolContext::new(
            protocol_id,
            Arc::clone(&self.network_state),
            self.p2p_control.clone(),
        ));
        f(context)
    }
}

impl Drop for NetworkController {
    fn drop(&mut self) {
        // FIXME: should gracefully shutdown network in p2p library
        self.stop.try_send();
    }
}
