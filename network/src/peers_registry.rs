use crate::network_group::{Group, NetworkGroup};
use crate::peer_store::PeerStore;
use crate::{Error, ErrorKind, PeerId, PeerIndex, ProtocolId};
use bytes::Bytes;
use ckb_util::RwLock;
use faketime::unix_time_as_millis;
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::mpsc::UnboundedSender;
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::ping;
use log::debug;
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub(crate) const EVICTION_PROTECT_PEERS: usize = 8;

struct PeerConnections {
    id_allocator: AtomicUsize,
    peers: FnvHashMap<PeerId, PeerConnection>,
    pub(crate) peer_id_by_index: FnvHashMap<PeerIndex, PeerId>,
}

impl PeerConnections {
    #[inline]
    fn get(&self, peer_id: &PeerId) -> Option<&PeerConnection> {
        self.peers.get(peer_id)
    }

    #[inline]
    fn get_peer_id(&self, peer_index: PeerIndex) -> Option<&PeerId> {
        self.peer_id_by_index.get(&peer_index)
    }

    #[inline]
    fn get_mut(&mut self, peer_id: &PeerId) -> Option<&mut PeerConnection> {
        self.peers.get_mut(peer_id)
    }

    #[inline]
    fn remove(&mut self, peer_id: &PeerId) -> Option<PeerConnection> {
        if let Some(peer) = self.peers.remove(peer_id) {
            self.peer_id_by_index.remove(&peer.peer_index.unwrap());
            return Some(peer);
        }
        None
    }

    #[inline]
    fn iter(&self) -> impl Iterator<Item = (&PeerId, &PeerConnection)> {
        self.peers.iter()
    }
    #[inline]
    fn or_insert(&mut self, peer_id: PeerId, peer: PeerConnection) -> PeerIndex {
        let mut peer = peer;
        let peer_index = match peer.peer_index {
            Some(peer_index) => peer_index,
            None => {
                let id = self.id_allocator.fetch_add(1, Ordering::Relaxed);
                peer.peer_index = Some(id);
                id
            }
        };
        self.peers.entry(peer_id.clone()).or_insert(peer);
        self.peer_id_by_index.entry(peer_index).or_insert(peer_id);
        peer_index
    }

    fn clear(&mut self) {
        self.peers.clear();
        self.peer_id_by_index.clear();
        self.id_allocator.store(0, Ordering::Relaxed)
    }
}

