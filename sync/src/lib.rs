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
extern crate nervos_util as util;
extern crate protobuf;
extern crate rand;
extern crate siphasher;

pub mod chain;
pub mod compact_block;
mod executor;
mod peers;
pub mod protocol;
mod queue;

pub const MAX_HEADERS_LEN: usize = 2_000;
pub const MAX_INVENTORY_LEN: usize = 50_000;
pub const MAX_SCHEDULED_LEN: usize = 4 * 1024;
pub type BlockHeight = u64;
