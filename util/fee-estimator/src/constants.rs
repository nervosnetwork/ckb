//! The constants for the fee estimator.

use ckb_chain_spec::consensus::{MAX_BLOCK_INTERVAL, MIN_BLOCK_INTERVAL, TX_PROPOSAL_WINDOW};
use ckb_types::core::{BlockNumber, FeeRate};

/// Average block interval.
pub(crate) const AVG_BLOCK_INTERVAL: u64 = (MAX_BLOCK_INTERVAL + MIN_BLOCK_INTERVAL) / 2;

/// Max target blocks, about 1 hour.
pub(crate) const MAX_TARGET: BlockNumber = (60 * 60) / AVG_BLOCK_INTERVAL;
/// Min target blocks, in next block.
pub(crate) const MIN_TARGET: BlockNumber = TX_PROPOSAL_WINDOW.closest() + 1;

/// Lowest fee rate.
pub(crate) const LOWEST_FEE_RATE: FeeRate = FeeRate::from_u64(1000);

/// Target blocks for no priority (lowest priority, about 1 hour).
pub const DEFAULT_TARGET: BlockNumber = MAX_TARGET;
/// Target blocks for low priority (about 30 minutes).
pub const LOW_TARGET: BlockNumber = DEFAULT_TARGET / 2;
/// Target blocks for medium priority (about 10 minutes).
pub const MEDIUM_TARGET: BlockNumber = LOW_TARGET / 3;
/// Target blocks for high priority.
pub const HIGH_TARGET: BlockNumber = MIN_TARGET;
