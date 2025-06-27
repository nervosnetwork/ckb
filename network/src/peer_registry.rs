//! Peer registry
use crate::Flags;
use crate::network_group::Group;
use crate::peer_store::PeerStore;
use crate::{
    Peer, PeerId, SessionType,
    errors::{Error, PeerError},
    extract_peer_id,
};
use ckb_logger::debug;
use ckb_systemtime::Instant;
use p2p::{SessionId, multiaddr::Multiaddr, service::SessionType as RawSessionType};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::{HashMap, HashSet};

pub(crate) const EVICTION_PROTECT_PEERS: usize = 8;

pub(crate) const MAX_OUTBOUND_BLOCK_RELAY: u32 = 2;

/// Memory records of opened session information
pub struct PeerRegistry {
    peers: HashMap<SessionId, Peer>,
    // max inbound limitation
    max_inbound: u32,
    // max outbound limitation
    max_outbound: u32,
    // max block-relay only outbound limitation
    // We do not relay tx or addr messages with these peers
    max_outbound_block_relay: u32,
    // Only whitelist peers or allow all peers.
    whitelist_only: bool,
    whitelist_peers: HashSet<PeerId>,
    feeler_peers: HashMap<PeerId, Flags>,
    disable_block_relay_only_connection: bool,
}

/// Global network connection status
#[derive(Clone, Copy, Debug)]
pub struct ConnectionStatus {
    /// Total session number
    pub total: u32,
    /// Not whitelist inbound number
    pub non_whitelist_inbound: u32,
    /// Not whitelist outbound number
    pub non_whitelist_outbound: u32,
    ///BLOCK relay only outbound number
    pub block_relay_only_outbound_count: u32,
    /// Maximum number of inbound session
    pub max_inbound: u32,
    /// Maximum number of outbound session
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
    /// Init registry from config
    pub fn new(
        max_inbound: u32,
        max_outbound: u32,
        whitelist_only: bool,
        whitelist_peers: Vec<Multiaddr>,
        disable_block_relay_only_connection: bool,
    ) -> Self {
        PeerRegistry {
            peers: HashMap::with_capacity_and_hasher(20, Default::default()),
            whitelist_peers: whitelist_peers.iter().filter_map(extract_peer_id).collect(),
            feeler_peers: HashMap::default(),
            max_inbound,
            max_outbound,
            max_outbound_block_relay: MAX_OUTBOUND_BLOCK_RELAY,
            whitelist_only,
            disable_block_relay_only_connection,
        }
    }

