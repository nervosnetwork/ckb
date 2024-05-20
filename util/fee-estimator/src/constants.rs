//! The constants for the fee estimator.

use ckb_chain_spec::consensus::{MAX_BLOCK_INTERVAL, MIN_BLOCK_INTERVAL};
use ckb_types::core::{BlockNumber, FeeRate};

/// Average block interval.
pub(crate) const AVG_BLOCK_INTERVAL: u64 = (MAX_BLOCK_INTERVAL + MIN_BLOCK_INTERVAL) / 2;

/// Max target blocks, about 2 hours.
pub(crate) const MAX_TARGET: BlockNumber = (60 * 2 * 60) / AVG_BLOCK_INTERVAL;
/// Min target blocks, about 5 minutes.
pub(crate) const MIN_TARGET: BlockNumber = (60 * 5) / AVG_BLOCK_INTERVAL;

/// Lowest fee rate.
pub(crate) const LOWEST_FEE_RATE: FeeRate = FeeRate::from_u64(1000);

/// Target blocks for default priority (lowest priority).
pub(crate) const DEFAULT_TARGET: BlockNumber = MAX_TARGET;
/// Target blocks for low priority.
pub(crate) const LOW_TARGET: BlockNumber = MIN_TARGET * 10;
/// Target blocks for medium priority.
pub(crate) const MEDIUM_TARGET: BlockNumber = MIN_TARGET * 2;
/// Target blocks for high priority.
pub(crate) const HIGH_TARGET: BlockNumber = MIN_TARGET;
