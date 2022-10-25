//! Peer store manager
//!
//! This module implements a locally managed node information set, which is used for
//! booting into the network when the node is started, real-time update detection/timing
//! saving at runtime, and saving data when stopping
//!
//! The validity and screening speed of the data set are very important to the entire network,
//! and the address information collected on the network cannot be blindly trusted

pub mod addr_manager;
pub mod ban_list;
mod peer_store_db;
mod peer_store_impl;
pub mod types;

pub(crate) use crate::Behaviour;
pub use crate::SessionType;
use p2p::multiaddr::Multiaddr;
pub(crate) use peer_store_impl::required_flags_filter;
pub use peer_store_impl::PeerStore;

/// peer store evict peers after reach this limitation
pub(crate) const ADDR_COUNT_LIMIT: usize = 16384;
/// Consider we never seen a peer if peer's last_connected_at beyond this timeout
const ADDR_TIMEOUT_MS: u64 = 7 * 24 * 3600 * 1000;
/// The timeout that peer's address should be added to the feeler list again
pub(crate) const ADDR_TRY_TIMEOUT_MS: u64 = 3 * 24 * 3600 * 1000;
/// When obtaining the list of selectable nodes for identify,
/// the node that has just been disconnected needs to be excluded
pub(crate) const DIAL_INTERVAL: u64 = 15 * 1000;
const ADDR_MAX_RETRIES: u32 = 3;
const ADDR_MAX_FAILURES: u32 = 10;

/// Alias score
pub type Score = i32;

/// PeerStore Scoring configuration
#[derive(Copy, Clone, Debug)]
pub struct PeerScoreConfig {
    /// Default score
    pub default_score: Score,
    /// Ban score
    pub ban_score: Score,
    /// Ban time
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
    /// Connected
    Connected,
    /// The peer is disconnected
    Disconnected,
}

/// Report result
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ReportResult {
    /// Ok
    Ok,
    /// The peer is banned
    Banned,
}

impl ReportResult {
    /// Whether ban
    pub fn is_banned(self) -> bool {
        self == ReportResult::Banned
    }

    /// Whether ok
    pub fn is_ok(self) -> bool {
        self == ReportResult::Ok
    }
}
