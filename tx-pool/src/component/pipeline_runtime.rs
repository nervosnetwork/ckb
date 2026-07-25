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
use crate::constants::MAX_POOL_MUTATION_CANDIDATES;
use crate::error::Reject;
use crate::resolved_tx::{PoolCandidate, ResolvedTx};
use crate::tx_source::TxSource;
use ckb_app_config::{TxPoolConfig, VerifyOrdering};
use ckb_chain_spec::consensus::Consensus;
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use ckb_verification::cache::Completed;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU8, Ordering},
};
use std::time::Instant;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Raw payload retained for the complete pre-pool lifecycle. Source priority
/// lives in the coordinator. Immutable ingress attribution stays with the raw
/// payload so a later Local/Proposal promotion cannot lose the peer whose
/// relay filter still requires exactly one terminal settlement.
#[derive(Clone, Debug)]
pub(crate) struct PipelineRawTx {
    pub(crate) tx: TransactionView,
    pub(crate) declared_cycles: Option<Cycle>,
    /// Immutable owner of the relayer's outstanding request/filter. It must
    /// receive exactly one terminal settlement even if a trusted witness
    /// later replaces the payload.
    ingress_peer: Option<PeerIndex>,
    /// Peer responsible for the witness currently stored in `tx`. Unlike
    /// `ingress_peer`, this is cleared when Local/Proposal installs a
    /// different witness so malformed-payload blame cannot hit the old peer.
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

    /// Replace only the witness-bearing transaction view while retaining the
    /// immutable peer attribution of the original network ingress. Raw hash
    /// equality is checked by the coordinator adapter before this value is
    /// installed, so inputs, outputs and dependency hashes are unchanged.
    fn trusted_variant(&self, tx: TransactionView, admitted_epoch: u64) -> Self {
        let same_witness = self.tx.witness_hash() == tx.witness_hash();
        Self {
            tx,
            declared_cycles: self.declared_cycles,
            ingress_peer: self.ingress_peer,
            payload_peer: same_witness.then_some(self.payload_peer).flatten(),
            admitted_epoch,
        }
    }

