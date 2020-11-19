//! Type used on peer store
use crate::{
    errors::{AddrError, Error},
    peer_store::{
        peer_id_serde, PeerId, Score, SessionType, ADDR_MAX_FAILURES, ADDR_MAX_RETRIES,
        ADDR_TIMEOUT_MS,
    },
};
use ipnetwork::IpNetwork;
use p2p::multiaddr::{self, Multiaddr, Protocol};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, net::IpAddr};

/// ip and port
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpPort {
    /// ip addr
    pub ip: IpAddr,
    /// port
    pub port: u16,
}

/// peer info
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Peer id
    pub peer_id: PeerId,
    /// address
    pub connected_addr: Multiaddr,
    /// session type
    pub session_type: SessionType,
    /// connected time
    pub last_connected_at_ms: u64,
}

impl PeerInfo {
    /// init
    pub fn new(
        peer_id: PeerId,
        connected_addr: Multiaddr,
        session_type: SessionType,
        last_connected_at_ms: u64,
    ) -> Self {
        PeerInfo {
            peer_id,
            connected_addr,
            session_type,
            last_connected_at_ms,
        }
    }
}

/// Address info
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct AddrInfo {
    /// peer id
    #[serde(with = "peer_id_serde")]
    pub peer_id: PeerId,
    /// ip and port
    pub ip_port: IpPort,
    /// multiaddr
    pub addr: Multiaddr,
    /// score about this addr
    pub score: Score,
    /// last connected time
    pub last_connected_at_ms: u64,
    /// last try time
    pub last_tried_at_ms: u64,
    /// attempts count
    pub attempts_count: u32,
    /// random id
    pub random_id_pos: usize,
}

impl AddrInfo {
    /// Init
    pub fn new(
        peer_id: PeerId,
        ip_port: IpPort,
        addr: Multiaddr,
        last_connected_at_ms: u64,
        score: Score,
    ) -> Self {
        AddrInfo {
            peer_id,
            ip_port,
            addr,
            score,
            last_connected_at_ms,
            last_tried_at_ms: 0,
            attempts_count: 0,
            random_id_pos: 0,
        }
    }

    /// Get ip and port
    pub fn ip_port(&self) -> IpPort {
        self.ip_port
    }

    /// Whether already connected
    pub fn had_connected(&self, expires_ms: u64) -> bool {
        self.last_connected_at_ms > expires_ms
    }

    /// Whether already try dail within a minute
    pub fn tried_in_last_minute(&self, now_ms: u64) -> bool {
        self.last_tried_at_ms >= now_ms.saturating_sub(60_000)
    }

    /// whether terrible peer
    pub fn is_terrible(&self, now_ms: u64) -> bool {
        // do not remove addr tried in last minute
        if self.tried_in_last_minute(now_ms) {
            return false;
        }
        // we give up if never connect to this addr
        if self.last_connected_at_ms == 0 && self.attempts_count >= ADDR_MAX_RETRIES {
            return true;
        }
        // consider addr is terrible if failed too many times
        if now_ms.saturating_sub(self.last_connected_at_ms) > ADDR_TIMEOUT_MS
            && (self.attempts_count >= ADDR_MAX_FAILURES)
        {
            return true;
        }
        false
    }

    /// try dail count
    pub fn mark_tried(&mut self, tried_at_ms: u64) {
        self.last_tried_at_ms = tried_at_ms;
        self.attempts_count = self.attempts_count.saturating_add(1);
    }

    /// mart last connected time
    pub fn mark_connected(&mut self, connected_at_ms: u64) {
        self.last_connected_at_ms = connected_at_ms;
        // reset attempts
        self.attempts_count = 0;
    }

    /// get multiaddr
    pub fn multiaddr(&self) -> Result<Multiaddr, Error> {
        self.addr.attach_p2p(&self.peer_id)
    }
}

/// Banned addr info
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BannedAddr {
    /// ip address
    pub address: IpNetwork,
    /// ban until time
    pub ban_until: u64,
    /// ban reason
    pub ban_reason: String,
    /// ban time
    pub created_at: u64,
}

/// convert multiaddr to IpNetwork
pub fn multiaddr_to_ip_network(multiaddr: &Multiaddr) -> Option<IpNetwork> {
    for addr_component in multiaddr {
        match addr_component {
            Protocol::IP4(ipv4) => return Some(IpNetwork::V4(ipv4.into())),
            Protocol::IP6(ipv6) => return Some(IpNetwork::V6(ipv6.into())),
            _ => (),
        }
    }
    None
}

/// convert IpAddr to IpNetwork
pub fn ip_to_network(ip: IpAddr) -> IpNetwork {
    match ip {
        IpAddr::V4(ipv4) => IpNetwork::V4(ipv4.into()),
        IpAddr::V6(ipv6) => IpNetwork::V6(ipv6.into()),
    }
}

/// Some util patch to multiaddr
pub trait MultiaddrExt {
    /// extract IP from multiaddr,
    fn extract_ip_addr(&self) -> Result<IpPort, Error>;
    /// remove peer id
    fn exclude_p2p(&self) -> Multiaddr;
    /// attach peer id
    fn attach_p2p(&self, peer_id: &PeerId) -> Result<Multiaddr, Error>;
}

impl MultiaddrExt for Multiaddr {
    fn extract_ip_addr(&self) -> Result<IpPort, Error> {
        let mut ip = None;
        let mut port = None;
        for component in self {
            match component {
                Protocol::IP4(ipv4) => ip = Some(IpAddr::V4(ipv4)),
                Protocol::IP6(ipv6) => ip = Some(IpAddr::V6(ipv6)),
                Protocol::TCP(tcp_port) => port = Some(tcp_port),
                _ => (),
            }
        }
        Ok(IpPort {
            ip: ip.ok_or(AddrError::MissingIP)?,
            port: port.ok_or(AddrError::MissingPort)?,
        })
    }

    fn exclude_p2p(&self) -> Multiaddr {
        self.iter()
            .filter_map(|proto| match proto {
                Protocol::P2P(_) => None,
                value => Some(value),
            })
            .collect::<Multiaddr>()
    }

    fn attach_p2p(&self, peer_id: &PeerId) -> Result<Multiaddr, Error> {
        let mut addr = self.exclude_p2p();
        let peer_id_hash = Cow::Owned(peer_id.clone().into_bytes());
        addr.push(multiaddr::Protocol::P2P(peer_id_hash));
        Ok(addr)
    }
}
