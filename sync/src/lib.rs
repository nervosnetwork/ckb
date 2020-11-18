//! # The Sync module
//!
//! Sync module implement ckb sync protocol as specified here:
//! https://github.com/nervosnetwork/rfcs/tree/master/rfcs/0000-block-sync-protocol

mod block_status;
mod net_time_checker;
mod orphan_block_pool;
mod relayer;
mod status;
mod synchronizer;
mod types;

#[cfg(test)]
mod tests;

pub use crate::net_time_checker::NetTimeProtocol;
pub use crate::relayer::Relayer;
pub use crate::status::{Status, StatusCode};
pub use crate::synchronizer::Synchronizer;
pub use crate::types::SyncShared;
use std::time::Duration;

/// TODO(doc): @driftluo
pub const MAX_HEADERS_LEN: usize = 2_000;
/// TODO(doc): @driftluo
pub const MAX_INVENTORY_LEN: usize = 50_000;
/// TODO(doc): @driftluo
pub const MAX_SCHEDULED_LEN: usize = 4 * 1024;
/// TODO(doc): @driftluo
pub const MAX_BLOCKS_TO_ANNOUNCE: usize = 8;
/// TODO(doc): @driftluo
pub const MAX_UNCONNECTING_HEADERS: usize = 10;
/// TODO(doc): @driftluo
pub const MAX_TIP_AGE: u64 = 24 * 60 * 60 * 1000;
/// TODO(doc): @driftluo
pub const STALE_RELAY_AGE_LIMIT: u64 = 30 * 24 * 60 * 60 * 1000;

/// TODO(doc): @driftluo
/* About Download Scheduler */
pub const INIT_BLOCKS_IN_TRANSIT_PER_PEER: usize = 16;
/// TODO(doc): @driftluo
pub const MAX_BLOCKS_IN_TRANSIT_PER_PEER: usize = 128;
/// TODO(doc): @driftluo
pub const CHECK_POINT_WINDOW: u64 = (MAX_BLOCKS_IN_TRANSIT_PER_PEER * 4) as u64;

// Time recording window size, ibd period scheduler dynamically adjusts frequency
// for acquisition/analysis generating dynamic time range
pub(crate) const TIME_TRACE_SIZE: usize = MAX_BLOCKS_IN_TRANSIT_PER_PEER * 4;
// Fast Zone Boundaries for the Time Window
pub(crate) const FAST_INDEX: usize = TIME_TRACE_SIZE / 3;
// Normal Zone Boundaries for the Time Window
pub(crate) const NORMAL_INDEX: usize = TIME_TRACE_SIZE * 4 / 5;
// Low Zone Boundaries for the Time Window
pub(crate) const LOW_INDEX: usize = TIME_TRACE_SIZE * 9 / 10;

pub(crate) const LOG_TARGET_RELAY: &str = "ckb_relay";

/// TODO(doc): @driftluo
// Inspect the headers downloading every 2 minutes
pub const HEADERS_DOWNLOAD_INSPECT_WINDOW: u64 = 2 * 60 * 1000;
/// TODO(doc): @driftluo
// Global Average Speed
//      Expect 300 KiB/second
//          = 1600 headers/second (300*1024/192)
//          = 96000 headers/minute (1600*60)
//          = 11.11 days-in-blockchain/minute-in-reality (96000*10/60/60/24)
//      => Sync 1 year headers in blockchain will be in 32.85 minutes (365/11.11) in reality
pub const HEADERS_DOWNLOAD_HEADERS_PER_SECOND: u64 = 1600;
/// TODO(doc): @driftluo
// Acceptable Lowest Instantaneous Speed: 75.0 KiB/second (300/4)
pub const HEADERS_DOWNLOAD_TOLERABLE_BIAS_FOR_SINGLE_SAMPLE: u64 = 4;
/// TODO(doc): @driftluo
pub const POW_INTERVAL: u64 = 10;

/// TODO(doc): @driftluo
// Protect at least this many outbound peers from disconnection due to slow
// behind headers chain.
pub const MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT: usize = 4;
/// TODO(doc): @driftluo
pub const CHAIN_SYNC_TIMEOUT: u64 = 12 * 60 * 1000; // 12 minutes
/// TODO(doc): @driftluo
pub const SUSPEND_SYNC_TIME: u64 = 5 * 60 * 1000; // 5 minutes
/// TODO(doc): @driftluo
pub const EVICTION_HEADERS_RESPONSE_TIME: u64 = 120 * 1000; // 2 minutes

/// TODO(doc): @driftluo
//The maximum number of entries in a locator
pub const MAX_LOCATOR_SIZE: usize = 101;

/// TODO(doc): @driftluo
pub const BLOCK_DOWNLOAD_TIMEOUT: u64 = 30 * 1000; // 30s

/// TODO(doc): @driftluo
// Size of the "block download window": how far ahead of our current height do we fetch?
// Larger windows tolerate larger download speed differences between peers, but increase the
// potential degree of disordering of blocks.
pub const BLOCK_DOWNLOAD_WINDOW: u64 = 1024 * 8; // 1024 * default_outbound_peers

/// TODO(doc): @driftluo
pub const RETRY_ASK_TX_TIMEOUT_INCREASE: Duration = Duration::from_secs(30);

/// TODO(doc): @driftluo
// ban time
// 5 minutes
pub const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(5 * 60);
/// TODO(doc): @driftluo
// 10 minutes, peer have no common ancestor block
pub const SYNC_USELESS_BAN_TIME: Duration = Duration::from_secs(10 * 60);