    pub(crate) fn authoritative_source(
        &self,
        source: CoordinatorSource,
    ) -> Result<TxSource, CoordinatorError> {
        match source {
            CoordinatorSource::Local => Ok(TxSource::Local),
            CoordinatorSource::Proposal => Ok(TxSource::Proposal),
            CoordinatorSource::Remote(peer) => self
                .declared_cycles
                .filter(|_| self.ingress_peer == Some(peer) && self.payload_peer == Some(peer))
                .map(|cycles| TxSource::Remote { cycles, peer })
                .ok_or(CoordinatorError::SourceAttributionMismatch),
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
    /// Deliberately snapshot-free after verification; see `PoolCandidate`.
    pub(crate) candidate: PoolCandidate,
    pub(crate) completed: Completed,
    pub(crate) verify_cache_hit: bool,
    pub(crate) started_at: Instant,
}

pub(crate) type ProductionCoordinator =
    PipelineCoordinator<PipelineRawTx, ResolvedTx, PipelineVerifiedTx>;

/// Failure severity is monotonic. A coordinator-only failure invalidates
/// transient pipeline ownership and stops this service generation, but the
/// accepted PoolMap remains a safe persistence source. A panic crossing the
/// TxPool/coordinator boundary may have interrupted an authoritative pool
/// mutation and therefore forbids persistence of that generation.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FailureDomain {
    Healthy = 0,
    Pipeline = 1,
    Authoritative = 2,
}

// Conservative allocator-independent charges for coordinator-owned indexes.
// One lifecycle ticket is retained in both the live set and an owner heap
// (with bounded stale heap slack); dependency/conflict relations likewise own
// both entry-local and reverse-index records. These weights intentionally
// cover those materialized copies instead of counting only the packed hash.
const COORDINATOR_ENTRY_INDEX_BYTES: usize = 1_024;
const COORDINATOR_DEPENDENCY_EDGE_BYTES: usize = 256;
const COORDINATOR_LIFECYCLE_TICKET_BYTES: usize = 512;
const COORDINATOR_DEADLINE_TICKET_BYTES: usize = 256;
const COORDINATOR_CONFLICT_INPUT_BYTES: usize = 512;

/// Actorless production owner. Every mutation is synchronous under `state`;
/// notifications are emitted only after the lock is released.
pub(crate) struct PipelineRuntime {
    state: Mutex<ProductionCoordinator>,
    /// A panic anywhere inside the state boundary may occur after a
    /// coordinator transition but before its reserved effects are journaled.
    /// Such a state must never be recovered from `Mutex` poisoning and put
    /// back into service.
    failure_domain: AtomicU8,
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
        // The metadata charge below makes the entry limit independently
        // enforceable even for tiny invalid transactions. Keep the count
        // proportional to the configured byte residency instead of adding a
        // second user-visible tuning knob during the migration.
        let minimum_entry_metadata =
            COORDINATOR_ENTRY_INDEX_BYTES.saturating_add(COORDINATOR_LIFECYCLE_TICKET_BYTES);
        let global_bytes = config.tx_pipeline_resident_size_budget();
        assert!(
            global_bytes >= minimum_entry_metadata,
            "tx_pool.max_tx_pipeline_resident_size must be at least {minimum_entry_metadata} bytes"
        );
        let max_entries = global_bytes.saturating_div(minimum_entry_metadata);
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
        let edge_limit = global_bytes
            .saturating_div(COORDINATOR_CONFLICT_INPUT_BYTES)
            .max(1);
        let limits = CoordinatorLimits::new(
            CoordinatorResidency::new(max_entries, global_bytes),
            Some(CoordinatorResidency::new(peer_entries, peer_bytes)),
            max_dependencies,
            max_dependents,
            CoordinatorReconciliationLimits::new(
                config.max_ancestors_count,
                MAX_POOL_MUTATION_CANDIDATES,
            ),
        )
        .with_conflict_limits(max_dependencies, MAX_POOL_MUTATION_CANDIDATES, edge_limit)
        .with_metadata_cost(CoordinatorMetadataCost {
            entry_bytes: COORDINATOR_ENTRY_INDEX_BYTES,
            dependency_edge_bytes: COORDINATOR_DEPENDENCY_EDGE_BYTES,
            lifecycle_ticket_bytes: COORDINATOR_LIFECYCLE_TICKET_BYTES,
            deadline_ticket_bytes: COORDINATOR_DEADLINE_TICKET_BYTES,
            conflict_edge_bytes: COORDINATOR_CONFLICT_INPUT_BYTES,
        })
        .with_active_limits(active_work, active_work.saturating_div(4).max(1))
        .with_verify_ordering(verify_ordering);

        Self {
            state: Mutex::new(PipelineCoordinator::new(limits)),
            failure_domain: AtomicU8::new(FailureDomain::Healthy as u8),
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
        if self.failure_domain.load(Ordering::Acquire) != FailureDomain::Healthy as u8 {
            panic!("tx-pool pipeline coordinator is in fail-closed state");
        }
        match self.state.lock() {
            Ok(state) => state,
            Err(_) => {
                self.fail_closed(FailureDomain::Pipeline, "coordinator mutex was poisoned");
                panic!("tx-pool pipeline coordinator mutex was poisoned");
            }
        }
    }

    fn fail_closed(&self, domain: FailureDomain, reason: &str) {
        let previous = self
            .failure_domain
            .fetch_max(domain as u8, Ordering::AcqRel);
        if previous == FailureDomain::Healthy as u8 {
            ckb_logger::error!(
                "tx-pool pipeline entered {:?} fail-closed state: {}; cancelling tx-pool service",
                domain,
                reason,
            );
        } else if previous < domain as u8 {
            ckb_logger::error!(
                "tx-pool failure escalated to {:?}: {}; accepted-pool persistence is disabled",
                domain,
                reason,
            );
        }
        // Production passes the tx-pool service token, not a stage-local
        // child. This stops every worker and the dispatcher, and the shutdown
        // path skips persistence while `failed` is set.
        self.shutdown.cancel();
    }

