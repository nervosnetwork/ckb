pub(crate) const SECONDS_PER_DAY: i32 = 24 * 60 * 60;
pub(crate) const MALFORMED_TX_BAN_SECONDS: u64 = 3 * (SECONDS_PER_DAY as u64);

pub(crate) const MIN_ESTIMATE_TARGET: u64 = 3;
pub(crate) const MAX_ESTIMATE_TARGET: u64 = 131;

pub(crate) const GAP_PROPOSAL_INDEX: u64 = 0;
pub(crate) const PROPOSED_PROPOSAL_INDEX: u64 = 1;

pub(crate) const VERIFY_CACHE_CHANNEL_SIZE: usize = 1024;
/// Maximum number of stable-state effect batches retained while external
/// consumers are slow. Bytes are bounded separately from the tx-pool config.
pub(crate) const EFFECT_JOURNAL_REMOTE_MAX_BATCHES: usize = 4096;
/// Batches unavailable to Remote publication, preserving Local/Proposal and
/// bounded maintenance progress while an untrusted sink is saturated.
pub(crate) const EFFECT_TRUSTED_HEADROOM_BATCHES: usize = 64;
pub(crate) const MESSAGE_CONCURRENCY_MULTIPLIER: usize = 2;

/// Maximum number of entries one indexed conflict, capacity, or ancestor
/// displacement sub-transition may visit or remove. Reorg reconciliation and
/// configured pool-size trimming have separate, formula-bounded cohorts.
pub(crate) const MAX_POOL_MUTATION_CANDIDATES: usize = 100;

/// Maximum time the shutdown path waits for each pipeline-worker group
/// (cache worker, maintenance, pre-check, verify, and resolver workers)
/// to finish its current job before persisting the tx-pool state.
pub(crate) const PIPELINE_SHUTDOWN_TIMEOUT_SECONDS: u64 = 30;
