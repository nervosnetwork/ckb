//! Production adapter for the single-authority pipeline coordinator.
//!
//! The coordinator itself is a synchronous transition engine. This adapter
//! owns its short critical-section mutex and the stage notifications used by
//! asynchronous workers. Payloads never travel through channels or secondary
//! queue maps: workers receive versioned leases directly from the coordinator.

use crate::component::pipeline_coordinator::{
    CoordinatorError, CoordinatorLimits, CoordinatorMetadataCost, CoordinatorReconciliationLimits,
    CoordinatorResidency, CoordinatorSource, CoordinatorVerifyOrdering, PipelineCoordinator,
    QueueKind, RawStage, RawWorkLease, TerminalRecord, TrustedSource,
};
use crate::constants::{
    MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE, MAX_PRE_CHECK_QUEUE_TX_SIZE, MAX_RBF_REPLACEMENT_CANDIDATES,
};
use crate::error::Reject;
use crate::resolved_tx::ResolvedTx;
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_types::core::{Cycle, TransactionView};
use ckb_verification::cache::Completed;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Raw payload retained for the complete pre-pool lifecycle. Source priority
/// and peer ownership live only in the coordinator; this object retains the
/// remote-only declared-cycle datum needed by script verification.
#[derive(Clone, Debug)]
pub(crate) struct PipelineRawTx {
    pub(crate) tx: TransactionView,
    pub(crate) declared_cycles: Option<Cycle>,
    pub(crate) admitted_epoch: u64,
}

impl PipelineRawTx {
    pub(crate) fn new(tx: TransactionView, source: TxSource, admitted_epoch: u64) -> Self {
        Self {
            tx,
            declared_cycles: source.cycles(),
            admitted_epoch,
        }
    }

    pub(crate) fn authoritative_source(
        &self,
        source: CoordinatorSource,
    ) -> Result<TxSource, Reject> {
        match source {
            CoordinatorSource::Local => Ok(TxSource::Local),
            CoordinatorSource::Proposal => Ok(TxSource::Proposal),
            CoordinatorSource::Remote(peer) => self
                .declared_cycles
                .map(|cycles| TxSource::Remote { cycles, peer })
                .ok_or_else(|| {
                    Reject::Internal(
                        "remote pipeline owner is missing declared-cycle metadata".to_string(),
                    )
                }),
        }
    }

    pub(crate) fn charge_bytes(&self) -> usize {
        self.tx.data().serialized_size_in_block()
    }
}

/// Verified phase payload. Publication metadata is retained until the
/// authoritative pool/coordinator handoff has produced a stable outcome.
#[derive(Clone, Debug)]
pub(crate) struct PipelineVerifiedTx {
    pub(crate) resolved: ResolvedTx,
    pub(crate) completed: Completed,
    pub(crate) verify_cache_hit: bool,
    pub(crate) started_at: Instant,
}

pub(crate) type ProductionCoordinator =
    PipelineCoordinator<PipelineRawTx, ResolvedTx, PipelineVerifiedTx>;

/// Actorless production owner. Every mutation is synchronous under `state`;
/// notifications are emitted only after the lock is released.
pub(crate) struct PipelineRuntime {
    state: Mutex<ProductionCoordinator>,
    ready: HashMap<QueueKind, Arc<Notify>>,
    maintenance_ready: Arc<Notify>,
    shutdown: CancellationToken,
    commit_serial: tokio::sync::Mutex<()>,
    max_entries: usize,
}

