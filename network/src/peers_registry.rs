use crate::peer_store::PeerStore;
use crate::{
    errors::{Error, PeerError},
    Peer, PeerId, PeerIndex, ProtocolId, ProtocolVersion, SessionType,
};
use ckb_util::RwLock;
use fnv::{FnvHashMap, FnvHashSet};
use log::debug;
use p2p::{multiaddr::Multiaddr, SessionId};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub(crate) const EVICTION_PROTECT_PEERS: usize = 8;

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum RegisterResult {
    New(PeerIndex),
    Exist(PeerIndex),
}

impl RegisterResult {
    pub fn peer_index(&self) -> PeerIndex {
        match self {
            RegisterResult::New(peer_index) => *peer_index,
            RegisterResult::Exist(peer_index) => *peer_index,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConnectionStatus {
    pub total: u32,
    pub unreserved_inbound: u32,
    pub unreserved_outbound: u32,
    pub max_inbound: u32,
    pub max_outbound: u32,
}

pub(crate) struct PeersRegistry {
    id_allocator: AtomicUsize,
    peers: RwLock<FnvHashMap<PeerId, Peer>>,
    peer_id_by_index: RwLock<FnvHashMap<PeerIndex, PeerId>>,
    peer_store: Arc<dyn PeerStore>,
    // max inbound limitation
    max_inbound: u32,
    // max outbound limitation
    max_outbound: u32,
    // Only reserved peers or allow all peers.
    reserved_only: bool,
    reserved_peers: FnvHashSet<PeerId>,
}

fn find_most_peers_in_same_network_group<'a>(
    peers: impl Iterator<Item = (&'a PeerId, &'a Peer)>,
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
        peer_store: Arc<dyn PeerStore>,
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
            id_allocator: AtomicUsize::new(0),
            peers: RwLock::new(FnvHashMap::with_capacity_and_hasher(20, Default::default())),
            peer_id_by_index: RwLock::new(FnvHashMap::with_capacity_and_hasher(
                20,
                Default::default(),
            )),
            peer_store,
            reserved_peers: reserved_peers_set,
            max_inbound,
            max_outbound,
            reserved_only,
        }
    }

    pub fn get_peer_id(&self, peer_index: PeerIndex) -> Option<PeerId> {
        self.peer_indexes_guard().read().get(&peer_index).cloned()
    }

    pub fn is_reserved(&self, peer_id: &PeerId) -> bool {
        self.reserved_peers.contains(&peer_id)
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
        let mut peers = self.peers.write();

        if let Some(peer) = peers.get_mut(&peer_id) {
            peer.protocols.insert(protocol_id, protocol_version);
            return Ok(RegisterResult::Exist(peer.peer_index));
        }

        let inbound = session_type.is_inbound();
        let mut peer_id_by_index = self.peer_id_by_index.write();

        if !self.is_reserved(&peer_id) {
            if self.reserved_only {
                return Err(Error::Peer(PeerError::NonReserved(peer_id)));
            }
            // ban_list lock acquired
            if self.peer_store.is_banned(&connected_addr) {
                return Err(Error::Peer(PeerError::Banned(peer_id)));
            }

            let connection_status = self._connection_status(peers.iter());
            // check peers connection limitation
            if inbound {
                if connection_status.unreserved_inbound >= self.max_inbound
                    && !self._try_evict_inbound_peer(&mut peers, &mut peer_id_by_index)
                {
                    return Err(Error::Peer(PeerError::ReachMaxInboundLimit(peer_id)));
                }
            } else if connection_status.unreserved_outbound >= self.max_outbound {
                return Err(Error::Peer(PeerError::ReachMaxOutboundLimit(peer_id)));
            }
        }
        self.peer_store
            .add_connected_peer(&peer_id, connected_addr.clone(), session_type);
        let peer_index = self.id_allocator.fetch_add(1, Ordering::Relaxed);
        let mut peer = Peer::new(peer_index, connected_addr, session_id, session_type);
        peer.protocols.insert(protocol_id, protocol_version);
        peers.insert(peer_id.clone(), peer);
        peer_id_by_index.insert(peer_index, peer_id);
        Ok(RegisterResult::New(peer_index))
    }

