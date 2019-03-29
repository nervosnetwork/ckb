use crate::errors::{Error, ProtocolError};
use crate::peer_store::{sqlite::SqlitePeerStore, PeerStore};
use crate::peers_registry::{ConnectionStatus, PeersRegistry, RegisterResult};
use crate::protocol::ckb_handler::DefaultCKBProtocolContext;
use crate::protocols::outbound_peer::OutboundPeerProtocol;
use crate::service::{
    discovery_service::{DiscoveryProtocol, DiscoveryService},
    identify_service::IdentifyCallback,
    ping_service::PingService,
};
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, CKBProtocolContext, NetworkConfig, PeerIndex, ProtocolId,
    ProtocolVersion, ServiceContext, ServiceControl, SessionId, SessionType,
};
use ckb_util::RwLock;
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::mpsc::channel;
use futures::sync::{mpsc, oneshot};
use futures::Future;
use futures::Stream;
use log::{debug, error, info, warn};
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    multiaddr::{self, multihash::Multihash, Multiaddr},
    secio::PeerId,
    service::{DialProtocol, ProtocolHandle, Service, ServiceError, ServiceEvent},
    traits::ServiceHandle,
};
use p2p_identify::IdentifyProtocol;
use p2p_ping::PingHandler;
use secio;
use std::boxed::Box;
use std::cmp::max;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::usize;
use stop_handler::{SignalSender, StopHandler};
use tokio::runtime::Runtime;

const PING_PROTOCOL_ID: ProtocolId = 0;
const DISCOVERY_PROTOCOL_ID: ProtocolId = 1;
const IDENTIFY_PROTOCOL_ID: ProtocolId = 2;
pub const FEELER_PROTOCOL_ID: ProtocolId = 3;

const OUTBOUND_PROTOCOL_ID: ProtocolId = 3;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<u8>,
}

pub struct NetworkState {
    protocol_ids: RwLock<FnvHashSet<ProtocolId>>,
    pub(crate) peers_registry: RwLock<PeersRegistry>,
    peer_store: Arc<RwLock<dyn PeerStore>>,
    listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
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
            .public_addresses
            .iter()
            .map(|addr| (addr.to_owned(), std::u8::MAX))
            .collect();
        let peer_store: Arc<RwLock<dyn PeerStore>> = {
            let mut peer_store =
                SqlitePeerStore::file(config.peer_store_path().to_string_lossy().to_string())?;
            let bootnodes = config.bootnodes()?;
            for (peer_id, addr) in bootnodes {
                peer_store.add_bootnode(peer_id, addr);
            }
            Arc::new(RwLock::new(peer_store))
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
            peers_registry: RwLock::new(peers_registry),
            listened_addresses: RwLock::new(listened_addresses),
            original_listened_addresses: RwLock::new(Vec::new()),
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.to_public_key().peer_id(),
            protocol_ids: RwLock::new(FnvHashSet::default()),
        })
    }

    pub fn report(&self, peer_id: &PeerId, behaviour: Behaviour) {
        self.peer_store.write().report(peer_id, behaviour);
    }

    pub fn drop_peer(&self, peer_id: &PeerId) {
        debug!(target: "network", "drop peer {:?}", peer_id);
        self.peers_registry.write().drop_peer(&peer_id);
    }

    pub fn drop_all(&self) {
        debug!(target: "network", "drop all connections...");
        self.peers_registry.write().drop_all();
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
        let peers_registry = self.peers_registry.read();
        peers_registry.get(&peer_id).map(|peer| peer.peer_index)
    }

    pub(crate) fn get_peer_id(&self, peer_index: PeerIndex) -> Option<PeerId> {
        let peers_registry = self.peers_registry.read();
        peers_registry
            .get_peer_id(peer_index)
            .map(|peer_id| peer_id.to_owned())
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        let peers_registry = self.peers_registry.read();
        peers_registry.connection_status()
    }

    pub(crate) fn modify_peer<F>(&self, peer_id: &PeerId, f: F)
    where
        F: FnOnce(&mut Peer) -> (),
    {
        let mut peers_registry = self.peers_registry.write();
        if let Some(peer) = peers_registry.get_mut(peer_id) {
            f(peer);
        }
    }

    pub(crate) fn peers_indexes(&self) -> Vec<PeerIndex> {
        let peers_registry = self.peers_registry.read();
        let iter = peers_registry.connected_peers_indexes();
        iter.collect::<Vec<_>>()
    }

    #[inline]
    pub(crate) fn ban_peer(&self, peer_id: &PeerId, timeout: Duration) {
        self.drop_peer(peer_id);
        self.peer_store.write().ban_peer(peer_id, timeout);
    }

    #[inline]
    pub(crate) fn peer_store(&self) -> &RwLock<dyn PeerStore> {
        &self.peer_store
    }

    pub(crate) fn local_private_key(&self) -> &secio::SecioKeyPair {
        &self.local_private_key
    }

    pub fn external_urls(&self, max_urls: usize) -> Vec<(String, u8)> {
        let original_listened_addresses = self.original_listened_addresses.read();
        self.listened_addresses(max_urls.saturating_sub(original_listened_addresses.len()))
            .into_iter()
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
        let _ = self
            .peer_store()
            .write()
            .add_discovered_addr(peer_id, address);
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
        let mut peers_registry = self.peers_registry.write();
        let register_result = match session_type {
            SessionType::Client => peers_registry.try_outbound_peer(
                peer_id.clone(),
                connected_addr,
                session_id,
                session_type,
            ),
            SessionType::Server => peers_registry.accept_inbound_peer(
                peer_id.clone(),
                connected_addr,
                session_id,
                session_type,
            ),
        }?;
        // add session to peer
        match peers_registry.get_mut(&peer_id) {
            Some(peer) => match peer.protocol_version(protocol_id) {
                Some(_) => return Err(ProtocolError::Duplicate(protocol_id).into()),
                None => {
                    peer.protocols.insert(protocol_id, protocol_version);
                }
            },
            None => unreachable!("get peer after inserted"),
        }
        Ok(register_result)
    }

    pub fn peer_protocol_version(&self, peer_id: &PeerId, protocol_id: ProtocolId) -> Option<u8> {
        let peers_registry = self.peers_registry.read();
        peers_registry
            .get(peer_id)
            .and_then(|peer| peer.protocol_version(protocol_id))
    }
    pub fn session_info(&self, peer_id: &PeerId, protocol_id: ProtocolId) -> Option<SessionInfo> {
        let peers_registry = self.peers_registry.read();
        peers_registry.get(peer_id).map(|peer| {
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

    pub fn dial(&self, p2p_control: &mut ServiceControl, peer_id: &PeerId, mut addr: Multiaddr) {
        match Multihash::from_bytes(peer_id.as_bytes().to_vec()) {
            Ok(peer_id_hash) => {
                addr.append(multiaddr::Protocol::P2p(peer_id_hash));
                if let Err(err) = p2p_control.dial(addr.clone(), DialProtocol::All) {
                    debug!(target: "network", "dial fialed: {:?}", err);
                }
            }
            Err(err) => {
                error!(target: "network", "failed to convert peer_id to addr: {}", err);
            }
        }
    }
}

pub struct EventHandler {}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, _context: &mut ServiceContext, error: ServiceError) {
        debug!(target: "network", "p2p service error: {:?}", error);
    }

    fn handle_event(&mut self, _context: &mut ServiceContext, event: ServiceEvent) {
        debug!(target: "network", "p2p service event: {:?}", event);
    }
}

