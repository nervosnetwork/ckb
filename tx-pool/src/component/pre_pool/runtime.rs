use super::*;
use crate::error::Reject;
use crate::resolved_tx::PoolCandidate;
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use ckb_verification::cache::Completed;
use std::sync::{Arc, Mutex, MutexGuard};
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
            // Recovery owns its payload in the kernel but deliberately uses
            // the direct trusted validation policy. Public Local admission is
            // still rejected by `admit_transaction`.
            PrePoolSource::Recovery => Ok(TxSource::Local),
            PrePoolSource::Remote(peer) => self
                .declared_cycles
                .filter(|_| self.ingress_peer == Some(peer) && self.payload_peer == Some(peer))
                .map(|cycles| TxSource::Remote { cycles, peer })
                .ok_or(PrePoolError::Repair("source attribution mismatch")),
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
    ready: [Arc<Notify>; WORKER_CLASSES.len()],
    maintenance_ready: Arc<Notify>,
    shutdown: CancellationToken,
    commit_serial: tokio::sync::Mutex<()>,
    max_entries: usize,
}

/// Sealed state moved out by an explicit clear or chain fallback.  Moving the
/// generation is an ownership optimization: its population is destroyed after
/// authority locks open.  It is deliberately not a panic-recovery protocol.
#[must_use = "retired generation must be dropped after authority guards are released"]
pub(crate) struct KernelDisposal {
    retired: Option<PrePoolGeneration>,
}

impl Drop for KernelDisposal {
    fn drop(&mut self) {
        drop(self.retired.take());
    }
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
            ready: WORKER_CLASSES.map(|_| Arc::new(Notify::new())),
            maintenance_ready: Arc::new(Notify::new()),
            shutdown,
            commit_serial: tokio::sync::Mutex::new(()),
            max_entries: total_entries,
        }
    }

    fn lock(&self) -> MutexGuard<'_, PrePoolKernel> {
        self.state
            .lock()
            .expect("pre-pool kernel mutex poisoned by an invariant failure")
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
            let ready =
                WORKER_CLASSES.map(|(lane, capability)| state.work_is_ready(lane, capability));
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
        match self.mutate_authoritative(apply) {
            Ok(value) => value,
            Err(error) => panic!("{context}: {error:?}"),
        }
    }

    /// Prepare an explicit clear/chain replacement off the live generation,
    /// then move it into place in one critical section.  Preparation failure
    /// leaves the live state unchanged; the retired population is returned so
    /// callers can destroy it after opening the TxPool guard.
    pub(crate) fn reset_for_chain<T>(
        &self,
        prepare: impl FnOnce(&mut PrePoolKernel) -> Result<T, PrePoolError>,
    ) -> Result<(T, KernelDisposal), PrePoolError> {
        let mut state = self.lock();
        let mut prepared = PrePoolKernel {
            generation: PrePoolGeneration::new(),
            limits: state.limits,
            next_version: state.next_version,
            next_arrival: state.next_arrival,
        };
        let value = prepare(&mut prepared)?;
        let retired = std::mem::replace(&mut state.generation, prepared.generation);
        state.next_version = prepared.next_version;
        state.next_arrival = prepared.next_arrival;
        drop(state);
        self.mutate_authoritative(|_| ());
        Ok((
            value,
            KernelDisposal {
                retired: Some(retired),
            },
        ))
    }

    pub(crate) fn subscribe(&self, lane: WorkLane, capability: WorkCapability) -> Arc<Notify> {
        let index = WORKER_CLASSES
            .iter()
            .position(|class| *class == (lane, capability))
            .expect("worker class belongs to the closed registry");
        let notify = Arc::clone(&self.ready[index]);
        if self.read(|state| state.work_is_ready(lane, capability)) {
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

    pub(crate) fn admit_transaction(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        lane: ResolveLane,
    ) -> Result<bool, PrePoolError> {
        let hash = tx.hash();
        let short_id = tx.proposal_short_id();
        self.mutate_authoritative(|kernel| {
            if let Some(existing) = kernel.entries.get(&hash).cloned() {
                return match source {
                    TxSource::Local => Err(PrePoolError::LocalMustRunDirect),
                    TxSource::Remote { .. } => Ok(false),
                    TxSource::Proposal => {
                        if existing.raw.tx.witness_hash() != tx.witness_hash() {
                            let raw = existing.raw.trusted_variant(tx, epoch);
                            let raw_bytes = raw.charge_bytes();
                            kernel.replace_raw_payload(&hash, raw, raw_bytes, lane)?;
                        } else {
                            kernel.promote_source(&hash)?;
                        }
                        Ok(false)
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
            Ok(true)
        })
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
        let work_lane = PrePoolKernel::lane_for_resolve(lane);
        let ready = self.subscribe(work_lane, WorkCapability::Any);
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

#[cfg(test)]
#[path = "../tests/pre_pool_runtime_seam.rs"]
mod test_seam;

pub(crate) fn pre_pool_reject(error: PrePoolError) -> Reject {
    assert!(
        error.is_transaction_rejection(),
        "a structural pre-pool defect must be contained, not converted into a transaction rejection: {error:?}"
    );
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
