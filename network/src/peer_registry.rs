//! TODO(doc): @driftluo
use crate::peer_store::PeerStore;
use crate::{
    errors::{Error, PeerError},
    Peer, PeerId, SessionType,
};
use ckb_logger::debug;
use p2p::{multiaddr::Multiaddr, SessionId};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;

pub(crate) const EVICTION_PROTECT_PEERS: usize = 8;

/// TODO(doc): @driftluo
pub struct PeerRegistry {
    peers: HashMap<SessionId, Peer>,
    // max inbound limitation
    max_inbound: u32,
    // max outbound limitation
    max_outbound: u32,
    // Only whitelist peers or allow all peers.
    whitelist_only: bool,
    whitelist_peers: HashSet<PeerId>,
    feeler_peers: HashSet<PeerId>,
}

/// TODO(doc): @driftluo
#[derive(Clone, Copy, Debug)]
pub struct ConnectionStatus {
    /// TODO(doc): @driftluo
    pub total: u32,
    /// TODO(doc): @driftluo
    pub non_whitelist_inbound: u32,
    /// TODO(doc): @driftluo
    pub non_whitelist_outbound: u32,
    /// TODO(doc): @driftluo
    pub max_inbound: u32,
    /// TODO(doc): @driftluo
    pub max_outbound: u32,
}

fn sort_then_drop<T, F>(list: &mut Vec<T>, n: usize, compare: F)
where
    F: FnMut(&T, &T) -> std::cmp::Ordering,
{
    list.sort_by(compare);
    if list.len() > n {
        list.truncate(list.len() - n);
    }
}

impl PeerRegistry {
    /// TODO(doc): @driftluo
    pub fn new(
        max_inbound: u32,
        max_outbound: u32,
        whitelist_only: bool,
        whitelist_peers: Vec<PeerId>,
    ) -> Self {
        let whitelist_peers_set = HashSet::from_iter(whitelist_peers);
        PeerRegistry {
            peers: HashMap::with_capacity_and_hasher(20, Default::default()),
            whitelist_peers: whitelist_peers_set,
            feeler_peers: HashSet::default(),
            max_inbound,
            max_outbound,
            whitelist_only,
        }
    }

    pub(crate) fn accept_peer(
        &mut self,
        peer_id: PeerId,
        remote_addr: Multiaddr,
        session_id: SessionId,
        session_type: SessionType,
        peer_store: &mut PeerStore,
    ) -> Result<Option<Peer>, Error> {
        if self.peers.contains_key(&session_id) {
            return Err(PeerError::SessionExists(session_id).into());
        }
        if self.get_key_by_peer_id(&peer_id).is_some() {
            return Err(PeerError::PeerIdExists(peer_id).into());
        }

        let is_whitelist = self.whitelist_peers.contains(&peer_id);
        let mut evicted_peer: Option<Peer> = None;

        if !is_whitelist {
            if self.whitelist_only {
                return Err(PeerError::NonReserved.into());
            }
            if peer_store.is_addr_banned(&remote_addr) {
                return Err(PeerError::Banned.into());
            }

            let connection_status = self.connection_status();
            // check peers connection limitation
            if session_type.is_inbound() {
                if connection_status.non_whitelist_inbound >= self.max_inbound {
                    if let Some(evicted_session) = self.try_evict_inbound_peer(peer_store) {
                        evicted_peer = self.remove_peer(evicted_session);
                    } else {
                        return Err(PeerError::ReachMaxInboundLimit.into());
                    }
                }
            } else if connection_status.non_whitelist_outbound >= self.max_outbound {
                return Err(PeerError::ReachMaxOutboundLimit.into());
            }
        }
        peer_store.add_connected_peer(peer_id.clone(), remote_addr.clone(), session_type)?;
        let peer = Peer::new(session_id, session_type, peer_id, remote_addr, is_whitelist);
        self.peers.insert(session_id, peer);
        Ok(evicted_peer)
    }

