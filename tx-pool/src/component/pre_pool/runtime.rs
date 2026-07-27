use super::*;
use crate::error::Reject;
use crate::resolved_tx::PoolCandidate;
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use ckb_util::{Mutex, MutexGuard};
use ckb_verification::cache::Completed;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const ENTRY_INDEX_BYTES: usize = 768;
const DEPENDENCY_EDGE_BYTES: usize = 160;
const CONFLICT_HISTORY_MAX_ENTRIES: usize = 10_000;
const CONFLICT_HISTORY_MAX_BYTES: usize = 50_000_000;
const REMOTE_RESIDENCY_BLOCKS: u64 = 100;

/// The only origins permitted to acquire pre-pool ownership. Local RPC
/// submissions are absent from this type and therefore cannot accidentally
/// enter the asynchronous pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PipelineAdmissionSource {
    Remote(RemoteSource),
    Proposal,
}

impl PipelineAdmissionSource {
    pub(crate) fn from_tx_source(source: TxSource) -> Option<Self> {
        match source {
            TxSource::Remote { cycles, peer } => {
                Some(Self::Remote(RemoteSource::new(peer, cycles)))
            }
            TxSource::Proposal => Some(Self::Proposal),
            TxSource::Local => None,
        }
    }

    fn owner(self) -> PrePoolSource {
        match self {
            Self::Remote(remote) => PrePoolSource::Remote(remote),
            Self::Proposal => PrePoolSource::Proposal,
        }
    }

