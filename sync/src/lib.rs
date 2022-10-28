//! # The Sync module
//!
//! Sync module implement ckb sync protocol as specified here:
//! https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0004-ckb-block-sync/0004-ckb-block-sync.md

mod block_status;
mod filter;
pub(crate) mod net_time_checker;
pub(crate) mod orphan_block_pool;
mod relayer;
mod status;
mod synchronizer;
mod types;
mod utils;

#[cfg(test)]
mod tests;

pub use crate::filter::BlockFilter;
pub use crate::net_time_checker::NetTimeProtocol;
pub use crate::relayer::Relayer;
pub use crate::status::{Status, StatusCode};
pub use crate::synchronizer::Synchronizer;
pub use crate::types::{ActiveChain, SyncShared};
use ckb_constant::sync::MAX_BLOCKS_IN_TRANSIT_PER_PEER;

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

pub(crate) const LOG_TARGET_FILTER: &str = "ckb_filter";
