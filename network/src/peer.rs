use crate::network_group::{Group, NetworkGroup};
use crate::{multiaddr::Multiaddr, ProtocolId, ProtocolVersion, SessionType};
use fnv::FnvHashMap;
use p2p::{secio::PeerId, SessionId};
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
    pub address: Multiaddr,
    pub peer_id: PeerId,
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
    pub is_reserved: bool,
}

impl Peer {
    pub fn new(
        session_id: SessionId,
        session_type: SessionType,
        peer_id: PeerId,
        address: Multiaddr,
        is_reserved: bool,
    ) -> Self {
        Peer {
            address,
            identify_info: None,
            ping: None,
            last_ping_time: None,
            last_message_time: None,
            connected_time: Instant::now(),
            is_feeler: false,
            peer_id,
            session_id,
            session_type,
            protocols: FnvHashMap::with_capacity_and_hasher(1, Default::default()),
            is_reserved,
        }
    }

    pub fn is_outbound(&self) -> bool {
        self.session_type.is_outbound()
    }

    pub fn is_inbound(&self) -> bool {
        self.session_type.is_inbound()
    }

    pub fn network_group(&self) -> Group {
        self.address.network_group()
    }

    pub fn protocol_version(&self, protocol_id: ProtocolId) -> Option<ProtocolVersion> {
        self.protocols.get(&protocol_id).cloned()
    }
}