    fn tx_source(self) -> TxSource {
        match self {
            Self::Remote(remote) => remote.tx_source(),
            Self::Proposal => TxSource::Proposal,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PipelineRawTx {
    pub(crate) tx: TransactionView,
    pub(crate) declared_cycles: Option<Cycle>,
    ingress_peer: Option<PeerIndex>,
    payload_peer: Option<PeerIndex>,
    pub(crate) admitted_epoch: u64,
}

impl PipelineRawTx {
    pub(crate) fn new(tx: TransactionView, source: TxSource, admitted_epoch: u64) -> Self {
        Self {
            tx: tx.into_compact(),
            declared_cycles: source.cycles(),
            ingress_peer: source.peer(),
            payload_peer: source.peer(),
            admitted_epoch,
        }
    }

    pub(crate) fn ingress_peer(&self) -> Option<PeerIndex> {
        self.ingress_peer
    }

    pub(crate) fn blame_peer(&self) -> Option<PeerIndex> {
        self.payload_peer
    }

    fn trusted_variant(&self, tx: TransactionView, admitted_epoch: u64) -> Self {
        let same_witness = self.tx.witness_hash() == tx.witness_hash();
        Self {
            tx: tx.into_compact(),
            declared_cycles: self.declared_cycles,
            ingress_peer: self.ingress_peer,
            payload_peer: same_witness.then_some(self.payload_peer).flatten(),
            admitted_epoch,
        }
    }

    pub(crate) fn authoritative_source(&self, source: PrePoolSource) -> TxSource {
        match source {
            PrePoolSource::Proposal => TxSource::Proposal,
            // Recovery owns its payload in the kernel but deliberately uses
            // the direct trusted validation policy. Public Local admission is
            // still rejected by `admit_transaction`.
            PrePoolSource::Recovery => TxSource::Local,
            PrePoolSource::Remote(remote) => remote.tx_source(),
        }
    }

    pub(crate) fn recovery(tx: TransactionView, admitted_epoch: u64) -> Self {
        Self {
            tx: tx.into_compact(),
            declared_cycles: None,
            ingress_peer: None,
            payload_peer: None,
            admitted_epoch,
        }
    }

    pub(crate) fn charge_bytes(&self) -> usize {
        self.tx.data().serialized_size_in_block()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PipelineVerifiedTx {
    pub(crate) candidate: PoolCandidate,
    pub(crate) completed: Completed,
    pub(crate) verify_cache_hit: bool,
    pub(crate) started_at: Instant,
}

/// Stable asynchronous shell around [`PrePoolKernel`]. Notifications are hints;
/// exact queue membership remains under the kernel mutex. The only service-wide
/// failure latch records typed structural contradictions or unexpected worker
/// termination. It is not a rollback/retry protocol and legal transaction
/// outcomes never reach it.
pub(crate) struct PrePool {
    state: Mutex<PrePoolKernel>,
    ingress_ready: Arc<Notify>,
    resolve_ready: Arc<Notify>,
    verify_ready: Arc<Notify>,
    small_cycle_ready: Arc<Notify>,
    commit_ready: Arc<Notify>,
    maintenance_ready: Arc<Notify>,
    shutdown: CancellationToken,
    failed: AtomicBool,
}

impl PrePool {
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        shutdown: CancellationToken,
    ) -> Result<Self, PrePoolError> {
        let total_bytes = config.tx_pipeline_resident_size_budget();
        if total_bytes < ENTRY_INDEX_BYTES {
            return Err(PrePoolError::InvalidConfiguration(
                "tx_pool.max_tx_pipeline_resident_size must hold one pre-pool entry",
            ));
        }
        let total_entries = total_bytes / ENTRY_INDEX_BYTES;
        let remote_bytes = total_bytes.saturating_mul(7) / 8;
        let remote_entries = total_entries.saturating_mul(7) / 8;
        let per_peer = Residency::new(
            remote_entries.saturating_div(8).max(1),
            remote_bytes.saturating_div(8).max(1),
        );
        let conflict_history = Residency::new(
            CONFLICT_HISTORY_MAX_ENTRIES.min(total_entries.saturating_div(16).max(1)),
            CONFLICT_HISTORY_MAX_BYTES.min(total_bytes.saturating_div(16).max(1)),
        );
        let max_dependencies = (consensus.max_block_bytes() as usize)
            .saturating_div(32)
            .saturating_add(1);
        let verify_workers = config.max_tx_verify_workers.max(1);
        let resolve_workers = verify_workers
            .min(std::thread::available_parallelism().map_or(4, |count| count.get()))
            .saturating_add(1);
        let max_active_work = resolve_workers.saturating_add(verify_workers);
        let limits = PrePoolLimits {
            total: Residency::new(total_entries, total_bytes),
            remote: Residency::new(remote_entries, remote_bytes),
            per_peer,
            conflict_history,
            max_dependencies_per_entry: max_dependencies,
            max_dependents_per_parent: 4_096usize.min(total_entries).max(1),
            max_inputs_per_ready: max_dependencies,
            max_candidates_per_input: crate::constants::MAX_POOL_MUTATION_CANDIDATES,
            max_active_work,
            max_active_work_per_peer: max_active_work.saturating_div(4).max(1),
            entry_overhead: ENTRY_INDEX_BYTES,
            dependency_overhead: DEPENDENCY_EDGE_BYTES,
            verify_fee_rate_ordering: matches!(config.verify_ordering, VerifyOrdering::FeeRate),
        };
        Ok(Self {
            state: Mutex::new(PrePoolKernel::new(limits)),
            ingress_ready: Arc::new(Notify::new()),
            resolve_ready: Arc::new(Notify::new()),
            verify_ready: Arc::new(Notify::new()),
            small_cycle_ready: Arc::new(Notify::new()),
            commit_ready: Arc::new(Notify::new()),
            maintenance_ready: Arc::new(Notify::new()),
            shutdown,
            failed: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> MutexGuard<'_, PrePoolKernel> {
        self.state.lock()
    }

    pub(crate) fn read<T>(&self, inspect: impl FnOnce(&PrePoolKernel) -> T) -> T {
        inspect(&self.lock())
    }

    /// The sole mutation boundary.  Transaction rejection, backpressure and
    /// stale leases are returned by the transition itself; this shell never
    /// converts an invariant panic into a second state-management protocol.
    pub(crate) fn mutate_authoritative<T>(&self, apply: impl FnOnce(&mut PrePoolKernel) -> T) -> T {
        let (result, ready, maintenance) = {
            let mut state = self.lock();
            let result = apply(&mut state);
            let ready = (
                state.work_is_ready(WorkLane::Ingress, WorkCapability::Any),
                state.work_is_ready(WorkLane::Resolve, WorkCapability::Any),
                state.work_is_ready(WorkLane::Verify, WorkCapability::Any),
                state.work_is_ready(WorkLane::Verify, WorkCapability::SmallCycleOnly),
                state.work_is_ready(WorkLane::Commit, WorkCapability::Any),
            );
            (result, ready, state.wait_wake_pending())
        };
        let (ingress, resolve, verify, small_cycle, commit) = ready;
        if ingress {
            self.ingress_ready.notify_one();
        }
        if resolve {
            self.resolve_ready.notify_one();
        }
        if verify {
            self.verify_ready.notify_one();
        }
        if small_cycle {
            self.small_cycle_ready.notify_one();
        }
        if commit {
            self.commit_ready.notify_one();
        }
        if maintenance {
            self.maintenance_ready.notify_one();
        }
        result
    }

    fn subscribe_named(
        &self,
        notify: &Arc<Notify>,
        lane: WorkLane,
        capability: WorkCapability,
    ) -> Arc<Notify> {
        let notify = Arc::clone(notify);
        if self.read(|state| state.work_is_ready(lane, capability)) {
            notify.notify_one();
        }
        notify
    }

    pub(crate) fn subscribe_resolve(&self, lane: ResolveLane) -> Arc<Notify> {
        match lane {
            ResolveLane::Ingress => {
                self.subscribe_named(&self.ingress_ready, WorkLane::Ingress, WorkCapability::Any)
            }
            ResolveLane::Ordered => {
                self.subscribe_named(&self.resolve_ready, WorkLane::Resolve, WorkCapability::Any)
            }
        }
    }

    pub(crate) fn subscribe_verify(&self, capability: WorkCapability) -> Arc<Notify> {
        match capability {
            WorkCapability::Any => {
                self.subscribe_named(&self.verify_ready, WorkLane::Verify, WorkCapability::Any)
            }
            WorkCapability::SmallCycleOnly => self.subscribe_named(
                &self.small_cycle_ready,
                WorkLane::Verify,
                WorkCapability::SmallCycleOnly,
            ),
        }
    }

    pub(crate) fn subscribe_commit(&self) -> Arc<Notify> {
        self.subscribe_named(&self.commit_ready, WorkLane::Commit, WorkCapability::Any)
    }

    pub(crate) fn subscribe_maintenance(&self) -> Arc<Notify> {
        let notify = Arc::clone(&self.maintenance_ready);
        if self.read(PrePoolKernel::wait_wake_pending) {
            notify.notify_one();
        }
        notify
    }

    pub(crate) fn maintenance_pending(&self) -> bool {
        self.read(PrePoolKernel::wait_wake_pending)
    }

    /// Contain a typed kernel defect without unwinding through an authority
    /// lock. Legal transaction outcomes inhabit other error variants; this
    /// marks the generation ineligible for persistence before quiescence.
    pub(crate) fn report_fault(&self, context: &'static str, error: &impl std::fmt::Debug) {
        ckb_logger::error!("{context}: {error:?}");
        self.failed.store(true, Ordering::Release);
        self.shutdown.cancel();
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    pub(crate) fn queue_is_empty(&self, lane: WorkLane) -> bool {
        self.read(|state| state.queue_len(lane) == 0)
    }

    pub(crate) fn admit_transaction(
        &self,
        tx: TransactionView,
        source: PipelineAdmissionSource,
        epoch: u64,
        lane: ResolveLane,
    ) -> Result<bool, PrePoolAdmissionError> {
        let hash = tx.hash();
        let admitted: Result<bool, PrePoolError> = self.mutate_authoritative(|kernel| {
            if kernel.entries.contains_key(&hash) {
                return match source {
                    PipelineAdmissionSource::Remote(_) => Ok(false),
                    PipelineAdmissionSource::Proposal => {
                        let trusted_variant = {
                            let existing = kernel.entries.get(&hash).ok_or_else(|| {
                                PrePoolError::ProjectionInconsistent(
                                    "observed admission owner lost its primary",
                                )
                            })?;
                            (existing.raw.tx.witness_hash() != tx.witness_hash())
                                .then(|| existing.raw.trusted_variant(tx, epoch))
                        };
                        if let Some(raw) = trusted_variant {
                            let raw_bytes = raw.charge_bytes();
                            kernel.replace_raw_payload(&hash, raw, raw_bytes, lane)?;
                        } else {
                            kernel.promote_source(&hash)?;
                        }
                        Ok(false)
                    }
                };
            }
            let owner = source.owner();
            let raw = PipelineRawTx::new(tx, source.tx_source(), epoch);
            let dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty());
            let expires_at = historical_deadline(owner);
            kernel.admit(raw, lane, owner, expires_at, dependencies)?;
            Ok(true)
        });
        admitted.map_err(PrePoolAdmissionError::from)
    }

    pub(crate) fn checkout_resolve(
        &self,
        lane: ResolveLane,
    ) -> Result<Option<ResolveLease>, PrePoolError> {
        self.mutate_authoritative(|state| state.checkout_resolve(lane))
    }

    pub(crate) fn recovery_snapshot(&self) -> Vec<TransactionView> {
        self.read(PrePoolKernel::recovery_snapshot)
    }

    pub(crate) async fn wait_resolve(
        &self,
        lane: ResolveLane,
    ) -> Result<Option<ResolveLease>, PrePoolError> {
        let ready = self.subscribe_resolve(lane);
        loop {
            if self.shutdown.is_cancelled() {
                return Ok(None);
            }
            if let Some(lease) = self.checkout_resolve(lane)? {
                return Ok(Some(lease));
            }
            tokio::select! {
                _ = ready.notified() => {}
                _ = self.shutdown.cancelled() => return Ok(None),
            }
        }
    }
}

impl PrePoolKernel {
    /// Prepare a fresh chain generation off-authority and swap it in with one
    /// move. The same primitive is usable while the caller already owns the
    /// kernel mutex, so a failed reorg can converge before any worker observes
    /// its discarded in-place draft.
    pub(crate) fn replace_generation_for_chain<T>(
        &mut self,
        prepare: impl FnOnce(&mut PrePoolKernel) -> Result<T, PrePoolError>,
    ) -> Result<(T, PrePoolGeneration), PrePoolError> {
        let mut prepared = PrePoolKernel {
            generation: PrePoolGeneration::new(),
            limits: self.limits,
            next_version: self.next_version,
            next_arrival: self.next_arrival,
        };
        let value = prepare(&mut prepared)?;
        let retired = std::mem::replace(&mut self.generation, prepared.generation);
        self.next_version = prepared.next_version;
        self.next_arrival = prepared.next_arrival;
        Ok((value, retired))
    }

    /// Install the unique valid empty generation without allocation-sized
    /// traversal. Process-global ABA clocks deliberately remain monotonic.
    pub(crate) fn replace_empty_generation(&mut self) -> PrePoolGeneration {
        std::mem::replace(&mut self.generation, PrePoolGeneration::new())
    }
}

pub(crate) fn historical_source(source: TxSource) -> PrePoolSource {
    match source {
        TxSource::Remote { cycles, peer } => PrePoolSource::Remote(RemoteSource::new(peer, cycles)),
        TxSource::Local | TxSource::Proposal => PrePoolSource::Proposal,
    }
}

pub(crate) fn historical_deadline(source: PrePoolSource) -> Option<u64> {
    source.peer().map(|_| {
        ckb_systemtime::unix_time()
            .as_secs()
            .saturating_add(REMOTE_RESIDENCY_BLOCKS * ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL)
    })
}

#[cfg(test)]
#[path = "../tests/pre_pool_runtime_seam.rs"]
mod test_seam;

pub(crate) fn pre_pool_reject(error: PrePoolPublicError) -> Reject {
    match &error {
        PrePoolPublicError::Rejected(_) => Reject::Malformed(
            "transaction".to_string(),
            format!("pre-pool policy rejection: {error:?}"),
        ),
        PrePoolPublicError::Backpressure(_) => {
            Reject::Full(format!("pre-pool backpressure: {error:?}"))
        }
    }
}
