//! # The Sync module
//!
//! Sync module implement ckb sync protocol as specified here:
//! https://github.com/nervosnetwork/rfcs/tree/master/rfcs/0000-block-sync-protocol

mod config;
mod relayer;
mod synchronizer;
mod types;

#[cfg(test)]
mod tests;

pub use crate::config::Config;
pub use crate::relayer::Relayer;
pub use crate::synchronizer::Synchronizer;

use ckb_network::ProtocolId;

pub const MAX_HEADERS_LEN: usize = 2_000;
pub const MAX_INVENTORY_LEN: usize = 50_000;
pub const MAX_SCHEDULED_LEN: usize = 4 * 1024;
pub const MAX_BLOCKS_TO_ANNOUNCE: usize = 8;
pub const MAX_UNCONNECTING_HEADERS: usize = 10;
pub const MAX_BLOCKS_IN_TRANSIT_PER_PEER: usize = 16;
pub const MAX_TIP_AGE: u64 = 60 * 60 * 1000;
pub const STALE_RELAY_AGE_LIMIT: u64 = 30 * 24 * 60 * 60 * 1000;
pub const BLOCK_DOWNLOAD_WINDOW: u64 = 1024;
pub const PER_FETCH_BLOCK_LIMIT: usize = 128;
pub const SYNC_PROTOCOL_ID: ProtocolId = *b"syn";
pub const RELAY_PROTOCOL_ID: ProtocolId = *b"rel";

//  Timeout = base + per_header * (expected number of headers)
pub const HEADERS_DOWNLOAD_TIMEOUT_BASE: u64 = 15 * 60 * 1000; // 15 minutes
pub const HEADERS_DOWNLOAD_TIMEOUT_PER_HEADER: u64 = 1; //1ms/header
pub const POW_SPACE: u64 = 10_000; //10s

// Protect at least this many outbound peers from disconnection due to slow
// behind headers chain.
pub const MAX_OUTBOUND_PEERS_TO_PROTECT_FROM_DISCONNECT: usize = 4;
pub const CHAIN_SYNC_TIMEOUT: u64 = 20 * 60 * 1000; // 20 minutes
pub const EVICTION_HEADERS_RESPONSE_TIME: u64 = 120 * 1000; // 2 minutes

//The maximum number of entries in a locator
pub const MAX_LOCATOR_SIZE: usize = 101;

pub const BLOCK_DOWNLOAD_TIMEOUT: u64 = 30 * 1000; // 30s
