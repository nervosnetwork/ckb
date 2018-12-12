use crate::network_group::{Group, NetworkGroup};
use crate::peer_store::PeerStore;
use crate::{Error, ErrorKind, PeerId, PeerIndex, ProtocolId};
use bytes::Bytes;
use ckb_util::RwLock;
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::mpsc::UnboundedSender;
use libp2p::core::{Endpoint, Multiaddr, UniqueConnec};
use libp2p::ping;
use log::debug;
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

struct PeerConnections {
    id_allocator: AtomicUsize,
    peers: FnvHashMap<PeerId, PeerConnection>,
    pub(crate) peer_id_by_index: FnvHashMap<PeerIndex, PeerId>,
}

impl PeerConnections {
    #[inline]
    fn get<'a>(&'a self, peer_id: &PeerId) -> Option<&'a PeerConnection> {
        self.peers.get(peer_id)
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    fn get_peer_id<'a>(&'a self, peer_index: PeerIndex) -> Option<&'a PeerId> {
        self.peer_id_by_index.get(&peer_index)
    }

    #[inline]
    fn get_mut<'a>(&'a mut self, peer_id: &PeerId) -> Option<&'a mut PeerConnection> {
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

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    fn iter<'a>(&'a self) -> impl Iterator<Item = (&'a PeerId, &'a PeerConnection)> {
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
    pub last_ping_time: Option<Instant>,
}

impl PeerConnection {
    pub fn new(connected_addr: Multiaddr, endpoint_role: Endpoint) -> Self {
        PeerConnection {
            endpoint_role,
            connected_addr,
            pinger_loader: UniqueConnec::empty(),
            identify_info: None,
            ckb_protocols: Vec::with_capacity(1),
            last_ping_time: None,
            peer_index: None,
        }
    }

    #[inline]
    pub fn is_outgoing(&self) -> bool {
        self.endpoint_role == Endpoint::Dialer
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_incoming(&self) -> bool {
        !self.is_outgoing()
    }

    #[allow(dead_code)]
    #[inline]
    fn network_group(&self) -> Group {
        self.connected_addr.network_group()
    }
}

pub struct ConnectionStatus {
    pub total: u32,
    pub unreserved_incoming: u32,
    pub unreserved_outgoing: u32,
    pub max_incoming: u32,
    pub max_outgoing: u32,
}

pub(crate) struct PeersRegistry {
    // store all known peers
    peer_store: Arc<RwLock<Box<PeerStore>>>,
    peers: PeerConnections,
    // max incoming limitation
    max_incoming: u32,
    // max outgoing limitation
    max_outgoing: u32,
    // Only reserved peers or allow all peers.
    reserved_only: bool,
    reserved_peers: FnvHashSet<PeerId>,
}

fn find_most_peers_in_same_network_group<'a>(
    peers: impl Iterator<Item = (&'a PeerId, &'a PeerConnection)>,
) -> Vec<(&'a PeerId, &'a PeerConnection)> {
    let mut groups: FnvHashMap<Group, Vec<(&'a PeerId, &'a PeerConnection)>> =
        FnvHashMap::with_capacity_and_hasher(16, Default::default());
    let largest_group_len = 0;
    let mut largest_group: Group = Default::default();

    for (peer_id, peer) in peers {
        let group_name = peer.network_group();
        let mut group = groups.entry(group_name.clone()).or_insert_with(Vec::new);
        group.push((peer_id, peer));
        if group.len() > largest_group_len {
            largest_group = group_name;
        }
    }
    groups[&largest_group].clone()
}