impl PipelineRuntime {
    pub(crate) fn new(
        config: &TxPoolConfig,
        consensus: &Consensus,
        shutdown: CancellationToken,
    ) -> Self {
        let verify_bytes = config.verify_queue_tx_size_budget();
        let global_bytes = MAX_PRE_CHECK_QUEUE_TX_SIZE
            .saturating_add(MAX_ORDERED_RESOLVE_QUEUE_TX_SIZE)
            .saturating_add(verify_bytes);
        // The metadata charge below makes the entry limit independently
        // enforceable even for tiny invalid transactions. Keep the count
        // proportional to the configured byte residency instead of adding a
        // second user-visible tuning knob during the migration.
        let max_entries = global_bytes.saturating_div(256).max(1);
        let max_dependencies = (consensus.max_block_bytes() as usize)
            .saturating_div(32)
            .saturating_add(1);
        let max_dependents = 4_096usize.min(max_entries).max(1);
        let peer_entries = max_entries.saturating_div(8).max(1);
        let peer_bytes = global_bytes.saturating_div(8).max(1);
        let verify_workers = config.max_tx_verify_workers.max(1);
        let pre_check_workers =
            verify_workers.min(std::thread::available_parallelism().map_or(4, |count| count.get()));
        let active_work = pre_check_workers
            .saturating_add(verify_workers)
            .saturating_add(2);
        let verify_ordering = match config.verify_ordering {
            VerifyOrdering::ArrivalTime => CoordinatorVerifyOrdering::ArrivalTime,
            VerifyOrdering::FeeRate => CoordinatorVerifyOrdering::FeeRate,
        };
        let edge_limit = global_bytes.saturating_div(64).max(1);
        let limits = CoordinatorLimits::new(
            CoordinatorResidency::new(max_entries, global_bytes),
            Some(CoordinatorResidency::new(peer_entries, peer_bytes)),
            max_dependencies,
            max_dependents,
            CoordinatorReconciliationLimits::new(
                config.max_ancestors_count,
                MAX_RBF_REPLACEMENT_CANDIDATES,
            ),
        )
        .with_conflict_limits(max_dependencies, MAX_RBF_REPLACEMENT_CANDIDATES, edge_limit)
        .with_metadata_cost(CoordinatorMetadataCost {
            entry_bytes: 256,
            dependency_edge_bytes: 64,
            lifecycle_ticket_bytes: 64,
            deadline_ticket_bytes: 64,
            conflict_edge_bytes: 64,
        })
        .with_active_limits(active_work, active_work.saturating_div(4).max(1))
        .with_verify_ordering(verify_ordering);

        Self {
            state: Mutex::new(PipelineCoordinator::new(limits)),
            ready: HashMap::from([
                (QueueKind::PreCheck, Arc::new(Notify::new())),
                (QueueKind::Resolve, Arc::new(Notify::new())),
                (QueueKind::Verify, Arc::new(Notify::new())),
                (QueueKind::Commit, Arc::new(Notify::new())),
            ]),
            maintenance_ready: Arc::new(Notify::new()),
            shutdown,
            commit_serial: tokio::sync::Mutex::new(()),
            max_entries,
        }
    }

    fn lock(&self) -> MutexGuard<'_, ProductionCoordinator> {
        self.state.lock().unwrap_or_else(|poisoned| {
            ckb_logger::error!(
                "tx-pool pipeline coordinator mutex was poisoned; retaining the undo-restored state"
            );
            poisoned.into_inner()
        })
    }

    pub(crate) fn read<T>(&self, inspect: impl FnOnce(&ProductionCoordinator) -> T) -> T {
        inspect(&self.lock())
    }

    /// Apply one complete coordinator transition. Queue notifications are
    /// level-triggered after unlock, so no worker waits while eligible work is
    /// resident and no `.await` occurs inside the state boundary.
    pub(crate) fn mutate<T>(&self, apply: impl FnOnce(&mut ProductionCoordinator) -> T) -> T {
        let (result, non_empty, maintenance_pending) = {
            let mut state = self.lock();
            let result = apply(&mut state);
            let non_empty = [
                QueueKind::PreCheck,
                QueueKind::Resolve,
                QueueKind::Verify,
                QueueKind::Commit,
            ]
            .map(|kind| (kind, state.queue_len(kind) != 0));
            let maintenance_pending =
                state.dependency_failure_len() != 0 || state.conflict_recheck_len() != 0;
            (result, non_empty, maintenance_pending)
        };
        for (kind, ready) in non_empty {
            if ready && let Some(notify) = self.ready.get(&kind) {
                notify.notify_one();
            }
        }
        if maintenance_pending {
            self.maintenance_ready.notify_one();
        }
        result
    }

    pub(crate) fn subscribe(&self, kind: QueueKind) -> Arc<Notify> {
        Arc::clone(
            self.ready
                .get(&kind)
                .expect("every production coordinator queue has a notifier"),
        )
    }

    pub(crate) fn subscribe_maintenance(&self) -> Arc<Notify> {
        Arc::clone(&self.maintenance_ready)
    }

    pub(crate) fn maintenance_pending(&self) -> bool {
        self.read(|state| state.dependency_failure_len() != 0 || state.conflict_recheck_len() != 0)
    }

    pub(crate) fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub(crate) fn queue_is_empty(&self, kind: QueueKind) -> bool {
        self.read(|state| state.queue_len(kind) == 0)
    }

    /// Admit one production transaction into the sole pre-pool owner. Normal
    /// entry and recovery share this operation so duplicate promotion,
    /// attribution and residency accounting cannot diverge.
    #[cfg(test)]
    pub(crate) fn admit_transaction(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: RawStage,
    ) -> Result<
        (
            bool,
            Vec<TerminalRecord<PipelineRawTx, ResolvedTx, PipelineVerifiedTx>>,
        ),
        CoordinatorError,
    > {
        self.admit_transaction_journaled(tx, source, epoch, stage, |_| {})
    }

    pub(crate) fn admit_transaction_journaled(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: RawStage,
        journal: impl FnOnce(&[TerminalRecord<PipelineRawTx, ResolvedTx, PipelineVerifiedTx>]),
    ) -> Result<
        (
            bool,
            Vec<TerminalRecord<PipelineRawTx, ResolvedTx, PipelineVerifiedTx>>,
        ),
        CoordinatorError,
    > {
        let hash = tx.hash();
        let short_id = tx.proposal_short_id();
        let dependencies = tx.unique_parents();
        let raw = PipelineRawTx::new(tx, source, epoch);
        let charge_bytes = raw.charge_bytes();
        let source = coordinator_source(source);
        let expires_at = matches!(source, CoordinatorSource::Remote(_)).then(|| {
            ckb_systemtime::unix_time()
                .as_secs()
                .saturating_add(100 * ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL)
        });

        self.mutate(|coordinator| {
            if coordinator.contains_hash(&hash) {
                let promotion = match source {
                    CoordinatorSource::Proposal => Some(TrustedSource::Proposal),
                    CoordinatorSource::Local => Some(TrustedSource::Local),
                    CoordinatorSource::Remote(_) => None,
                };
                if let Some(promotion) = promotion {
                    coordinator.promote_source(&hash, promotion)?;
                }
                let terminal = Vec::new();
                journal(&terminal);
                return Ok((false, terminal));
            }
            let result = coordinator
                .admit_raw_sourced(
                    hash,
                    short_id,
                    raw,
                    stage,
                    source,
                    expires_at,
                    charge_bytes,
                    dependencies,
                )
                .map(|(_, terminal)| (true, terminal));
            if let Ok((_, terminal)) = &result {
                journal(terminal);
            }
            result
        })
    }

    pub(crate) fn checkout_raw(
        &self,
        stage: RawStage,
    ) -> Result<Option<RawWorkLease<PipelineRawTx>>, CoordinatorError> {
        self.mutate(|state| state.checkout_raw(stage))
    }

    pub(crate) async fn wait_raw(
        &self,
        stage: RawStage,
    ) -> Result<Option<RawWorkLease<PipelineRawTx>>, CoordinatorError> {
        let kind = match stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let ready = self.subscribe(kind);
        loop {
            // Register before checking the queue so an admission between the
            // check and `.await` leaves a permit for this waiter.
            let notified = ready.notified();
            if let Some(lease) = self.checkout_raw(stage)? {
                return Ok(Some(lease));
            }
            tokio::select! {
                _ = notified => {}
                _ = self.shutdown.cancelled() => return Ok(None),
            }
        }
    }

    pub(crate) async fn lock_commit_driver(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.commit_serial.lock().await
    }
}

