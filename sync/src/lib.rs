#![feature(slice_patterns)]

extern crate bigint;
extern crate fnv;
#[macro_use]
extern crate log;
extern crate byteorder;
extern crate hash;
extern crate nervos_chain;
extern crate nervos_core as core;
extern crate nervos_network as network;
extern crate nervos_notify;
extern crate nervos_pool as pool;
extern crate nervos_protocol;
extern crate nervos_time;
#[macro_use]
extern crate nervos_util as util;
extern crate nervos_verification;
extern crate protobuf;
extern crate rand;
extern crate siphasher;
#[macro_use]
extern crate bitflags;
extern crate futures;
extern crate rayon;
extern crate tokio;
#[macro_use]
extern crate serde_derive;
#[cfg(test)]
extern crate nervos_db as db;

pub mod block_pool;
pub mod block_process;
pub mod compact_block;
// pub mod compact_block_process;
pub mod config;
pub mod getdata_process;
pub mod getheaders_process;
pub mod header_view;
pub mod headers_process;
pub mod peers;
pub mod protocol;
pub mod synchronizer;

pub use config::Config;
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
pub type BlockNumber = u64;
pub const SYNC_PROTOCOL_ID: ProtocolId = *b"syn";
pub const RELAY_PROTOCOL_ID: ProtocolId = *b"rel";
