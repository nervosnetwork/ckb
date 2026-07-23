/// Maximum total serialized size (in bytes) of transactions queued in the
/// pre-check queue.
///
/// The pre-check queue only absorbs submission bursts: it is drained by a
/// pool of parallel workers whose per-job cost (resolve + fee check) is
/// millisecond-scale, so occupancy stays low even under load. 64 MB holds
/// on the order of 100k typical transactions (~300-600 bytes each), far
/// above any legitimate burst; a larger buffer would only grow the flood
/// footprint without improving throughput.
pub(crate) const MAX_PRE_CHECK_QUEUE_TX_SIZE: usize = 64_000_000;

/// Maximum total serialized size (in bytes) of transactions queued in the
/// ordered resolve queue.
///
/// Entries in this queue are waiting for their parents to arrive — a
/// network-time event, not worker throughput — so a larger buffer does not
/// speed anything up; it only lets more unsatisfiable transactions linger
/// (each retried by the single ordered resolver on a 50 ms cadence).
/// 64 MB is far above the size of legitimate dependent backlogs.
pub(crate) const MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE: usize = 64_000_000;

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
/// Maximum number of stable-state effect batches retained while external
/// consumers are slow. Bytes are bounded separately from the tx-pool config.
pub(crate) const EFFECT_OUTBOX_MAX_BATCHES: usize = 4096;
pub(crate) const MESSAGE_CONCURRENCY_MULTIPLIER: usize = 2;

/// Maximum number of distinct in-flight registrations one RBF candidate may
/// displace. Keep this aligned with the main pool's replacement-candidate
/// bound: the speculative scheduling gate must not expose a larger O(n)
/// operation than the authoritative RBF check it precedes.
pub(crate) const MAX_RBF_REPLACEMENT_CANDIDATES: usize = 100;

/// Maximum time the shutdown path waits for each pipeline-worker group
/// (deferred worker, pre-check workers, verify manager, ordered resolver)
/// to finish its current job before persisting the tx-pool state.
pub(crate) const PIPELINE_SHUTDOWN_TIMEOUT_SECONDS: u64 = 30;
