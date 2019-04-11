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
use crate::MultiaddrList;
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, CKBProtocolContext, NetworkConfig, PeerIndex, ProtocolId,
    ProtocolVersion, PublicKey, ServiceContext, ServiceControl, SessionId, SessionType,
};
use crate::{DISCOVERY_PROTOCOL_ID, FEELER_PROTOCOL_ID, IDENTIFY_PROTOCOL_ID, PING_PROTOCOL_ID};
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

const FAILED_DIAL_CACHE_SIZE: usize = 100;
const ADDR_LIMIT: u32 = 3;

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub peer: Peer,
    pub protocol_version: Option<ProtocolVersion>,
}

pub struct NetworkState {
    pub(crate) protocol_ids: RwLock<FnvHashSet<ProtocolId>>,
    pub(crate) peers_registry: RwLock<PeersRegistry>,
    peer_store: Arc<RwLock<dyn PeerStore>>,
    pub(crate) listened_addresses: RwLock<FnvHashMap<Multiaddr, u8>>,
    pub(crate) original_listened_addresses: RwLock<Vec<Multiaddr>>,
    // For avoid repeat failed dial
    pub(crate) failed_dials: RwLock<LruCache<PeerId, Instant>>,
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
                peer_store.report(&peer_id, Behaviour::UnexpectedDisconnect);
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
    pub fn dial_node(&self, peer_id: &PeerId, address: Multiaddr) {
        self.add_discovered_addr(peer_id, address);
    }

    pub fn add_discovered_addr(&self, peer_id: &PeerId, addr: Multiaddr) {
        self.peer_store().write().add_discovered_addr(peer_id, addr);
    }

    pub fn to_external_url(&self, addr: &Multiaddr) -> String {
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

