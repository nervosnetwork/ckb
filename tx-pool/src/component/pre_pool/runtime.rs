use super::*;
use crate::error::Reject;
use crate::resolved_tx::PoolCandidate;
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use ckb_verification::cache::Completed;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const ENTRY_INDEX_BYTES: usize = 768;
const DEPENDENCY_EDGE_BYTES: usize = 160;
const CONFLICT_HISTORY_MAX_ENTRIES: usize = 10_000;
const CONFLICT_HISTORY_MAX_BYTES: usize = 50_000_000;
const REMOTE_RESIDENCY_BLOCKS: u64 = 100;

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

    pub(crate) fn authoritative_source(
        &self,
        source: PrePoolSource,
    ) -> Result<TxSource, PrePoolError> {
        match source {
            PrePoolSource::Proposal => Ok(TxSource::Proposal),
            PrePoolSource::Remote(peer) => self
                .declared_cycles
                .filter(|_| self.ingress_peer == Some(peer) && self.payload_peer == Some(peer))
                .map(|cycles| TxSource::Remote { cycles, peer })
                .ok_or(PrePoolError::Repair("source attribution mismatch")),
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

const WORKER_CLASSES: [(WorkLane, WorkCapability); 5] = [
    (WorkLane::Ingress, WorkCapability::Any),
    (WorkLane::Resolve, WorkCapability::Any),
    (WorkLane::Verify, WorkCapability::Any),
    (WorkLane::Verify, WorkCapability::SmallCycleOnly),
    (WorkLane::Commit, WorkCapability::Any),
];

/// Stable asynchronous shell around [`PrePoolKernel`]. Notifications are hints;
/// exact queue membership remains under the kernel mutex. The only service-wide
/// failure latch retained at P1 is for a panic that crossed an already-mutating
/// TxPool boundary; P2/P4 replace that legacy pool path with total Apply.
pub(crate) struct PrePool {
    state: Mutex<PrePoolKernel>,
    authoritative_failed: AtomicBool,
    ready: [Arc<Notify>; WORKER_CLASSES.len()],
    maintenance_ready: Arc<Notify>,
    shutdown: CancellationToken,
    commit_serial: tokio::sync::Mutex<()>,
    max_entries: usize,
}

impl PrePool {
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        shutdown: CancellationToken,
    ) -> Self {
        let total_bytes = config.tx_pipeline_resident_size_budget();
        assert!(
            total_bytes >= ENTRY_INDEX_BYTES,
            "tx_pool.max_tx_pipeline_resident_size must hold one pre-pool entry"
        );
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
        Self {
            state: Mutex::new(PrePoolKernel::new(limits)),
            authoritative_failed: AtomicBool::new(false),
            ready: WORKER_CLASSES.map(|_| Arc::new(Notify::new())),
            maintenance_ready: Arc::new(Notify::new()),
            shutdown,
            commit_serial: tokio::sync::Mutex::new(()),
            max_entries: total_entries,
        }
    }

    fn lock(&self) -> MutexGuard<'_, PrePoolKernel> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                // A worker panic is contained to its borrowed command. The
                // primary entry map remains the ownership oracle; exact
                // projections are rebuilt at P4's DefectDomain boundary.
                ckb_logger::error!("pre-pool mutex was poisoned; retaining primary state");
                poisoned.into_inner()
            }
        }
    }

    pub(crate) fn read<T>(&self, inspect: impl FnOnce(&PrePoolKernel) -> T) -> T {
        inspect(&self.lock())
    }

    pub(crate) fn mutate<T>(&self, apply: impl FnOnce(&mut PrePoolKernel) -> T) -> T {
        let (result, ready, maintenance) = {
            let mut state = self.lock();
            let result = apply(&mut state);
            let ready = WORKER_CLASSES
                .map(|(lane, capability)| state.work_is_ready(lane, capability).unwrap_or(false));
            (result, ready, state.wait_wake_pending())
        };
        for (notify, executable) in self.ready.iter().zip(ready) {
            if executable {
                notify.notify_one();
            }
        }
        if maintenance {
            self.maintenance_ready.notify_one();
        }
        result
    }

    pub(crate) fn mutate_required<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce(&mut PrePoolKernel) -> Result<T, PrePoolError>,
    ) -> T {
        match self.mutate(apply) {
            Ok(value) => value,
            Err(error) => panic!("{context}: {error:?}"),
        }
    }

    pub(crate) fn mutate_lease<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce(&mut PrePoolKernel) -> Result<T, PrePoolError>,
    ) -> Option<T> {
        match self.mutate(apply) {
            Ok(value) => Some(value),
            Err(error) if error.is_stale_lease() => None,
            Err(error) => panic!("{context}: {error:?}"),
        }
    }

    pub(crate) fn reject_or_fail(&self, context: &'static str, error: PrePoolError) -> Reject {
        if error.is_transaction_rejection() {
            pre_pool_reject(error)
        } else {
            ckb_logger::error!("{context}: {error:?}; rejecting only the affected command");
            Reject::Full("pre-pool repair required".to_string())
        }
    }

    pub(crate) fn fail_stop(&self, context: &'static str, error: &impl std::fmt::Debug) -> ! {
        // Compatibility for P1 callers that still use the old name. This is a
        // task-local defect panic and deliberately does not cancel the service.
        panic!("{context}: {error:?}")
    }

    pub(crate) fn guard_authoritative_mutation<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce() -> T,
    ) -> T {
        match catch_unwind(AssertUnwindSafe(apply)) {
            Ok(value) => value,
            Err(payload) => {
                self.authoritative_failed.store(true, Ordering::Release);
                self.shutdown.cancel();
                let message = crate::util::panic_payload_to_string(payload.as_ref());
                panic!("{context}: {message}")
            }
        }
    }

    pub(crate) fn guard_stable_effect_journal<T>(
        &self,
        _context: &'static str,
        apply: impl FnOnce() -> T,
    ) -> T {
        match catch_unwind(AssertUnwindSafe(apply)) {
            Ok(value) => value,
            Err(payload) => resume_unwind(payload),
        }
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.authoritative_failed.load(Ordering::Acquire)
    }

    pub(crate) fn pool_persistence_safe(&self) -> bool {
        !self.is_failed()
    }

    pub(crate) fn require_authoritative_source(
        &self,
        raw: &PipelineRawTx,
        source: PrePoolSource,
    ) -> TxSource {
        raw.authoritative_source(source)
            .unwrap_or_else(|error| panic!("pre-pool source attribution: {error:?}"))
    }

    pub(crate) fn subscribe(&self, lane: WorkLane, capability: WorkCapability) -> Arc<Notify> {
        let index = WORKER_CLASSES
            .iter()
            .position(|class| *class == (lane, capability))
            .expect("worker class belongs to the closed registry");
        let notify = Arc::clone(&self.ready[index]);
        if self.read(|state| state.work_is_ready(lane, capability).unwrap_or(false)) {
            notify.notify_one();
        }
        notify
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

    pub(crate) fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub(crate) fn queue_is_empty(&self, lane: WorkLane) -> bool {
        self.read(|state| state.queue_len(lane) == 0)
    }

    pub(crate) fn admit_transaction_journaled(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        lane: ResolveLane,
        journal: impl FnOnce(&[TerminalRecord]),
    ) -> Result<(bool, Vec<TerminalRecord>), PrePoolError> {
        let hash = tx.hash();
        let short_id = tx.proposal_short_id();
        let result = self.mutate(|kernel| {
            if let Some(existing) = kernel.entries.get(&hash).cloned() {
                return match source {
                    TxSource::Local => Err(PrePoolError::LocalMustRunDirect),
                    TxSource::Remote { .. } => Ok((false, Vec::new())),
                    TxSource::Proposal => {
                        if existing.raw.tx.witness_hash() != tx.witness_hash() {
                            let raw = existing.raw.trusted_variant(tx, epoch);
                            let raw_bytes = raw.charge_bytes();
                            kernel.replace_raw_payload(&hash, raw, raw_bytes, lane)?;
                        } else {
                            kernel.promote_source(&hash)?;
                        }
                        Ok((false, Vec::new()))
                    }
                };
            }
            let owner = pre_pool_source(source)?;
            let raw = PipelineRawTx::new(tx, source, epoch);
            let raw_bytes = raw.charge_bytes();
            let dependencies = conflict_dependency_keys(&raw.tx, std::iter::empty());
            let expires_at = historical_deadline(owner);
            kernel.admit(
                hash,
                short_id,
                raw,
                lane,
                owner,
                expires_at,
                raw_bytes,
                dependencies,
            )?;
            Ok((true, Vec::new()))
        });
        if let Ok((_, terminal)) = &result {
            journal(terminal);
        }
        result
    }

    pub(crate) fn checkout_resolve(&self, lane: ResolveLane) -> Option<ResolveLease> {
        self.mutate_required("resolve checkout failed", |state| {
            state.checkout_resolve(lane)
        })
    }

    pub(crate) async fn wait_resolve(&self, lane: ResolveLane) -> Option<ResolveLease> {
        let work_lane = PrePoolKernel::lane_for_resolve(lane);
        let ready = self.subscribe(work_lane, WorkCapability::Any);
        loop {
            if self.shutdown.is_cancelled() {
                return None;
            }
            if let Some(lease) = self.checkout_resolve(lane) {
                return Some(lease);
            }
            tokio::select! {
                _ = ready.notified() => {}
                _ = self.shutdown.cancelled() => return None,
            }
        }
    }

    pub(crate) async fn lock_commit_driver(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.commit_serial.lock().await
    }

    pub(crate) fn try_lock_commit_driver(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.commit_serial.try_lock().ok()
    }
}

pub(crate) fn pre_pool_source(source: TxSource) -> Result<PrePoolSource, PrePoolError> {
    match source {
        TxSource::Remote { peer, .. } => Ok(PrePoolSource::Remote(peer)),
        TxSource::Proposal => Ok(PrePoolSource::Proposal),
        TxSource::Local => Err(PrePoolError::LocalMustRunDirect),
    }
}

pub(crate) fn historical_source(source: TxSource) -> PrePoolSource {
    match source {
        TxSource::Remote { peer, .. } => PrePoolSource::Remote(peer),
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

pub(crate) fn pre_pool_reject(error: PrePoolError) -> Reject {
    match error {
        PrePoolError::UnderReplacementFee { .. }
        | PrePoolError::UnderFeeRate { .. }
        | PrePoolError::FeeRateOverflow
        | PrePoolError::ResidencyChargeOverflow
        | PrePoolError::ZeroTransactionSize(_)
        | PrePoolError::SelfDependency(_) => Reject::Malformed(
            "transaction".to_string(),
            format!("pre-pool policy rejection: {error:?}"),
        ),
        _ => Reject::Full(format!("pre-pool backpressure: {error:?}")),
    }
}
