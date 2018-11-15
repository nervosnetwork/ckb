//! # The Sync module
//!
//! Sync module implement ckb sync protocol as specified here:
//! https://github.com/nervosnetwork/rfcs/tree/master/rfcs/0000-block-sync-protocol

extern crate bigint;
extern crate fnv;
#[macro_use]
extern crate log;
extern crate ckb_chain;
extern crate ckb_core as core;
extern crate ckb_network as network;
extern crate ckb_pool as pool;
extern crate ckb_protocol;
extern crate ckb_shared;
extern crate ckb_time;
extern crate flatbuffers;
#[macro_use]
extern crate ckb_util as util;
extern crate ckb_verification;
#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate serde_derive;

#[cfg(test)]
extern crate ckb_chain_spec as chain_spec;
#[cfg(test)]
extern crate ckb_db as db;
#[cfg(test)]
extern crate ckb_notify;
#[cfg(test)]
extern crate ckb_pool;
#[cfg(test)]
extern crate crossbeam_channel;

mod config;
mod relayer;
mod synchronizer;

#[cfg(test)]
mod tests;

pub use config::Config;
pub use relayer::Relayer;
pub use synchronizer::Synchronizer;

use network::ProtocolId;

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
