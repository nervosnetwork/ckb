use crate::network_group::Group;
use crate::{multiaddr::Multiaddr, ProtocolId, ProtocolVersion, SessionType};
use p2p::SessionId;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Peer info from identify protocol message
#[derive(Clone, Debug)]
pub struct PeerIdentifyInfo {
    /// Node version
    pub client_version: String,
}

/// Peer info
#[derive(Clone, Debug)]
pub struct Peer {
    /// Peer address
    pub connected_addr: Multiaddr,
    /// Peer listen addresses
    pub listened_addrs: Vec<Multiaddr>,
    /// Peer info from identify protocol message
    pub identify_info: Option<PeerIdentifyInfo>,
    /// Ping/Pong message last received time
    pub last_ping_protocol_message_received_at: Option<Instant>,
    /// ping pong rtt
    pub ping_rtt: Option<Duration>,
    /// Indicates whether it is a probe connection of the fleer protocol
    pub is_feeler: bool,
    /// Peer connected time
    pub connected_time: Instant,
    /// Session id
    pub session_id: SessionId,
    /// Session type, Inbound or Outbound
    pub session_type: SessionType,
    /// Opened protocols on this session
    pub protocols: HashMap<ProtocolId, ProtocolVersion>,
    /// Whether a whitelist
    pub is_whitelist: bool,
    /// Remote peer is light client
    pub is_lightclient: bool,
}

impl Peer {
    /// Init session info
    pub fn new(
        session_id: SessionId,
        session_type: SessionType,
        connected_addr: Multiaddr,
        is_whitelist: bool,
    ) -> Self {
        Peer {
            connected_addr,
            listened_addrs: Vec::new(),
            identify_info: None,
            ping_rtt: None,
            last_ping_protocol_message_received_at: None,
            connected_time: Instant::now(),
            is_feeler: false,
            session_id,
            session_type,
            protocols: HashMap::with_capacity_and_hasher(1, Default::default()),
            is_whitelist,
            is_lightclient: false,
        }
    }

    /// Whether outbound session
    pub fn is_outbound(&self) -> bool {
        self.session_type.is_outbound()
    }

    /// Whether inbound session
    pub fn is_inbound(&self) -> bool {
        self.session_type.is_inbound()
    }

    /// Get net group
    pub fn network_group(&self) -> Group {
        (&self.connected_addr).into()
    }

    /// Opened protocol version
    pub fn protocol_version(&self, protocol_id: ProtocolId) -> Option<ProtocolVersion> {
        self.protocols.get(&protocol_id).cloned()
    }
}
