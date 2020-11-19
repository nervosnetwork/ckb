//! peer store manager
pub mod addr_manager;
pub mod ban_list;
mod peer_id_serde;
mod peer_store_db;
mod peer_store_impl;
pub mod types;

pub use crate::SessionType;
pub(crate) use crate::{Behaviour, PeerId};
use p2p::multiaddr::Multiaddr;
pub use peer_store_impl::PeerStore;

/// peer store evict peers after reach this limitation
pub(crate) const ADDR_COUNT_LIMIT: usize = 16384;
/// Consider we never seen a peer if peer's last_connected_at beyond this timeout
const ADDR_TIMEOUT_MS: u64 = 7 * 24 * 3600 * 1000;
const ADDR_MAX_RETRIES: u32 = 3;
const ADDR_MAX_FAILURES: u32 = 10;

/// alias score
pub type Score = i32;

/// PeerStore Scoring configuration
#[derive(Copy, Clone, Debug)]
pub struct PeerScoreConfig {
    /// default score
    pub default_score: Score,
    /// ban score
    pub ban_score: Score,
    /// ban time
    pub ban_timeout_ms: u64,
}

impl Default for PeerScoreConfig {
    fn default() -> Self {
        PeerScoreConfig {
            default_score: 100,
            ban_score: 40,
            ban_timeout_ms: 24 * 3600 * 1000, // 1 day
        }
    }
}

/// Peer Status
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
    /// connected
    Connected,
    /// disconnected
    Disconnected,
}

/// report result
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ReportResult {
    /// ok
    Ok,
    /// ban
    Banned,
}

impl ReportResult {
    /// whether ban
    pub fn is_banned(self) -> bool {
        self == ReportResult::Banned
    }

    /// whether ok
    pub fn is_ok(self) -> bool {
        self == ReportResult::Ok
    }
}
