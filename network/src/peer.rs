use crate::network_group::{Group, NetworkGroup};
use crate::{multiaddr::Multiaddr, ProtocolId, ProtocolVersion, SessionType};
use p2p::{secio::PeerId, SessionId};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PeerIdentifyInfo {
    pub client_version: String,
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub connected_addr: Multiaddr,
    pub listened_addrs: Vec<Multiaddr>,
    pub peer_id: PeerId,
    // Client or Server
    pub identify_info: Option<PeerIdentifyInfo>,
    pub last_message_time: Option<Instant>,
    pub ping: Option<Duration>,
    pub is_feeler: bool,
    pub connected_time: Instant,
    pub session_id: SessionId,
    pub session_type: SessionType,
    pub protocols: HashMap<ProtocolId, ProtocolVersion>,
    pub is_whitelist: bool,
}

impl Peer {
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

    pub fn is_outbound(&self) -> bool {
        self.session_type.is_outbound()
    }

    pub fn is_inbound(&self) -> bool {
        self.session_type.is_inbound()
    }

    pub fn network_group(&self) -> Group {
        self.connected_addr.network_group()
    }

    pub fn protocol_version(&self, protocol_id: ProtocolId) -> Option<ProtocolVersion> {
        self.protocols.get(&protocol_id).cloned()
    }
}
