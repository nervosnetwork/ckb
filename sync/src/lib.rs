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

pub const MAX_HEADERS_LEN: usize = 2_000;
pub const MAX_INVENTORY_LEN: usize = 50_000;
pub const MAX_SCHEDULED_LEN: usize = 4 * 1024;
pub const MAX_BLOCKS_TO_ANNOUNCE: usize = 8;
pub const MAX_UNCONNECTING_HEADERS: usize = 10;
pub const MAX_BLOCKS_IN_TRANSIT_PER_PEER: usize = 16;
pub const MAX_TIP_AGE: u64 = 24 * 60 * 60 * 1000;
pub const STALE_RELAY_AGE_LIMIT: u64 = 30 * 24 * 60 * 60 * 1000;

pub(crate) const LOG_TARGET_RELAY: &str = "ckb-relay";

use ckb_network::ProtocolId;

pub enum NetworkProtocol {
    SYNC = 100,
    RELAY = 101,
    TIME = 102,
    ALERT = 110,
}

impl Into<ProtocolId> for NetworkProtocol {
    fn into(self) -> ProtocolId {
        (self as usize).into()
    }
}

//  Timeout = base + per_header * (expected number of headers)
pub const HEADERS_DOWNLOAD_TIMEOUT_BASE: u64 = 6 * 60 * 1000; // 6 minutes
pub const HEADERS_DOWNLOAD_TIMEOUT_PER_HEADER: u64 = 1; // 1ms/header
pub const POW_SPACE: u64 = 10_000; // 10s
pub const MAX_PEERS_PER_BLOCK: usize = 2;

// Protect at least this many outbound peers from disconnection due to slow
// behind headers chain.
pub const MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT: usize = 4;
pub const CHAIN_SYNC_TIMEOUT: u64 = 12 * 60 * 1000; // 12 minutes
pub const SUSPEND_SYNC_TIME: u64 = 5 * 60 * 1000; // 5 minutes
pub const EVICTION_HEADERS_RESPONSE_TIME: u64 = 120 * 1000; // 2 minutes

//The maximum number of entries in a locator
pub const MAX_LOCATOR_SIZE: usize = 101;

pub const BLOCK_DOWNLOAD_TIMEOUT: u64 = 30 * 1000; // 30s

pub const RETRY_ASK_TX_TIMEOUT_INCREASE: Duration = Duration::from_secs(30);

// ban time
// 5 minutes
pub const BAD_MESSAGE_BAN_TIME: Duration = Duration::from_secs(5 * 60);
// 10 minutes, peer have no common ancestor block
pub const SYNC_USELESS_BAN_TIME: Duration = Duration::from_secs(10 * 60);