impl PeersRegistry {
    pub fn new(
        peer_store: Arc<RwLock<Box<PeerStore>>>,
        max_incoming: u32,
        max_outgoing: u32,
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
            max_incoming,
            max_outgoing,
            reserved_only,
        }
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    pub fn get_peer_id<'a>(&'a self, peer_index: PeerIndex) -> Option<&'a PeerId> {
        self.peers.get_peer_id(peer_index)
    }

    pub fn accept_inbound_peer(&mut self, peer_id: PeerId, addr: Multiaddr) -> Result<(), Error> {
        if self.peers.get(&peer_id).is_some() {
            return Ok(());
        }
        let is_reserved = self.reserved_peers.contains(&peer_id);
        if !is_reserved {
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
            if connection_status.unreserved_incoming >= self.max_incoming
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
            let inbound_peers = self.peers.iter().filter(|(_, peer)| peer.is_incoming());
            let candidate_peers = find_most_peers_in_same_network_group(inbound_peers);
            let peer_store = self.peer_store.read();

            let mut lowest_score = std::i32::MAX;
            let mut low_score_peers = Vec::new();
            for (peer_id, _peer) in candidate_peers {
                if let Some(score) = peer_store.peer_score(peer_id) {
                    if score > lowest_score {
                        continue;
                    }
                    if score < lowest_score {
                        lowest_score = score;
                        low_score_peers.clear();
                    }

                    low_score_peers.push(peer_id);
                }
            }
            // failed to evict
            if low_score_peers.is_empty() {
                return false;
            }
            let mut rng = thread_rng();
            low_score_peers[..]
                .choose(&mut rng)
                .unwrap()
                .to_owned()
                .to_owned()
        };
        debug!("evict inbound peer {:?}", peer_id);
        self.drop_peer(&peer_id);
        true
    }

    pub fn try_outbound_peer(&mut self, peer_id: PeerId, addr: Multiaddr) -> Result<(), Error> {
        if self.peers.get(&peer_id).is_some() {
            return Ok(());
        }
        let is_reserved = self.reserved_peers.contains(&peer_id);
        if !is_reserved {
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
            if connection_status.unreserved_outgoing >= self.max_outgoing {
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
        let peer = PeerConnection::new(connected_addr, endpoint);
        let peer_index = self.peers.or_insert(peer_id.clone(), peer);
        debug!(target: "network", "allocate peer_index {} to peer {:?}", peer_index, peer_id);
        peer_index
    }

    #[allow(clippy::needless_lifetimes)]
    #[inline]
    pub fn peers_iter<'a>(&'a self) -> impl Iterator<Item = (&'a PeerId, &'a PeerConnection)> {
        self.peers.iter()
    }

    #[inline]
    pub fn get<'a>(&'a self, peer_id: &PeerId) -> Option<&'a PeerConnection> {
        self.peers.get(peer_id)
    }

    #[inline]
    pub fn get_mut<'a>(&'a mut self, peer_id: &PeerId) -> Option<&'a mut PeerConnection> {
        self.peers.get_mut(peer_id)
    }

    pub fn connection_status(&self) -> ConnectionStatus {
        let mut total: u32 = 0;
        let mut unreserved_incoming: u32 = 0;
        let mut unreserved_outgoing: u32 = 0;
        for (_, peer_connection) in self.peers.iter() {
            total += 1;
            if peer_connection.is_outgoing() {
                unreserved_outgoing += 1;
            } else {
                unreserved_incoming += 1;
            }
        }
        ConnectionStatus {
            total,
            unreserved_incoming,
            unreserved_outgoing,
            max_incoming: self.max_incoming,
            max_outgoing: self.max_outgoing,
        }
    }

    #[inline]
    pub fn connected_peers_indexes<'a>(&'a self) -> impl Iterator<Item = PeerIndex> + 'a {
        Box::new(self.peers.peer_id_by_index.iter().map(|(k, _v)| *k))
    }

    #[inline]
    pub fn drop_peer(&mut self, peer_id: &PeerId) {
        self.peers.remove(peer_id);
    }

    #[inline]
    pub fn drop_all(&mut self) {
        debug!(target: "network", "drop_all");
        self.peers = Default::default();
    }
}
