pub(crate) const SECONDS_PER_DAY: i32 = 24 * 60 * 60;
pub(crate) const MALFORMED_TX_BAN_SECONDS: u64 = 3 * (SECONDS_PER_DAY as u64);
/// Maximum authority-local peer fences retained for controller-delayed Remote
/// ingress. Saturation evicts the soonest-expiring fence; it never blocks all Remote
/// admission or grows with unbounded session churn.
pub(crate) const PEER_BAN_FENCE_CAPACITY: usize = 1024;

pub(crate) const MIN_ESTIMATE_TARGET: u64 = 3;
pub(crate) const MAX_ESTIMATE_TARGET: u64 = 131;

pub(crate) const GAP_PROPOSAL_INDEX: u64 = 0;

/// Internal charged residency ceilings, separate from serialized pool capacity.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResidencyLimits {
    pub(crate) accepted: usize,
    pub(crate) pipeline: usize,
}

impl ResidencyLimits {
    pub(crate) fn from_pool_size(serialized: usize) -> Option<Self> {
        // Preserve the existing 1 GB accepted / 384 MB pipeline policy at the
        // 180 MB reference size. Smaller serialized limits still need room to
        // resolve dependencies and run normal transactions; larger pools scale
        // both ceilings proportionally. These are limits, not preallocations
        // or a whole-process RSS promise.
        const REFERENCE_SERIALIZED: u128 = 180_000_000;
        let scale = (serialized as u128).max(REFERENCE_SERIALIZED);
        let accepted =
            usize::try_from(1_000_000_000_u128.checked_mul(scale)? / REFERENCE_SERIALIZED).ok()?;
        let pipeline =
            usize::try_from(384_000_000_u128.checked_mul(scale)? / REFERENCE_SERIALIZED).ok()?;
        accepted.checked_add(pipeline)?;
        Some(Self { accepted, pipeline })
    }
}

/// Maximum number of stable-state effect batches retained while external
/// consumers are slow. Bytes are bounded separately from the tx-pool config.
pub(crate) const EFFECT_JOURNAL_REMOTE_MAX_BATCHES: usize = 4096;
/// Batches unavailable to Remote publication, preserving Local/Proposal and
/// bounded maintenance progress while an untrusted sink is saturated.
pub(crate) const EFFECT_TRUSTED_HEADROOM_BATCHES: usize = 64;
pub(crate) const MESSAGE_CONCURRENCY_MULTIPLIER: usize = 2;
/// One canonical ceiling for rejection diagnostics retained by either the
/// authority journal or the recent-reject projection. Keeping both consumers
/// on this value prevents an outcome that fits one committed boundary but can
/// never be published by the other.
pub(crate) const MAX_TX_POOL_REJECT_DESCRIPTION_BYTES: usize = 1024;

/// Maximum number of entries one indexed conflict, capacity, or ancestor
/// displacement sub-transition may visit or remove. Reorg reconciliation and
/// configured pool-size trimming have separate, formula-bounded cohorts.
pub(crate) const MAX_POOL_MUTATION_CANDIDATES: usize = 100;

/// Grace period for draining handlers/workers, then the notice publisher.
/// Exceeding either deadline faults the generation and skips persistence;
/// joining synchronous provider work still requires that provider to return.
pub(crate) const PIPELINE_SHUTDOWN_TIMEOUT_SECONDS: u64 = 30;
