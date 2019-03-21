mod db;
pub mod sqlite;
pub use crate::{peer_store::sqlite_peer_store::SqlitePeerStore, SessionType};
#[cfg(db_trace)]
pub mod db_trace;
mod score;
pub(crate) mod sqlite_peer_store;

pub(crate) use crate::PeerId;
use p2p::multiaddr::Multiaddr;
pub use score::{Behaviour, Score, ScoringSchema};
use std::time::Duration;

/// PeerStore
/// See [rfc0007](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0007-scoring-system-and-network-security/0007-scoring-system-and-network-security.md) for details.
pub trait PeerStore: Send + Sync {
    /// Add a peer and address into peer_store
    /// this method will assume peer is connected, which implies address is "verified".
    fn add_connected_peer(&mut self, peer_id: &PeerId, address: Multiaddr, endpoint: SessionType);
    /// Add discovered peer addresses
    /// this method will assume peer and addr is untrust since we have not connected to it.
    fn add_discovered_addr(&mut self, peer_id: &PeerId, address: Multiaddr) -> bool;
    /// Report peer behaviours
    fn report(&mut self, peer_id: &PeerId, behaviour: Behaviour) -> ReportResult;
    /// Update peer status
    fn update_status(&mut self, peer_id: &PeerId, status: Status);
    fn peer_status(&self, peer_id: &PeerId) -> Status;
    fn peer_score(&self, peer_id: &PeerId) -> Option<Score>;
    /// Add bootnode
    fn add_bootnode(&mut self, peer_id: PeerId, addr: Multiaddr);
    /// This method randomly return peers, it return bootnodes if no other peers in PeerStore.
    fn bootnodes(&self, count: u32) -> Vec<(PeerId, Multiaddr)>;
    /// Get addrs of a peer, note a peer may have multiple addrs
    fn peer_addrs(&self, peer_id: &PeerId, count: u32) -> Option<Vec<Multiaddr>>;
    /// Get peers for outbound connection, this method randomly return non-connected peer addrs
    fn peers_to_attempt(&self, count: u32) -> Vec<(PeerId, Multiaddr)>;
    /// Randomly get peers
    fn random_peers(&self, count: u32) -> Vec<(PeerId, Multiaddr)>;
    /// Ban a peer
    fn ban_peer(&mut self, peer_id: &PeerId, timeout: Duration);
    fn is_banned(&self, peer_id: &PeerId) -> bool;
    fn scoring_schema(&self) -> &ScoringSchema;
    fn peer_score_or_default(&self, peer_id: &PeerId) -> Score {
        self.peer_score(peer_id)
            .unwrap_or_else(|| self.scoring_schema().peer_init_score())
    }
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