pub(crate) fn coordinator_source(source: TxSource) -> CoordinatorSource {
    match source {
        TxSource::Remote { peer, .. } => CoordinatorSource::Remote(peer),
        TxSource::Local => CoordinatorSource::Local,
        TxSource::Proposal => CoordinatorSource::Proposal,
    }
}

/// Conservative residency charge for the complete raw + resolved bundle.
/// `CellMeta::mem_cell_data` is counted because dep-group/code cells can be
/// much larger than the transaction that references them.
pub(crate) fn resolved_charge_bytes(resolved: &ResolvedTx) -> Result<usize, CoordinatorError> {
    let mut bytes = resolved
        .tx_size
        .checked_mul(2)
        .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
    for cell in resolved
        .rtx
        .resolved_inputs
        .iter()
        .chain(resolved.rtx.resolved_cell_deps.iter())
        .chain(resolved.rtx.resolved_dep_groups.iter())
    {
        bytes = bytes
            .checked_add(std::mem::size_of_val(cell))
            .and_then(|value| {
                value.checked_add(cell.mem_cell_data.as_ref().map_or(0, |data| data.len()))
            })
            .ok_or(CoordinatorError::ResidencyChargeOverflow)?;
    }
    Ok(bytes)
}

pub(crate) fn coordinator_reject(error: CoordinatorError) -> Reject {
    use CoordinatorError::*;
    match error {
        GlobalBudgetExceeded
        | PeerBudgetExceeded(_)
        | DependencyLimitExceeded
        | DependencyAncestorLimitExceeded
        | ParentFanoutLimitExceeded(_)
        | ConflictInputLimitExceeded
        | ConflictCandidateLimitExceeded(_)
        | ConflictEdgeLimitExceeded
        | CapacityEvictionLimitExceeded
        | ActiveWorkLimitExceeded
        | PeerActiveWorkLimitExceeded(_) => Reject::Full(format!(
            "tx-pool pipeline coordinator capacity rejected transaction: {error:?}"
        )),
        UnderReplacementFee {
            required, actual, ..
        } => Reject::RBFRejected(format!(
            "replacement fee {actual} is below required fee {required}"
        )),
        UnderFeeRate {
            required_per_kb, ..
        } => Reject::RBFRejected(format!(
            "replacement fee rate is below {required_per_kb} shannons/KB"
        )),
        other => Reject::Internal(format!(
            "tx-pool pipeline coordinator transition failed: {other:?}"
        )),
    }
}