    /// Stop the complete tx-pool service when an authoritative transition can
    /// no longer establish a valid next owner. Continuing after this point
    /// would expose a partially live coordinator (for example a permanently
    /// `Committing` entry) and make shutdown persistence unsafe.
    pub(crate) fn fail_stop(&self, context: &'static str, error: &impl std::fmt::Debug) -> ! {
        let reason = format!("{context}: {error:?}");
        self.fail_closed(FailureDomain::Pipeline, &reason);
        panic!("tx-pool pipeline fail-stop: {reason}");
    }

    fn fail_authoritative(&self, context: &'static str, error: &impl std::fmt::Debug) -> ! {
        let reason = format!("{context}: {error:?}");
        self.fail_closed(FailureDomain::Authoritative, &reason);
        panic!("tx-pool authoritative fail-stop: {reason}");
    }

    /// Guard a synchronous mutation that crosses the authoritative TxPool /
    /// coordinator boundary. Coordinator-only transitions are already guarded
    /// by [`Self::mutate`], but a panic in PoolMap/RBF/effect journaling would
    /// otherwise unwind a worker after it checked out `Committing` ownership
    /// and leave the healthy process unable to settle that lease.
    pub(crate) fn guard_authoritative_mutation<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce() -> T,
    ) -> T {
        match catch_unwind(AssertUnwindSafe(apply)) {
            Ok(result) => result,
            Err(payload) => {
                let message = crate::util::panic_payload_to_string(payload.as_ref());
                self.fail_authoritative(context, &message)
            }
        }
    }

