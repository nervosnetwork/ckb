pub mod sqlite;
pub mod types;

use self::types::PeerAddr;
pub use crate::{peer_store::sqlite::SqlitePeerStore, SessionType};
pub(crate) use crate::{Behaviour, PeerId};
use p2p::multiaddr::{Multiaddr, Protocol};
use std::time::Duration;

/// After this limitation, peer store will try to eviction peers
pub const PEER_STORE_LIMIT: u32 = 8192;
/// Clear banned list if the list reach this size
pub const BAN_LIST_CLEAR_EXPIRES_SIZE: usize = 1024;
pub const DEFAULT_ADDRS: u32 = 3;
pub const MAX_ADDRS: u32 = 3;
/// Consider we never seen a peer if peer's last_connected_at beyond this timeout
pub const ADDR_TIMEOUT_MS: u64 = 7 * 24 * 3600 * 1000;
pub const ADDR_MAX_RETRIES: u32 = 3;
pub const ADDR_MAX_FAILURES: u32 = 10;

pub type Score = i32;

/// PeerStore Scoring configuration
#[derive(Copy, Clone, Debug)]
pub struct PeerScoreConfig {
    pub default_score: Score,
    pub ban_score: Score,
    pub ban_timeout: Duration,
}

impl Default for PeerScoreConfig {
    fn default() -> Self {
        PeerScoreConfig {
            default_score: 100,
            ban_score: 40,
            ban_timeout: Duration::from_secs(24 * 3600),
        }
    }
}

/// PeerStore
/// See [rfc0007](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0007-scoring-system-and-network-security/0007-scoring-system-and-network-security.md) for details.
pub trait PeerStore: Send {
    /// Add a peer and address into peer_store
    /// this method will assume peer is connected, which implies address is "verified".
    fn add_connected_peer(&mut self, peer_id: &PeerId, address: Multiaddr, endpoint: SessionType);
    /// Add discovered peer addresses
    /// this method will assume peer and addr is untrust since we have not connected to it.
    fn add_discovered_addr(&mut self, peer_id: &PeerId, address: Multiaddr);
    // Update PeerAddr
    fn update_peer_addr(&mut self, peer_addr: &PeerAddr);
    /// Report peer behaviours
    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult;
    /// Update peer status
    fn update_status(&self, peer_id: &PeerId, status: Status);
    fn peer_status(&self, peer_id: &PeerId) -> Status;
    /// Add bootnode
    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr);
    /// This method randomly return peers, it return bootnodes if no other peers in PeerStore.
    fn bootnodes(&self, count: u32) -> Vec<(PeerId, Multiaddr)>;
    /// Get addrs of a peer, note a peer may have multiple addrs
    fn peer_addrs(&self, peer_id: &PeerId, count: u32) -> Vec<PeerAddr>;
    /// Get peers for outbound connection, this method randomly return non-connected peer addrs
    fn peers_to_attempt(&self, count: u32) -> Vec<PeerAddr>;
    /// Get peers for feeler connection, this method randomly return peer addrs that we never
    /// connected to.
    fn peers_to_feeler(&self, count: u32) -> Vec<PeerAddr>;
    /// Randomly get peers
    fn random_peers(&self, count: u32) -> Vec<PeerAddr>;
    /// Ban a addr
    fn ban_addr(&mut self, addr: &Multiaddr, timeout: Duration);
    /// Check peer ban status
    fn is_banned(&self, addr: &Multiaddr) -> bool;
    /// peer score config
    fn peer_score_config(&self) -> PeerScoreConfig;
}

/// Peer Status
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    Connected = 0,
    Disconnected = 1,
    Unknown = 2,
}

impl From<u8> for Status {
    fn from(i: u8) -> Self {
        match i {
            0 => Status::Connected,
            1 => Status::Disconnected,
            2 => Status::Unknown,
            _ => Status::Unknown,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ReportResult {
    Ok,
    Banned,
}

#[allow(dead_code)]
impl ReportResult {
    pub fn is_banned(self) -> bool {
        self == ReportResult::Banned
    }

    pub fn is_ok(self) -> bool {
        self == ReportResult::Ok
    }
}