impl Default for PeerConnections {
    fn default() -> Self {
        PeerConnections {
            id_allocator: AtomicUsize::new(0),
            peers: FnvHashMap::with_capacity_and_hasher(20, Default::default()),
            peer_id_by_index: FnvHashMap::with_capacity_and_hasher(20, Default::default()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PeerIdentifyInfo {
    pub client_version: String,
    pub protocol_version: String,
    pub supported_protocols: Vec<String>,
    pub count_of_known_listen_addrs: usize,
}

type ProtocolConnec = (ProtocolId, UniqueConnec<(UnboundedSender<Bytes>, u8)>);

pub struct PeerConnection {
    pub(crate) peer_index: Option<PeerIndex>,
    pub connected_addr: Multiaddr,
    // Dialer or Listener
    pub endpoint_role: Endpoint,
    // Used for send ping to peer
    pub(crate) pinger_loader: UniqueConnec<ping::Pinger>,
    pub identify_info: Option<PeerIdentifyInfo>,
    pub(crate) ckb_protocols: Vec<ProtocolConnec>,
    pub last_ping_time: Option<u64>,
    pub last_message_time: Option<u64>,
    pub ping: Option<u64>,
    pub connected_time: Option<u64>,
}

impl PeerConnection {
    pub fn new(connected_addr: Multiaddr, endpoint_role: Endpoint) -> Self {
        PeerConnection {
            endpoint_role,
            connected_addr,
            pinger_loader: UniqueConnec::empty(),
            identify_info: None,
            ckb_protocols: Vec::with_capacity(1),
            ping: None,
            last_ping_time: None,
            last_message_time: None,
            connected_time: None,
            peer_index: None,
        }
    }

    #[inline]
    pub fn is_outbound(&self) -> bool {
        self.endpoint_role == Endpoint::Dialer
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_inbound(&self) -> bool {
        !self.is_outbound()
    }

    #[allow(dead_code)]
    #[inline]
    fn network_group(&self) -> Group {
        self.connected_addr.network_group()
    }
}

pub struct ConnectionStatus {
    pub total: u32,
    pub unreserved_inbound: u32,
    pub unreserved_outbound: u32,
    pub max_inbound: u32,
    pub max_outbound: u32,
}

pub(crate) struct PeersRegistry {
    // store all known peers
    peer_store: Arc<RwLock<dyn PeerStore>>,
    peers: PeerConnections,
    // max inbound limitation
    max_inbound: u32,
    // max outbound limitation
    max_outbound: u32,
    // Only reserved peers or allow all peers.
    reserved_only: bool,
    reserved_peers: FnvHashSet<PeerId>,
}

fn find_most_peers_in_same_network_group<'a>(
    peers: impl Iterator<Item = (&'a PeerId, &'a PeerConnection)>,
) -> Vec<&'a PeerId> {
    peers
        .fold(
            FnvHashMap::with_capacity_and_hasher(16, Default::default()),
            |mut groups, (peer_id, peer)| {
                groups
                    .entry(peer.network_group())
                    .or_insert_with(Vec::new)
                    .push(peer_id);
                groups
            },
        )
        .values()
        .max_by_key(|group| group.len())
        .cloned()
        .unwrap_or_else(Vec::new)
}

fn sort_then_drop_last_n_elements<T, F>(list: &mut Vec<T>, n: usize, compare: F)
where
    F: FnMut(&T, &T) -> std::cmp::Ordering,
{
    list.sort_by(compare);
    list.truncate(list.len().saturating_sub(n));
}

impl PeersRegistry {
    pub fn new(
        peer_store: Arc<RwLock<dyn PeerStore>>,
        max_inbound: u32,
        max_outbound: u32,
        reserved_only: bool,
        reserved_peers: Vec<PeerId>,
    ) -> Self {
        let mut reserved_peers_set =
            FnvHashSet::with_capacity_and_hasher(reserved_peers.len(), Default::default());
        for reserved_peer in reserved_peers {
            reserved_peers_set.insert(reserved_peer);
        }
        PeersRegistry {
            peer_store,
            peers: Default::default(),
            reserved_peers: reserved_peers_set,
            max_inbound,
            max_outbound,
            reserved_only,
        }
    }

    #[inline]
    pub fn get_peer_id(&self, peer_index: PeerIndex) -> Option<&PeerId> {
        self.peers.get_peer_id(peer_index)
    }

    pub fn is_reserved(&self, peer_id: &PeerId) -> bool {
        self.reserved_peers.contains(&peer_id)
    }

    pub fn accept_inbound_peer(&mut self, peer_id: PeerId, addr: Multiaddr) -> Result<(), Error> {
        if self.peers.get(&peer_id).is_some() {
            return Ok(());
        }
        if !self.is_reserved(&peer_id) {
            if self.reserved_only {
                return Err(ErrorKind::InvalidNewPeer(format!(
                    "We are in reserved_only mode, rejected non-reserved peer {:?}",
                    peer_id
                ))
                .into());
            }
            if self.peer_store.read().is_banned(&peer_id) {
                return Err(
                    ErrorKind::InvalidNewPeer(format!("peer {:?} is denied", peer_id)).into(),
                );
            }

            let connection_status = self.connection_status();
            // check peers connection limitation
            if connection_status.unreserved_inbound >= self.max_inbound
                && !self.try_evict_inbound_peer()
            {
                return Err(ErrorKind::InvalidNewPeer(format!(
                    "reach max inbound peers limitation, reject peer {:?}",
                    peer_id
                ))
                .into());
            }
        }
        self.new_peer(peer_id, addr, Endpoint::Listener);
        Ok(())
    }

    fn try_evict_inbound_peer(&mut self) -> bool {
        let peer_id: PeerId = {
            let mut candidate_peers = self
                .peers
                .iter()
                .filter(|(peer_id, peer)| peer.is_inbound() && !self.is_reserved(peer_id))
                .collect::<Vec<_>>();
            let peer_store = self.peer_store.read();
            // Protect peers based on characteristics that an attacker hard to simulate or manipulate
            // Protect peers which has the highest score
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                EVICTION_PROTECT_PEERS,
                |(peer_id1, _), (peer_id2, _)| {
                    let peer1_score = peer_store.peer_score(peer_id1).unwrap_or_default();
                    let peer2_score = peer_store.peer_score(peer_id2).unwrap_or_default();
                    peer1_score.cmp(&peer2_score)
                },
            );

            // Protect peers which has the lowest ping
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                EVICTION_PROTECT_PEERS,
                |(_, peer1), (_, peer2)| {
                    let peer1_ping = peer1.ping.unwrap_or_else(|| std::u64::MAX);
                    let peer2_ping = peer2.ping.unwrap_or_else(|| std::u64::MAX);
                    peer2_ping.cmp(&peer1_ping)
                },
            );

            // Protect peers which most recently sent messages
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                EVICTION_PROTECT_PEERS,
                |(_, peer1), (_, peer2)| {
                    let peer1_last_message_time = peer1.last_message_time.unwrap_or_default();
                    let peer2_last_message_time = peer2.last_message_time.unwrap_or_default();
                    peer1_last_message_time.cmp(&peer2_last_message_time)
                },
            );
            candidate_peers.sort_by(|(_, peer1), (_, peer2)| {
                let peer1_last_connected_at = peer1.connected_time.unwrap_or_else(|| std::u64::MAX);
                let peer2_last_connected_at = peer2.connected_time.unwrap_or_else(|| std::u64::MAX);
                peer2_last_connected_at.cmp(&peer1_last_connected_at)
            });
            // Protect half peers which have the longest connection time
            let protect_peers = candidate_peers.len() / 2;
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                protect_peers,
                |(_, peer1), (_, peer2)| {
                    let peer1_last_connected_at =
                        peer1.connected_time.unwrap_or_else(|| std::u64::MAX);
                    let peer2_last_connected_at =
                        peer2.connected_time.unwrap_or_else(|| std::u64::MAX);
                    peer2_last_connected_at.cmp(&peer1_last_connected_at)
                },
            );

            let mut evict_group =
                find_most_peers_in_same_network_group(candidate_peers.into_iter());
            let mut rng = thread_rng();
            evict_group.shuffle(&mut rng);
            // randomly evict a lowest scored peer
            match evict_group
                .iter()
                .min_by_key(|peer_id| peer_store.peer_score(peer_id).unwrap_or_default())
            {
                Some(peer_id) => peer_id.to_owned().to_owned(),
                None => return false,
            }
        };
        debug!(target: "network", "evict inbound peer {:?}", peer_id);
        self.drop_peer(&peer_id);
        true
    }

    pub fn try_outbound_peer(&mut self, peer_id: PeerId, addr: Multiaddr) -> Result<(), Error> {
        if self.peers.get(&peer_id).is_some() {
            return Ok(());
        }
        if !self.is_reserved(&peer_id) {
            if self.reserved_only {
                return Err(ErrorKind::InvalidNewPeer(format!(
                    "We are in reserved_only mode, rejected non-reserved peer {:?}",
                    peer_id
                ))
                .into());
            }
            if self.peer_store.read().is_banned(&peer_id) {
                return Err(
                    ErrorKind::InvalidNewPeer(format!("peer {:?} is denied", peer_id)).into(),
                );
            }
            let connection_status = self.connection_status();
            // check peers connection limitation
            // TODO: implement extra outbound peer logic
            if connection_status.unreserved_outbound >= self.max_outbound {
                return Err(ErrorKind::InvalidNewPeer(format!(
                    "reach max outbound peers limitation, reject peer {:?}",
                    peer_id
                ))
                .into());
            }
        }
        self.new_peer(peer_id, addr, Endpoint::Dialer);
        Ok(())
    }

    // registry a new peer
    #[allow(clippy::needless_pass_by_value)]
    fn new_peer(
        &mut self,
        peer_id: PeerId,
        connected_addr: Multiaddr,
        endpoint: Endpoint,
    ) -> PeerIndex {
        self.peer_store
            .write()
            .new_connected_peer(&peer_id, connected_addr.clone(), endpoint);
        let mut peer = PeerConnection::new(connected_addr, endpoint);
        peer.connected_time = Some(unix_time_as_millis());
        let peer_index = self.peers.or_insert(peer_id.clone(), peer);
        debug!(target: "network", "allocate peer_index {} to peer {:?}", peer_index, peer_id);
        peer_index
    }

    #[inline]
    pub fn peers_iter(&self) -> impl Iterator<Item = (&PeerId, &PeerConnection)> {
        self.peers.iter()
    }

    #[inline]
    pub fn get(&self, peer_id: &PeerId) -> Option<&PeerConnection> {
        self.peers.get(peer_id)
    }

    #[inline]
    pub fn get_mut(&mut self, peer_id: &PeerId) -> Option<&mut PeerConnection> {
        self.peers.get_mut(peer_id)
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        let mut total: u32 = 0;
        let mut unreserved_inbound: u32 = 0;
        let mut unreserved_outbound: u32 = 0;
        for (peer_id, peer_connection) in self.peers.iter() {
            total += 1;
            if self.is_reserved(peer_id) {
                continue;
            }
            if peer_connection.is_outbound() {
                unreserved_outbound += 1;
            } else {
                unreserved_inbound += 1;
            }
        }
        ConnectionStatus {
            total,
            unreserved_inbound,
            unreserved_outbound,
            max_inbound: self.max_inbound,
            max_outbound: self.max_outbound,
        }
    }

    #[inline]
    pub fn connected_peers_indexes(&self) -> impl Iterator<Item = PeerIndex> + '_ {
        self.peers.peer_id_by_index.iter().map(|(k, _v)| *k)
    }

    #[inline]
    pub fn drop_peer(&mut self, peer_id: &PeerId) {
        self.peers.remove(peer_id);
    }

    #[inline]
    pub fn drop_all(&mut self) {
        debug!(target: "network", "drop_all");
        self.peers.clear()
    }
}