    /// Guard publication that is ordered under an authoritative lock but
    /// cannot invalidate the already-stable PoolMap. A panic here loses an
    /// effect and must stop this service generation, yet the accepted pool
    /// remains a valid persistence source; classifying it as Authoritative
    /// would discard an exact recovery point.
    pub(crate) fn guard_stable_effect_journal<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce() -> T,
    ) -> T {
        match catch_unwind(AssertUnwindSafe(apply)) {
            Ok(result) => result,
            Err(payload) => {
                self.fail_closed(FailureDomain::Pipeline, context);
                resume_unwind(payload)
            }
        }
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.failure_domain.load(Ordering::Acquire) != FailureDomain::Healthy as u8
    }

    /// The accepted pool can be persisted after a coordinator-only failure.
    /// Only a failure that crossed an authoritative TxPool mutation makes the
    /// in-memory pool an unprovable recovery point.
    pub(crate) fn pool_persistence_safe(&self) -> bool {
        self.failure_domain.load(Ordering::Acquire) < FailureDomain::Authoritative as u8
    }

    /// Resolve the transaction-facing source from the coordinator owner and
    /// immutable ingress metadata. A mismatch means lifecycle attribution is
    /// corrupt: rejecting just this transaction could release the wrong peer
    /// filter or apply the wrong trust policy, so the only safe outcome is to
    /// stop the complete service.
    pub(crate) fn require_authoritative_source(
        &self,
        raw: &PipelineRawTx,
        source: CoordinatorSource,
    ) -> TxSource {
        match raw.authoritative_source(source) {
            Ok(source) => source,
            Err(error) => self.fail_stop("pipeline source attribution mismatch", &error),
        }
    }

    pub(crate) fn read<T>(&self, inspect: impl FnOnce(&ProductionCoordinator) -> T) -> T {
        match catch_unwind(AssertUnwindSafe(|| inspect(&self.lock()))) {
            Ok(result) => result,
            Err(payload) => {
                self.fail_closed(
                    FailureDomain::Pipeline,
                    "panic while inspecting coordinator state",
                );
                resume_unwind(payload)
            }
        }
    }

    /// Apply one complete coordinator transition. Queue notifications are
    /// level-triggered after unlock, so no worker waits while eligible work is
    /// resident and no `.await` occurs inside the state boundary.
    pub(crate) fn mutate<T>(&self, apply: impl FnOnce(&mut ProductionCoordinator) -> T) -> T {
        let transition = catch_unwind(AssertUnwindSafe(|| {
            let mut state = self.lock();
            let result = apply(&mut state);
            let non_empty = [
                QueueKind::PreCheck,
                QueueKind::Resolve,
                QueueKind::Verify,
                QueueKind::Commit,
            ]
            .map(|kind| (kind, state.queue_len(kind) != 0));
            let maintenance_pending = state.dependency_failure_len() != 0;
            (result, non_empty, maintenance_pending)
        }));
        let (result, non_empty, maintenance_pending) = match transition {
            Ok(result) => result,
            Err(payload) => {
                self.fail_closed(
                    FailureDomain::Pipeline,
                    "panic inside coordinator state/effect transaction",
                );
                resume_unwind(payload)
            }
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

    /// Run a transition whose failure cannot be represented as a healthy
    /// lifecycle state. Admission and speculative completion deliberately do
    /// not use this API because their capacity/policy errors are ordinary
    /// transaction outcomes; commit settlement, checkout, maintenance and
    /// clear do use it because they must always make progress or stop service.
    pub(crate) fn mutate_required<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce(&mut ProductionCoordinator) -> Result<T, CoordinatorError>,
    ) -> T {
        match self.mutate(apply) {
            Ok(value) => value,
            Err(error) => self.fail_stop(context, &error),
        }
    }

    /// Settle work held by a versioned worker lease. A stale lease has already
    /// lost ownership and returns `None`; an error for the still-current owner
    /// is a fail-stop condition rather than a log-and-leak condition.
    pub(crate) fn mutate_lease<T>(
        &self,
        context: &'static str,
        apply: impl FnOnce(&mut ProductionCoordinator) -> Result<T, CoordinatorError>,
    ) -> Option<T> {
        match self.mutate(apply) {
            Ok(value) => Some(value),
            Err(error) if error.is_stale_lease() => None,
            Err(error) => self.fail_stop(context, &error),
        }
    }

    /// Convert only an explicitly enumerated, rollback-safe policy error into
    /// a public transaction rejection. An unexpected coordinator error means
    /// the production adapter can no longer prove ownership/index integrity;
    /// continuing would turn an invariant failure into silent state drift.
    pub(crate) fn reject_or_fail(&self, context: &'static str, error: CoordinatorError) -> Reject {
        if error.is_transaction_rejection() {
            coordinator_reject(error)
        } else {
            self.fail_stop(context, &error)
        }
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

    /// Wake the shared bounded maintenance worker for work whose authority
    /// lives outside the coordinator (currently ConflictCache ownership
    /// transfer). The underlying queue is level-triggered; Notify is only a
    /// latency hint and the expiry tick provides a defensive retry.
    pub(crate) fn request_maintenance(&self) {
        self.maintenance_ready.notify_one();
    }

    pub(crate) fn maintenance_pending(&self) -> bool {
        self.read(|state| state.dependency_failure_len() != 0)
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
    ) -> Result<(bool, Vec<TerminalRecord<PipelineRawTx>>), CoordinatorError> {
        self.admit_transaction_journaled(tx, source, epoch, stage, |_| {})
    }

    pub(crate) fn admit_transaction_journaled(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: RawStage,
        journal: impl FnOnce(&[TerminalRecord<PipelineRawTx>]),
    ) -> Result<(bool, Vec<TerminalRecord<PipelineRawTx>>), CoordinatorError> {
        // Use the caller's view only for transient lookup. A new or
        // witness-replacing owner is compacted after deduplication, so a
        // duplicate flood pays no allocation/copy tax and no retained key can
        // keep the caller's enclosing block/network buffer alive.
        let lookup_hash = tx.hash();
        let coordinator_source = coordinator_source(source);
        let expires_at = matches!(coordinator_source, CoordinatorSource::Remote(_)).then(|| {
            ckb_systemtime::unix_time()
                .as_secs()
                .saturating_add(100 * ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL)
        });

        self.mutate(|coordinator| {
            if coordinator.contains_hash(&lookup_hash) {
                let promotion = match coordinator_source {
                    CoordinatorSource::Proposal => Some(TrustedSource::Proposal),
                    CoordinatorSource::Local => Some(TrustedSource::Local),
                    CoordinatorSource::Remote(_) => None,
                };
                if let Some(promotion) = promotion {
                    let existing_source = coordinator
                        .view(&lookup_hash)
                        .map(|view| view.source)
                        .ok_or_else(|| CoordinatorError::Missing(lookup_hash.clone()))?;
                    // Admission is a source-preference merge, not an order to
                    // overwrite the current authority. A Local historical
                    // recovery can legitimately race a Proposal owner of the
                    // same raw hash; treating that weaker duplicate as a
                    // forbidden coordinator downgrade turns normal ownership
                    // convergence into a service-wide fail-stop. Preserve the
                    // stronger owner (and its witness) without reticketing it.
                    if coordinator_source.trust() < existing_source.trust() {
                        let terminal = Vec::new();
                        journal(&terminal);
                        return Ok((false, terminal));
                    }
                    let existing = coordinator
                        .raw_by_hash(&lookup_hash)
                        .ok_or_else(|| CoordinatorError::Missing(lookup_hash.clone()))?;
                    let location = coordinator
                        .view(&lookup_hash)
                        .map(|view| view.location)
                        .ok_or_else(|| CoordinatorError::Missing(lookup_hash.clone()))?;
                    let committing = matches!(
                        &location,
                        crate::component::pipeline_coordinator::CoordinatorLocation::Committing
                    );
                    if existing.tx.witness_hash() != tx.witness_hash() && !committing {
                        let incoming = PipelineRawTx::new(tx, source, epoch);
                        let replacement = existing.trusted_variant(incoming.tx, epoch);
                        let replacement_charge = replacement.charge_bytes();
                        let (_, terminal) = coordinator.replace_raw_payload(
                            &lookup_hash,
                            replacement,
                            replacement_charge,
                            promotion,
                            stage,
                        )?;
                        journal(&terminal);
                        return Ok((false, terminal));
                    } else {
                        // A committing payload has already passed verification;
                        // replacing it would violate the pool handoff lease.
                        // Source promotion is sufficient in that state. An
                        // equivalent witness is reticketed by the coordinator
                        // itself if it was waiting for parents.
                        coordinator.promote_source(&lookup_hash, promotion)?;
                    }
                }
                let terminal = Vec::new();
                journal(&terminal);
                return Ok((false, terminal));
            }
            let raw = PipelineRawTx::new(tx, source, epoch);
            let hash = raw.tx.hash();
            debug_assert_eq!(hash, lookup_hash);
            let short_id = raw.tx.proposal_short_id();
            let dependencies = raw.tx.unique_parents();
            let charge_bytes = raw.charge_bytes();
            let result = coordinator
                .admit_raw_sourced(
                    hash,
                    short_id,
                    raw,
                    stage,
                    coordinator_source,
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

    pub(crate) fn checkout_raw(&self, stage: RawStage) -> Option<RawWorkLease<PipelineRawTx>> {
        self.mutate_required("raw checkout failed", |state| state.checkout_raw(stage))
    }

    pub(crate) async fn wait_raw(&self, stage: RawStage) -> Option<RawWorkLease<PipelineRawTx>> {
        let kind = match stage {
            RawStage::PreCheck => QueueKind::PreCheck,
            RawStage::Resolve => QueueKind::Resolve,
        };
        let ready = self.subscribe(kind);
        while !self.shutdown.is_cancelled() {
            // Cancellation is the dispatch barrier. In particular, do not
            // keep checking out a non-empty queue after shutdown: checkout
            // republishes the level-triggered queue notification and can
            // otherwise keep a runtime worker alive indefinitely.
            // Register before checking the queue so an admission between the
            // check and `.await` leaves a permit for this waiter.
            let notified = ready.notified();
            if let Some(lease) = self.checkout_raw(stage) {
                return Some(lease);
            }
            tokio::select! {
                _ = notified => {}
                _ = self.shutdown.cancelled() => return None,
            }
        }
        None
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
    Ok(resolved.resident_size)
}

pub(crate) fn candidate_charge_bytes(candidate: &PoolCandidate) -> Result<usize, CoordinatorError> {
    Ok(candidate.resident_size)
}

pub(crate) fn coordinator_reject(error: CoordinatorError) -> Reject {
    use CoordinatorError::*;
    if error.is_capacity_rejection() {
        return Reject::Full(format!(
            "tx-pool pipeline coordinator capacity rejected transaction: {error:?}"
        ));
    }
    match error {
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
