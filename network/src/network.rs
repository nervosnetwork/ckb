use crate::errors::{Error, PeerError, ProtocolError};
use crate::peer_store::{sqlite::SqlitePeerStore, PeerStore};
use crate::peers_registry::{ConnectionStatus, PeersRegistry, RegisterResult};
use crate::protocol::ckb::Version as ProtocolVersion;
use crate::protocol::ckb_handler::{CKBProtocolHandler, DefaultCKBProtocolContext};
use crate::service::{
    ckb_service::CKBService,
    discovery_service::{DiscoveryEvent, DiscoveryProtocol, DiscoveryService},
    identify_service::{IdentifyAddressManager, IdentifyEvent, IdentifyProtocol, IdentifyService},
    outbound_peer_service::OutboundPeerService,
    ping_service::PingService,
    timer_service::{TimerRegistry, TimerService},
};
use crate::Peer;
use crate::{
    Behaviour, CKBEvent, CKBProtocol, NetworkConfig, PeerIndex, ProtocolId, ServiceContext,
    ServiceControl, SessionId, SessionType,
};
use bytes::Bytes;
use ckb_util::{Mutex, RwLock};
use fnv::FnvHashMap;
use futures::future::{select_all, Future};
use futures::sync::mpsc;
use futures::sync::mpsc::channel;
use futures::sync::mpsc::Receiver;
use futures::sync::oneshot;
use futures::Stream;
use log::{debug, error, info, warn};
use multiaddr::multihash::Multihash;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    multiaddr::{self, Multiaddr},
    secio::{PeerId, PublicKey},
    service::{DialProtocol, ProtocolHandle, Service, ServiceError, ServiceEvent},
    traits::ServiceHandle,
};
use p2p_ping::{Event as PingEvent, PingHandler};
use secio;
use std::boxed::Box;
use std::cmp::max;
use std::sync::Arc;
use std::time::Duration;
use std::usize;

const PING_PROTOCOL_ID: ProtocolId = 0;
const DISCOVERY_PROTOCOL_ID: ProtocolId = 1;
const IDENTIFY_PROTOCOL_ID: ProtocolId = 2;
pub const FEELER_PROTOCOL_ID: ProtocolId = 3;

pub type CKBProtocols = Vec<(CKBProtocol, Arc<dyn CKBProtocolHandler>)>;
type NetworkResult = Result<
    (
        Arc<Network>,
        oneshot::Sender<()>,
        Box<Future<Item = (), Error = Error> + Send>,
    ),
    Error,
>;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<u8>,
}

pub struct EventReceivers {
    pub ping_event_receiver: Receiver<PingEvent>,
    pub disc_event_receiver: mpsc::UnboundedReceiver<DiscoveryEvent>,
    pub identify_event_receiver: mpsc::UnboundedReceiver<IdentifyEvent>,
}

type P2PService = Service<EventHandler>;

pub struct Network {
    pub(crate) peers_registry: RwLock<PeersRegistry>,
    peer_store: Arc<RwLock<dyn PeerStore>>,
    listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    pub(crate) ckb_protocols: CKBProtocols,
    local_private_key: secio::SecioKeyPair,
    local_peer_id: PeerId,
    p2p_control: RwLock<ServiceControl>,
}

impl Network {
    pub fn find_protocol(
        &self,
        id: ProtocolId,
        version: ProtocolVersion,
    ) -> Option<(&CKBProtocol, Arc<dyn CKBProtocolHandler>)> {
        self.ckb_protocols
            .iter()
            .find(|(protocol, _)| protocol.id() == id && protocol.match_version(version))
            .map(|(protocol, handler)| (protocol, Arc::clone(handler)))
    }

    pub fn find_protocol_without_version(
        &self,
        id: ProtocolId,
    ) -> Option<(&CKBProtocol, Arc<dyn CKBProtocolHandler>)> {
        self.ckb_protocols
            .iter()
            .find(|(protocol, _)| protocol.id() == id)
            .map(|(protocol, handler)| (protocol, Arc::clone(handler)))
    }

    pub fn report(&self, peer_id: &PeerId, behaviour: Behaviour) {
        self.peer_store.write().report(peer_id, behaviour);
    }

    pub fn drop_peer(&self, peer_id: &PeerId) {
        debug!(target: "network", "drop peer {:?}", peer_id);
        if let Some(peer) = self.peers_registry.write().drop_peer(&peer_id) {
            let mut p2p_control = self.p2p_control.write();
            if let Err(err) = p2p_control.disconnect(peer.session_id) {
                error!(target: "network", "disconnect peer error {:?}", err);
            }
        }
    }