pub struct NetworkService {
    p2p_service: Service<EventHandler>,
    network_state: Arc<NetworkState>,
    // Background services
    bg_services: Vec<Box<dyn Future<Item = (), Error = ()> + Send + 'static>>,
}

impl NetworkService {
    pub fn new(network_state: Arc<NetworkState>, protocols: Vec<CKBProtocol>) -> NetworkService {
        let config = &network_state.config;

        // Ping protocol
        let (ping_sender, ping_receiver) = channel(std::u8::MAX as usize);
        let ping_meta = MetaBuilder::default()
            .id(PING_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(PingHandler::new(
                    PING_PROTOCOL_ID,
                    Duration::from_secs(config.ping_interval_secs),
                    Duration::from_secs(config.ping_timeout_secs),
                    ping_sender,
                )))
            })
            .build();
        let ping_service = PingService {
            network_state: Arc::clone(&network_state),
            event_receiver: ping_receiver,
        };

        // Discovery protocol
        let (disc_sender, disc_receiver) = mpsc::unbounded();
        let disc_meta = MetaBuilder::default()
            .id(DISCOVERY_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(DiscoveryProtocol::new(disc_sender)))
            })
            .build();
        let disc_service = DiscoveryService::new(Arc::clone(&network_state), disc_receiver);

        // Identify protocol
        let identify_callback = IdentifyCallback::new(Arc::clone(&network_state));
        let identify_meta = MetaBuilder::default()
            .id(IDENTIFY_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(IdentifyProtocol::new(identify_callback)))
            })
            .build();

        let outbound_peer_meta = MetaBuilder::default()
            .id(OUTBOUND_PROTOCOL_ID)
            .service_handle({
                let network_state = Arc::clone(&network_state);
                move || {
                    ProtocolHandle::Callback(Box::new(OutboundPeerProtocol::new(
                        network_state,
                        Duration::from_secs(config.connect_outbound_interval_secs),
                    )))
                }
            })
            .build();

        let mut service_builder = ServiceBuilder::default();
        for meta in protocols
            .into_iter()
            .map(|protocol| protocol.build())
            .chain(vec![ping_meta, disc_meta, identify_meta, outbound_peer_meta].into_iter())
        {
            network_state.protocol_ids.write().insert(meta.id());
            service_builder = service_builder.insert_protocol(meta);
        }

        let p2p_service = service_builder
            .key_pair(network_state.local_private_key.clone())
            .forever(true)
            .build(EventHandler {});

        let bg_services = vec![
            Box::new(ping_service.for_each(|_| Ok(()))) as Box<_>,
            Box::new(disc_service.for_each(|_| Ok(()))) as Box<_>,
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
                .dial(self.p2p_service.control(), &peer_id, addr);
        }

        let bootnodes = self
            .network_state
            .peer_store()
            .read()
            .bootnodes(max((config.max_outbound_peers / 2) as u32, 1))
            .clone();
        // dial half bootnodes
        for (peer_id, addr) in bootnodes {
            debug!(target: "network", "dial bootnode {:?} {:?}", peer_id, addr);
            self.network_state
                .dial(self.p2p_service.control(), &peer_id, addr);
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

                let _ = receiver.recv();
                for signal in bg_signals.into_iter() {
                    let _ = signal.send(());
                }
                debug!(target: "network", "Shuting down network service");
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
