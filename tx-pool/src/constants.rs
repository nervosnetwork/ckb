/// Default maximum total serialized size (in bytes) of transactions queued in the
/// pipeline (pre-check + resolve + verify queues combined).
///
/// This bounds the memory footprint of transactions that have been received but
/// not yet fully accepted into the mempool. 256 MB is large enough to absorb
/// transaction bursts while preventing unbounded memory growth under load.
pub(crate) const DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE: usize = 256_000_000;

/// Threshold below which `HashMap`/`HashSet` capacity is allowed to shrink.
///
/// A collection is only shrunk when its len drops below this ratio of its
/// capacity. 100 is a simple floor: for very small collections the memory
/// savings are not worth the reallocation cost.
pub(crate) const SHRINK_THRESHOLD: usize = 100;

pub(crate) const SECONDS_PER_DAY: i32 = 24 * 60 * 60;
pub(crate) const MALFORMED_TX_BAN_SECONDS: u64 = 3 * (SECONDS_PER_DAY as u64);

pub(crate) const MIN_ESTIMATE_TARGET: u64 = 3;
pub(crate) const MAX_ESTIMATE_TARGET: u64 = 131;

pub(crate) const GAP_PROPOSAL_INDEX: u64 = 0;
pub(crate) const PROPOSED_PROPOSAL_INDEX: u64 = 1;

pub(crate) const DEFERRED_CHANNEL_SIZE: usize = 1024;
pub(crate) const MESSAGE_CONCURRENCY_MULTIPLIER: usize = 2;