    // try to evict an inbound peer
    fn try_evict_inbound_peer(&self, _peer_store: &PeerStore) -> Option<SessionId> {
        let mut candidate_peers = {
            self.peers
                .values()
                .filter(|peer| peer.is_inbound() && !peer.is_whitelist)
                .collect::<Vec<_>>()
        };
        // Protect peers based on characteristics that an attacker hard to simulate or manipulate
        // Protect peers which has the lowest ping
        sort_then_drop(
            &mut candidate_peers,
            EVICTION_PROTECT_PEERS,
            |peer1, peer2| {
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
        sort_then_drop(
            &mut candidate_peers,
            EVICTION_PROTECT_PEERS,
            |peer1, peer2| {
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
        let protect_peers = candidate_peers.len() >> 1;
        sort_then_drop(&mut candidate_peers, protect_peers, |peer1, peer2| {
            peer2.connected_time.cmp(&peer1.connected_time)
        });

        // Group peers by network group
        let evict_group = candidate_peers
            .into_iter()
            .fold(HashMap::new(), |mut groups, peer| {
                groups
                    .entry(peer.network_group())
                    .or_insert_with(Vec::new)
                    .push(peer);
                groups
            })
            .values()
            .max_by_key(|group| group.len())
            .cloned()
            .unwrap_or_else(Vec::new);

        // randomly evict a peer
        let mut rng = thread_rng();
        evict_group.choose(&mut rng).map(|peer| {
            debug!("evict inbound peer {:?}", peer.peer_id);
            peer.session_id
        })
    }

    /// TODO(doc): @driftluo
    pub fn add_feeler(&mut self, peer_id: PeerId) {
        self.feeler_peers.insert(peer_id);
    }

    /// TODO(doc): @driftluo
    pub fn remove_feeler(&mut self, peer_id: &PeerId) {
        self.feeler_peers.remove(peer_id);
    }

    /// TODO(doc): @driftluo
    pub fn is_feeler(&self, peer_id: &PeerId) -> bool {
        self.feeler_peers.contains(peer_id)
    }

    /// TODO(doc): @driftluo
    pub fn get_peer(&self, session_id: SessionId) -> Option<&Peer> {
        self.peers.get(&session_id)
    }

    /// TODO(doc): @driftluo
    pub fn get_peer_mut(&mut self, session_id: SessionId) -> Option<&mut Peer> {
        self.peers.get_mut(&session_id)
    }

    pub(crate) fn remove_peer(&mut self, session_id: SessionId) -> Option<Peer> {
        self.peers.remove(&session_id)
    }

    /// TODO(doc): @driftluo
    pub fn get_key_by_peer_id(&self, peer_id: &PeerId) -> Option<SessionId> {
        self.peers.values().find_map(|peer| {
            if &peer.peer_id == peer_id {
                Some(peer.session_id)
            } else {
                None
            }
        })
    }

    pub(crate) fn remove_peer_by_peer_id(&mut self, peer_id: &PeerId) -> Option<Peer> {
        self.get_key_by_peer_id(peer_id)
            .and_then(|session_id| self.peers.remove(&session_id))
    }

    /// TODO(doc): @driftluo
    pub fn peers(&self) -> &HashMap<SessionId, Peer> {
        &self.peers
    }

    /// TODO(doc): @driftluo
    pub fn connected_peers(&self) -> Vec<SessionId> {
        self.peers.keys().cloned().collect()
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        let total = self.peers.len() as u32;
        let mut non_whitelist_inbound: u32 = 0;
        let mut non_whitelist_outbound: u32 = 0;
        for peer in self.peers.values().filter(|peer| !peer.is_whitelist) {
            if peer.is_outbound() {
                non_whitelist_outbound += 1;
            } else {
                non_whitelist_inbound += 1;
            }
        }
        ConnectionStatus {
            total,
            non_whitelist_inbound,
            non_whitelist_outbound,
            max_inbound: self.max_inbound,
            max_outbound: self.max_outbound,
        }
    }
}
