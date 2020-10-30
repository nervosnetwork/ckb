//! TODO(doc): @driftluo
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

/// TODO(doc): @driftluo
pub type Score = i32;

/// PeerStore Scoring configuration
#[derive(Copy, Clone, Debug)]
pub struct PeerScoreConfig {
    /// TODO(doc): @driftluo
    pub default_score: Score,
    /// TODO(doc): @driftluo
    pub ban_score: Score,
    /// TODO(doc): @driftluo
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
    /// TODO(doc): @driftluo
    Connected,
    /// TODO(doc): @driftluo
    Disconnected,
}

/// TODO(doc): @driftluo
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ReportResult {
    /// TODO(doc): @driftluo
    Ok,
    /// TODO(doc): @driftluo
    Banned,
}

impl ReportResult {
    /// TODO(doc): @driftluo
    pub fn is_banned(self) -> bool {
        self == ReportResult::Banned
    }

    /// TODO(doc): @driftluo
    pub fn is_ok(self) -> bool {
        self == ReportResult::Ok
    }
}
