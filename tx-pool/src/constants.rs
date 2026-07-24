/// Threshold below which `HashMap`/`HashSet` capacity is allowed to shrink.
///
/// A collection is only shrunk when its len drops below this ratio of its
/// capacity. 100 is a simple floor: for very small collections the memory
/// savings are not worth the reallocation cost.
pub(crate) const SHRINK_THRESHOLD: usize = 100;

/// Shared slack for generation-tagged lazy ticket queues. Physical storage is
/// rebuilt once it exceeds twice the live set plus this fixed allowance.
pub(crate) const LAZY_TICKET_STALE_SLACK: usize = 64;

pub(crate) const fn lazy_ticket_compaction_limit(live: usize) -> usize {
    live.saturating_mul(2)
        .saturating_add(LAZY_TICKET_STALE_SLACK)
}

pub(crate) const SECONDS_PER_DAY: i32 = 24 * 60 * 60;
pub(crate) const MALFORMED_TX_BAN_SECONDS: u64 = 3 * (SECONDS_PER_DAY as u64);

pub(crate) const MIN_ESTIMATE_TARGET: u64 = 3;
pub(crate) const MAX_ESTIMATE_TARGET: u64 = 131;

pub(crate) const GAP_PROPOSAL_INDEX: u64 = 0;
pub(crate) const PROPOSED_PROPOSAL_INDEX: u64 = 1;

pub(crate) const VERIFY_CACHE_CHANNEL_SIZE: usize = 1024;
/// Maximum number of stable-state effect batches retained while external
/// consumers are slow. Bytes are bounded separately from the tx-pool config.
pub(crate) const EFFECT_OUTBOX_MAX_BATCHES: usize = 4096;
pub(crate) const MESSAGE_CONCURRENCY_MULTIPLIER: usize = 2;

/// Maximum number of coordinator conflict/capacity victims or authoritative
/// pool replacement candidates one transition may displace.
pub(crate) const MAX_RBF_REPLACEMENT_CANDIDATES: usize = 100;

/// Maximum time the shutdown path waits for each pipeline-worker group
/// (cache worker, maintenance, pre-check, verify, and resolver workers)
/// to finish its current job before persisting the tx-pool state.
pub(crate) const PIPELINE_SHUTDOWN_TIMEOUT_SECONDS: u64 = 30;