    fn _try_evict_inbound_peer(
        &self,
        peers: &mut FnvHashMap<PeerId, Peer>,
        peer_id_by_index: &mut FnvHashMap<PeerIndex, PeerId>,
    ) -> bool {
        let peer_id: PeerId = {
            let mut candidate_peers = {
                peers
                    .iter()
                    .filter(|(peer_id, peer)| peer.is_inbound() && !self.is_reserved(peer_id))
                    .collect::<Vec<_>>()
            };
            // Protect peers based on characteristics that an attacker hard to simulate or manipulate
            // Protect peers which has the highest score
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                EVICTION_PROTECT_PEERS,
                |(peer_id1, _), (peer_id2, _)| {
                    let peer1_score = self.peer_store.peer_score(peer_id1).unwrap_or_default();
                    let peer2_score = self.peer_store.peer_score(peer_id2).unwrap_or_default();
                    peer1_score.cmp(&peer2_score)
                },
            );

            // Protect peers which has the lowest ping
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                EVICTION_PROTECT_PEERS,
                |(_, peer1), (_, peer2)| {
                    let peer1_ping = peer1
                        .ping
                        .map(|p| p.as_secs())
                        .unwrap_or_else(|| std::u64::MAX);
                    let peer2_ping = peer2
                        .ping
                        .map(|p| p.as_secs())
                        .unwrap_or_else(|| std::u64::MAX);
                    peer2_ping.cmp(&peer1_ping)
                },
            );

            // Protect peers which most recently sent messages
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                EVICTION_PROTECT_PEERS,
                |(_, peer1), (_, peer2)| {
                    let peer1_last_message = peer1
                        .last_message_time
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or_else(|| std::u64::MAX);
                    let peer2_last_message = peer2
                        .last_message_time
                        .map(|t| t.elapsed().as_secs())
                        .unwrap_or_else(|| std::u64::MAX);
                    peer2_last_message.cmp(&peer1_last_message)
                },
            );
            // Protect half peers which have the longest connection time
            let protect_peers = candidate_peers.len() / 2;
            sort_then_drop_last_n_elements(
                &mut candidate_peers,
                protect_peers,
                |(_, peer1), (_, peer2)| peer2.connected_time.cmp(&peer1.connected_time),
            );

            let mut evict_group =
                find_most_peers_in_same_network_group(candidate_peers.into_iter());
            let mut rng = thread_rng();
            evict_group.shuffle(&mut rng);
            // randomly evict a lowest scored peer
            match evict_group
                .iter()
                .min_by_key(|peer_id| self.peer_store.peer_score(peer_id).unwrap_or_default())
            {
                Some(peer_id) => peer_id.to_owned().to_owned(),
                None => return false,
            }
        };
        debug!(target: "network", "evict inbound peer {:?}", peer_id);
        self._drop_peer(&peer_id, peers, peer_id_by_index);
        true
    }

    pub fn modify_peer<R>(
        &self,
        peer_id: &PeerId,
        callback: impl FnOnce(&mut Peer) -> R,
    ) -> Option<R> {
        self.peers.write().get_mut(peer_id).map(callback)
    }

    pub fn _connection_status<'a>(
        &self,
        peers: impl Iterator<Item = (&'a PeerId, &'a Peer)>,
    ) -> ConnectionStatus {
        let mut total: u32 = 0;
        let mut unreserved_inbound: u32 = 0;
        let mut unreserved_outbound: u32 = 0;
        for (peer_id, peer_connection) in peers {
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

    pub fn connection_status(&self) -> ConnectionStatus {
        self._connection_status(self.peers.read().iter())
    }

    #[inline]
    pub fn connected_peers_indexes(&self) -> Vec<PeerIndex> {
        self.peer_id_by_index
            .read()
            .iter()
            .map(|(k, _v)| *k)
            .collect::<Vec<_>>()
    }

    fn _drop_peer(
        &self,
        peer_id: &PeerId,
        peers: &mut FnvHashMap<PeerId, Peer>,
        peer_id_by_index: &mut FnvHashMap<PeerIndex, PeerId>,
    ) -> Option<Peer> {
        if let Some(peer) = peers.remove(peer_id) {
            peer_id_by_index.remove(&peer.peer_index);
            return Some(peer);
        }
        None
    }

    #[inline]
    pub fn drop_peer(&self, peer_id: &PeerId) -> Option<Peer> {
        let mut peers = self.peers.write();
        let mut peer_id_by_index = self.peer_id_by_index.write();
        self._drop_peer(peer_id, &mut peers, &mut peer_id_by_index)
    }

    pub fn peers_guard(&self) -> &RwLock<FnvHashMap<PeerId, Peer>> {
        &self.peers
    }

    pub fn peer_indexes_guard(&self) -> &RwLock<FnvHashMap<PeerIndex, PeerId>> {
        &self.peer_id_by_index
    }

    fn _drop_all(
        &self,
        peers: &mut FnvHashMap<PeerId, Peer>,
        peer_id_by_index: &mut FnvHashMap<PeerIndex, PeerId>,
    ) {
        peers.clear();
        peer_id_by_index.clear();
        self.id_allocator.store(0, Ordering::Relaxed)
    }

    pub fn drop_all(&self) {
        debug!(target: "network", "drop_all");
        let mut peers = self.peers.write();
        let mut peer_id_by_index = self.peer_id_by_index.write();
        self._drop_all(&mut peers, &mut peer_id_by_index);
    }
}
