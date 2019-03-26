use crate::network_group::{Group, NetworkGroup};
use crate::{multiaddr::Multiaddr, PeerIndex, ProtocolId, ProtocolVersion, SessionId, SessionType};
use fnv::FnvHashMap;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PeerIdentifyInfo {
    pub client_version: String,
    pub protocol_version: String,
    pub supported_protocols: Vec<String>,
    pub count_of_known_listen_addrs: usize,
}

#[derive(Clone, Debug)]
pub struct Peer {
    pub(crate) peer_index: PeerIndex,
    pub connected_addr: Multiaddr,
    // Client or Server
    pub identify_info: Option<PeerIdentifyInfo>,
    pub last_ping_time: Option<Instant>,
    pub last_message_time: Option<Instant>,
    pub ping: Option<Duration>,
    pub is_feeler: bool,
    pub connected_time: Instant,
    pub session_id: SessionId,
    pub session_type: SessionType,
    pub protocols: FnvHashMap<ProtocolId, ProtocolVersion>,
}

impl Peer {
    pub fn new(
        peer_index: PeerIndex,
        connected_addr: Multiaddr,
        session_id: SessionId,
        session_type: SessionType,
    ) -> Self {
        Peer {
            connected_addr,
            identify_info: None,
            ping: None,
            last_ping_time: None,
            last_message_time: None,
            connected_time: Instant::now(),
            is_feeler: false,
            peer_index,
            session_id,
            session_type,
            protocols: FnvHashMap::with_capacity_and_hasher(1, Default::default()),
        }
    }
    #[inline]
    pub fn is_outbound(&self) -> bool {
        self.session_type == SessionType::Client
    }

    #[allow(dead_code)]
    #[inline]
    pub fn is_inbound(&self) -> bool {
        !self.is_outbound()
    }

    #[inline]
    pub fn network_group(&self) -> Group {
        self.connected_addr.network_group()
    }

    pub fn protocol_version(&self, protocol_id: ProtocolId) -> Option<ProtocolVersion> {
        self.protocols.get(&protocol_id).cloned()
    }
}