    pub(crate) fn accept_peer(
        &mut self,
        remote_addr: Multiaddr,
        session_id: SessionId,
        raw_session_type: RawSessionType,
        peer_store: &mut PeerStore,
    ) -> Result<Option<Peer>, Error> {
        if self.peers.contains_key(&session_id) {
            return Err(PeerError::SessionExists(session_id).into());
        }
        let peer_id = extract_peer_id(&remote_addr).expect("opened session should have peer id");
        if self.get_key_by_peer_id(&peer_id).is_some() {
            return Err(PeerError::PeerIdExists(peer_id).into());
        }

        let is_whitelist = self.whitelist_peers.contains(&peer_id);
        let mut evicted_peer: Option<Peer> = None;

        let mut session_type: SessionType = raw_session_type.into();
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
                if self.disable_block_relay_only_connection
                    || connection_status.block_relay_only_outbound_count
                        >= self.max_outbound_block_relay
                {
                    return Err(PeerError::ReachMaxOutboundLimit.into());
                } else {
                    peer_store.add_anchors(remote_addr.clone());
                    session_type = SessionType::BlockRelayOnly;
                }
            }
        }
        peer_store.add_connected_peer(remote_addr.clone(), session_type);
        let peer = Peer::new(session_id, session_type, remote_addr, is_whitelist);
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
                    .ping_rtt
                    .map(|p| p.as_secs())
                    .unwrap_or_else(|| u64::MAX);
                let peer2_ping = peer2
                    .ping_rtt
                    .map(|p| p.as_secs())
                    .unwrap_or_else(|| u64::MAX);
                peer2_ping.cmp(&peer1_ping)
            },
        );

        // Protect peers which most recently sent messages
        sort_then_drop(
            &mut candidate_peers,
            EVICTION_PROTECT_PEERS,
            |peer1, peer2| {
                let now = Instant::now();
                let peer1_last_message = peer1
                    .last_ping_protocol_message_received_at
                    .map(|t| now.saturating_duration_since(t).as_secs())
                    .unwrap_or_else(|| u64::MAX);
                let peer2_last_message = peer2
                    .last_ping_protocol_message_received_at
                    .map(|t| now.saturating_duration_since(t).as_secs())
                    .unwrap_or_else(|| u64::MAX);
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
            .fold(
                HashMap::new(),
                |mut groups: HashMap<Group, Vec<&Peer>>, peer| {
                    groups.entry(peer.network_group()).or_default().push(peer);
                    groups
                },
            )
            .values()
            .max_by_key(|group| group.len())
            .cloned()
            .unwrap_or_default();

        // randomly evict a peer
        let mut rng = thread_rng();
        evict_group.choose(&mut rng).map(|peer| {
            debug!("Disconnect inbound peer {:?}", peer.connected_addr);
            peer.session_id
        })
    }

    /// Add feeler dail task
    pub fn add_feeler(&mut self, addr: &Multiaddr) {
        if let Some(peer_id) = extract_peer_id(addr) {
            self.feeler_peers.insert(peer_id, Flags::COMPATIBILITY);
        }
    }

    /// Identify change feeler flags
    pub fn change_feeler_flags(&mut self, addr: &Multiaddr, flags: Flags) -> bool {
        if let Some(peer_id) = extract_peer_id(addr) {
            if let Some(i) = self.feeler_peers.get_mut(&peer_id) {
                *i = flags;
                return true;
            }
        }
        false
    }

    /// Get feeler session flags
    pub fn feeler_flags(&self, addr: &Multiaddr) -> Option<Flags> {
        extract_peer_id(addr).and_then(|peer_id| self.feeler_peers.get(&peer_id).cloned())
    }

    /// Remove feeler dail task on session disconnects or fails
    pub fn remove_feeler(&mut self, addr: &Multiaddr) {
        if let Some(peer_id) = extract_peer_id(addr) {
            self.feeler_peers.remove(&peer_id);
        }
    }

    /// Whether this session is feeler session
    pub fn is_feeler(&self, addr: &Multiaddr) -> bool {
        extract_peer_id(addr)
            .map(|peer_id| self.feeler_peers.contains_key(&peer_id))
            .unwrap_or_default()
    }

    /// Whether this session is anchor
    pub fn is_anchor(&self, session_id: SessionId) -> bool {
        self.get_peer(session_id)
            .map(|peer| peer.is_block_relay_only())
            .unwrap_or(false)
    }

    /// Get peer info
    pub fn get_peer(&self, session_id: SessionId) -> Option<&Peer> {
        self.peers.get(&session_id)
    }

    /// Get mut peer info
    pub fn get_peer_mut(&mut self, session_id: SessionId) -> Option<&mut Peer> {
        self.peers.get_mut(&session_id)
    }

    pub(crate) fn remove_peer(&mut self, session_id: SessionId) -> Option<Peer> {
        self.peers.remove(&session_id)
    }

    /// Get session id by peer id
    pub fn get_key_by_peer_id(&self, peer_id: &PeerId) -> Option<SessionId> {
        self.peers.iter().find_map(|(session_id, peer)| {
            extract_peer_id(&peer.connected_addr).and_then(|pid| {
                if &pid == peer_id {
                    Some(*session_id)
                } else {
                    None
                }
            })
        })
    }

    /// Get all connected peers' information
    pub fn peers(&self) -> &HashMap<SessionId, Peer> {
        &self.peers
    }

    /// Get all sessions' id
    pub fn connected_peers(&self) -> Vec<SessionId> {
        self.peers.keys().cloned().collect()
    }

    pub(crate) fn connection_status(&self) -> ConnectionStatus {
        let total = self.peers.len() as u32;
        let mut non_whitelist_inbound: u32 = 0;
        let mut non_whitelist_outbound: u32 = 0;
        let mut block_relay_only_outbound_count: u32 = 0;
        for peer in self.peers.values().filter(|peer| !peer.is_whitelist) {
            if peer.is_outbound() {
                non_whitelist_outbound += 1;
            } else if peer.is_block_relay_only() {
                block_relay_only_outbound_count += 1;
            } else {
                non_whitelist_inbound += 1;
            }
        }
        ConnectionStatus {
            total,
            non_whitelist_inbound,
            non_whitelist_outbound,
            block_relay_only_outbound_count,
            max_inbound: self.max_inbound,
            max_outbound: self.max_outbound,
        }
    }
}
