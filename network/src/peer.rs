use crate::network_group::{Group, NetworkGroup};
use crate::{multiaddr::Multiaddr, ProtocolId, ProtocolVersion, SessionType};
use p2p::{secio::PeerId, SessionId};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// TODO(doc): @driftluo
#[derive(Clone, Debug)]
pub struct PeerIdentifyInfo {
    /// TODO(doc): @driftluo
    pub client_version: String,
}

/// TODO(doc): @driftluo
#[derive(Clone, Debug)]
pub struct Peer {
    /// TODO(doc): @driftluo
    pub connected_addr: Multiaddr,
    /// TODO(doc): @driftluo
    pub listened_addrs: Vec<Multiaddr>,
    /// TODO(doc): @driftluo
    pub peer_id: PeerId,
    /// TODO(doc): @driftluo
    // Client or Server
    pub identify_info: Option<PeerIdentifyInfo>,
    /// TODO(doc): @driftluo
    pub last_message_time: Option<Instant>,
    /// TODO(doc): @driftluo
    pub ping: Option<Duration>,
    /// TODO(doc): @driftluo
    pub is_feeler: bool,
    /// TODO(doc): @driftluo
    pub connected_time: Instant,
    /// TODO(doc): @driftluo
    pub session_id: SessionId,
    /// TODO(doc): @driftluo
    pub session_type: SessionType,
    /// TODO(doc): @driftluo
    pub protocols: HashMap<ProtocolId, ProtocolVersion>,
    /// TODO(doc): @driftluo
    pub is_whitelist: bool,
}

impl Peer {
    /// TODO(doc): @driftluo
    pub fn new(
        session_id: SessionId,
        session_type: SessionType,
        peer_id: PeerId,
        connected_addr: Multiaddr,
        is_whitelist: bool,
    ) -> Self {
        Peer {
            connected_addr,
            listened_addrs: Vec::new(),
            identify_info: None,
            ping: None,
            last_message_time: None,
            connected_time: Instant::now(),
            is_feeler: false,
            peer_id,
            session_id,
            session_type,
            protocols: HashMap::with_capacity_and_hasher(1, Default::default()),
            is_whitelist,
        }
    }

    /// TODO(doc): @driftluo
    pub fn is_outbound(&self) -> bool {
        self.session_type.is_outbound()
    }

    /// TODO(doc): @driftluo
    pub fn is_inbound(&self) -> bool {
        self.session_type.is_inbound()
    }

    /// TODO(doc): @driftluo
    pub fn network_group(&self) -> Group {
        self.connected_addr.network_group()
    }

    /// TODO(doc): @driftluo
    pub fn protocol_version(&self, protocol_id: ProtocolId) -> Option<ProtocolVersion> {
        self.protocols.get(&protocol_id).cloned()
    }
}