    pub fn drop_all(&self) {
        debug!(target: "network", "drop all connections...");
        let mut peers_registry = self.peers_registry.write();
        let mut p2p_control = self.p2p_control.write();
        for (_peer_id, peer) in peers_registry.peers_iter() {
            if let Err(err) = p2p_control.disconnect(peer.session_id) {
                error!(target: "network", "disconnect peer error {:?}", err);
            }
        }
        peers_registry.drop_all();
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

    pub(crate) fn local_public_key(&self) -> PublicKey {
        self.local_private_key.to_public_key()
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

    pub(crate) fn send(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
        data: Bytes,
    ) -> Result<(), Error> {
        self.peers_registry
            .read()
            .get(peer_id)
            .map(|peer| match peer.protocol_version(protocol_id) {
                Some(_) => self
                    .p2p_control
                    .write()
                    .send_message(peer.session_id, protocol_id, data.to_vec())
                    .map_err(|_| {
                        Error::P2P(format!(
                            "error send to peer {:?} protocol {}",
                            peer_id, protocol_id
                        ))
                    }),
                None => Err(PeerError::ProtocolNotFound(peer_id.to_owned(), protocol_id).into()),
            })
            .unwrap_or_else(|| Err(PeerError::NotFound(peer_id.to_owned()).into()))
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

    fn inner_dial(
        &self,
        expected_peer_id: &PeerId,
        mut addr: Multiaddr,
        dial_protocol: DialProtocol,
    ) -> Result<(), Error> {
        if expected_peer_id == self.local_peer_id() {
            debug!(target: "network", "ignore dial to self");
            return Ok(());
        }
        debug!(target: "network", "dial to peer {:?} address {:?}", expected_peer_id, addr);
        let p2p_phase =
            Multihash::from_bytes(expected_peer_id.as_bytes().to_vec()).map_err(|_| {
                Error::P2P(format!("peer_id is invalid {:?}", expected_peer_id).to_string())
            })?;
        addr.append(multiaddr::Protocol::P2p(p2p_phase));
        self.p2p_control
            .write()
            .dial(addr, dial_protocol)
            .map_err(|_| Error::P2P("dial error".to_string()))
    }

    pub fn dial(&self, expected_peer_id: &PeerId, addr: Multiaddr) -> Result<(), Error> {
        self.inner_dial(expected_peer_id, addr, {
            let mut protocols = vec![
                PING_PROTOCOL_ID,
                DISCOVERY_PROTOCOL_ID,
                IDENTIFY_PROTOCOL_ID,
            ];
            protocols.extend(
                // TODO remove the filter after refactor
                self.ckb_protocols
                    .iter()
                    .map(|p| p.0.id())
                    .filter(|id| *id != FEELER_PROTOCOL_ID)
                    .collect::<Vec<_>>(),
            );
            DialProtocol::Multi(protocols)
        })
    }

    pub fn dial_feeler(&self, expected_peer_id: &PeerId, addr: Multiaddr) -> Result<(), Error> {
        self.inner_dial(
            expected_peer_id,
            addr,
            DialProtocol::Single(FEELER_PROTOCOL_ID),
        )
    }

    pub(crate) fn inner_build(
        config: &NetworkConfig,
        ckb_protocols: CKBProtocols,
    ) -> Result<(Arc<Self>, P2PService, TimerRegistry, EventReceivers), Error> {
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
        // register protocols
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

        let (disc_sender, disc_receiver) = mpsc::unbounded();
        let disc_meta = MetaBuilder::default()
            .id(DISCOVERY_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(DiscoveryProtocol::new(
                    DISCOVERY_PROTOCOL_ID,
                    disc_sender,
                )))
            })
            .build();

        let (identify_sender, identify_receiver) = mpsc::unbounded();
        let identify_addr_mgr = IdentifyAddressManager::new(identify_sender.clone());
        let identify_meta = MetaBuilder::default()
            .id(IDENTIFY_PROTOCOL_ID)
            .service_handle(move || {
                ProtocolHandle::Callback(Box::new(IdentifyProtocol::new(
                    IDENTIFY_PROTOCOL_ID,
                    identify_addr_mgr.clone(),
                )))
            })
            .build();

        let mut p2p_service = ServiceBuilder::default()
            .forever(true)
            .insert_protocol(ping_meta)
            .insert_protocol(disc_meta)
            .insert_protocol(identify_meta);
        for (ckb_protocol, _) in &ckb_protocols {
            p2p_service = p2p_service.insert_protocol(ckb_protocol.build());
        }
        let mut p2p_service = p2p_service
            .key_pair(local_private_key.clone())
            .build(EventHandler {});

        let p2p_control = p2p_service.control().clone();
        let network: Arc<Network> = Arc::new(Network {
            peers_registry: RwLock::new(peers_registry),
            peer_store: Arc::clone(&peer_store),
            listened_addresses: RwLock::new(listened_addresses),
            original_listened_addresses: RwLock::new(Vec::new()),
            ckb_protocols,
            local_private_key: local_private_key.clone(),
            local_peer_id: local_private_key.to_public_key().peer_id(),
            p2p_control: RwLock::new(p2p_control.clone()),
        });

        let timer_registry = Arc::new(Mutex::new(Some(Vec::new())));
        // Transport used to handling received connections
        for (protocol, handler) in &network.ckb_protocols {
            handler.initialize(Box::new(DefaultCKBProtocolContext::with_timer_registry(
                Arc::clone(&network),
                protocol.id(),
                Arc::clone(&timer_registry),
            )));
        }
        // listen local addresses
        for addr in &config.listen_addresses {
            match p2p_service.listen(addr.to_owned()) {
                Ok(listen_address) => {
                    info!(
                    target: "network",
                    "Listen on address: {}",
                    network.to_external_url(&listen_address)
                    );
                    network
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

        // dial reserved nodes and bootnodes
        {
            let network = Arc::clone(&network);
            // dial reserved_nodes
            for (peer_id, addr) in config.reserved_peers()? {
                let _ = network.dial(&peer_id, addr);
            }

            let bootnodes = network
                .peer_store()
                .read()
                .bootnodes(max((config.max_outbound_peers / 2) as u32, 1))
                .clone();
            // dial half bootnodes
            for (peer_id, addr) in bootnodes {
                debug!(target: "network", "dial bootnode {:?} {:?}", peer_id, addr);
                let _ = network.dial(&peer_id, addr);
            }
        }

        Ok((
            network,
            p2p_service,
            timer_registry,
            EventReceivers {
                ping_event_receiver: ping_receiver,
                disc_event_receiver: disc_receiver,
                identify_event_receiver: identify_receiver,
            },
        ))
    }

    pub(crate) fn build_network_future(
        network: Arc<Network>,
        config: &NetworkConfig,
        close_rx: oneshot::Receiver<()>,
        p2p_service: P2PService,
        timer_registry: TimerRegistry,
        ckb_event_receiver: Receiver<CKBEvent>,
        receivers: EventReceivers,
    ) -> Result<Box<Future<Item = (), Error = Error> + Send>, Error> {
        // initialize ckb_protocols
        let ping_service = PingService {
            network: Arc::clone(&network),
            event_receiver: receivers.ping_event_receiver,
        };
        let disc_service =
            DiscoveryService::new(Arc::clone(&network), receivers.disc_event_receiver);
        let identify_service =
            IdentifyService::new(Arc::clone(&network), receivers.identify_event_receiver);

        let ckb_service = CKBService {
            network: Arc::clone(&network),
            event_receiver: ckb_event_receiver,
        };
        let timer_service = TimerService::new(timer_registry, Arc::clone(&network));
        let outbound_peer_service = OutboundPeerService::new(
            Arc::clone(&network),
            Duration::from_secs(config.connect_outbound_interval_secs),
        );
        // prepare services futures
        let futures: Vec<Box<Future<Item = (), Error = Error> + Send>> = vec![
            Box::new(
                p2p_service
                    .for_each(|_| Ok(()))
                    .map_err(|_err| Error::Shutdown),
            ),
            Box::new(
                ckb_service
                    .for_each(|_| Ok(()))
                    .map_err(|_err| Error::Shutdown),
            ),
            Box::new(
                ping_service
                    .for_each(|_| Ok(()))
                    .map_err(|_err| Error::Shutdown),
            ),
            Box::new(
                disc_service
                    .for_each(|_| Ok(()))
                    .map_err(|_err| Error::Shutdown),
            ),
            Box::new(
                identify_service
                    .for_each(|_| Ok(()))
                    .map_err(|_err| Error::Shutdown),
            ),
            Box::new(timer_service.timer_futures.for_each(|_| Ok(()))),
            Box::new(
                outbound_peer_service
                    .for_each(|_| Ok(()))
                    .map_err(|_| Error::Shutdown),
            ),
            Box::new(close_rx.map_err(|_err| Error::Shutdown)),
        ];
        let service_futures = select_all(futures)
            .and_then({
                let network = Arc::clone(&network);
                move |_| {
                    network.drop_all();
                    debug!(target: "network", "network shutdown");
                    Ok(())
                }
            })
            .map_err(|(err, _, _)| {
                debug!(target: "network", "network exit, error {:?}", err);
                err
            });
        let service_futures =
            Box::new(service_futures) as Box<Future<Item = (), Error = Error> + Send>;
        Ok(service_futures)
    }

    pub fn build(
        config: &NetworkConfig,
        ckb_protocols: CKBProtocols,
        ckb_event_receiver: Receiver<CKBEvent>,
    ) -> NetworkResult {
        let (network, p2p_service, timer_registry, receivers) =
            Self::inner_build(config, ckb_protocols)?;
        let (close_tx, close_rx) = oneshot::channel();
        let network_future = Self::build_network_future(
            Arc::clone(&network),
            &config,
            close_rx,
            p2p_service,
            timer_registry,
            ckb_event_receiver,
            receivers,
        )?;
        Ok((network, close_tx, network_future))
    }
}

pub struct EventHandler {}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, _env: &mut ServiceContext, error: ServiceError) {
        debug!(target: "network", "p2p service error: {:?}", error);
    }

    fn handle_event(&mut self, _env: &mut ServiceContext, event: ServiceEvent) {
        debug!(target: "network", "p2p service event: {:?}", event);
    }
}
