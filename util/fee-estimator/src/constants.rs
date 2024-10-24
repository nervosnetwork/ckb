//! The constants for the fee estimator.

use ckb_chain_spec::consensus::{MAX_BLOCK_INTERVAL, MIN_BLOCK_INTERVAL, TX_PROPOSAL_WINDOW};
use ckb_types::core::{BlockNumber, FeeRate};

/// Average block interval (28).
pub(crate) const AVG_BLOCK_INTERVAL: u64 = (MAX_BLOCK_INTERVAL + MIN_BLOCK_INTERVAL) / 2;

/// Max target blocks, about 1 hour (128).
pub(crate) const MAX_TARGET: BlockNumber = (60 * 60) / AVG_BLOCK_INTERVAL;
/// Min target blocks, in next block (5).
/// NOTE After tests, 3 blocks are too strict; so to adjust larger: 5.
pub(crate) const MIN_TARGET: BlockNumber = (TX_PROPOSAL_WINDOW.closest() + 1) + 2;

/// Lowest fee rate.
pub(crate) const LOWEST_FEE_RATE: FeeRate = FeeRate::from_u64(1000);

/// Target blocks for no priority (lowest priority, about 1 hour, 128).
pub const DEFAULT_TARGET: BlockNumber = MAX_TARGET;
/// Target blocks for low priority (about 30 minutes, 64).
pub const LOW_TARGET: BlockNumber = DEFAULT_TARGET / 2;
/// Target blocks for medium priority (about 10 minutes, 42).
pub const MEDIUM_TARGET: BlockNumber = LOW_TARGET / 3;
/// Target blocks for high priority (3).
pub const HIGH_TARGET: BlockNumber = MIN_TARGET;
