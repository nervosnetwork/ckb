//! Tx-pool-only resolution against one immutable chain/Accepted-state cut.
//!
//! The consensus resolver remains the single implementation of CKB cell and
//! dep-group semantics. This module contributes only the role-specific
//! Accepted overlay and a complete, bounded missing-frontier pass needed by
//! transaction relay. The overlay is an owned read receipt for one checked-out
//! capability; it is never a second membership authority or a persistent
//! cache.

use super::{
    chain::{
        CellLocationReceipt, CellLocationReceiptError, DirectAdmissionWork, TimeContextReceipt,
        VerificationContextReceipt,
    },
    ingress::{DirectCommand, DirectTransaction},
    plan::TxPoolAuthority,
    rejection::{DirectTransactionRejection, duplicate_inputs_reject},
    resources::AcceptedCost,
    state::{
        AsyncProcessStart, CandidateMetrics, ChainViewId, DependencyCut, DependencyKey,
        EntryVersion, InputEvidenceDisposition, InputEvidenceError, OwnedTx, PayloadPolicy,
        RawTxHash, ResolvedPayload,
    },
    validation::{proposal_status, verification_environment},
    work::{
        ComputeSettlement, ContinuousResolution, ContinuousResolveWork, ContinuousVerifyWork,
        ReceiptFailure, ResolutionEvidence, ResolutionReceiptError, ResolveWork,
        SnapshotBoundVerifyWork, VerifyWork,
    },
};
use crate::{
    component::entry::resolved_transaction_charge_bytes,
    error::Reject,
    util::{
        TxPoolVerificationBudget, TxPoolVerificationOutcome, check_tx_fee_with_min_fee_rate,
        compact_packed, verify_rtx,
    },
};
use ckb_script::{ChunkCommand, TxVerifyEnv};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        Capacity, DepType, FeeRate, TransactionView,
        cell::{
            CellMetaBuilder, CellProvider, CellStatus, HeaderChecker, OverlayCellProvider,
            ResolvedDep, ResolvedTransaction, SYSTEM_CELL, resolve_transaction_with_cell_providers,
        },
        error::OutPointError,
    },
    packed::{OutPoint, OutPointVec},
    prelude::{Entity, Unpack},
};
use ckb_verification::cache::{
    ScriptVerificationProof, ScriptVerificationRules, TxVerificationCache, TxVerificationCacheKey,
};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
enum CellRole {
    Input,
    ProducerOnly,
}

/// Proof that a direct resolved payload passed the same tx-pool resolver,
/// footprint, fee, and residency checks as retained work without acquiring a
/// resident owner.
pub(super) struct DirectResolutionSeal(());

/// Proof that direct script verification used the snapshot-bound tx-pool
/// rules and cache identity. It is intentionally distinct from block
/// verification evidence.
pub(super) struct DirectVerificationSeal(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum VerificationTimePolicyError {
    ZeroCycleRate,
    InvalidDurationRange,
}

/// Immutable node-local classification of one already-owned verification
/// lease. Consensus cycles are only an untrusted signal selecting an equal or
/// shorter duration; no transaction can extend the unconditional hard cap.
#[derive(Clone, Copy, Debug)]
pub(in crate::authority) struct VerificationTimePolicy {
    minimum: Duration,
    cycles_per_millisecond: NonZeroU64,
    hard_maximum: Duration,
}

impl VerificationTimePolicy {
    pub(in crate::authority) fn from_runtime(
        minimum_millis: u32,
        cycles_per_millisecond: u64,
        hard_maximum_millis: u32,
    ) -> Result<Self, VerificationTimePolicyError> {
        let cycles_per_millisecond = NonZeroU64::new(cycles_per_millisecond)
            .ok_or(VerificationTimePolicyError::ZeroCycleRate)?;
        if minimum_millis == 0 || minimum_millis > hard_maximum_millis {
            return Err(VerificationTimePolicyError::InvalidDurationRange);
        }
        Ok(Self {
            minimum: Duration::from_millis(u64::from(minimum_millis)),
            cycles_per_millisecond,
            hard_maximum: Duration::from_millis(u64::from(hard_maximum_millis)),
        })
    }

    pub(in crate::authority) fn hard_maximum(self) -> Duration {
        self.hard_maximum
    }

    pub(in crate::authority) fn deadline(
        self,
        started_at: Instant,
        hard_deadline: Instant,
        payload_policy: PayloadPolicy,
    ) -> Instant {
        let duration = match payload_policy {
            PayloadPolicy::RemoteDeclaredCycles(limit) => {
                let milliseconds = limit.declared().div_ceil(self.cycles_per_millisecond.get());
                Duration::from_millis(milliseconds)
                    .max(self.minimum)
                    .min(self.hard_maximum)
            }
            PayloadPolicy::Trusted => self.hard_maximum,
        };
        started_at
            .checked_add(duration)
            .map_or(hard_deadline, |deadline| deadline.min(hard_deadline))
    }
}

/// Production capability for constructing resolution evidence. Its private
/// field keeps the evidence producer inside this resolver; test fixtures use
/// the separately cfg-gated constructor in `work`.
pub(super) struct ResolutionEvidenceSeal(());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DirectComputationError {
    StaleView,
    ResourceUnavailable,
    InvalidEvidence,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CellQuery {
    out_point: OutPoint,
    producer: RawTxHash,
    role: CellRole,
}

impl CellQuery {
    fn new(out_point: OutPoint, role: CellRole) -> Self {
        Self {
            producer: RawTxHash(compact_packed(&out_point.tx_hash())),
            out_point: compact_packed(&out_point),
            role,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedOwnerObservation {
    key: RawTxHash,
    version: EntryVersion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AcceptedProducerObservation {
    owner: AcceptedOwnerObservation,
    tx: Arc<TransactionView>,
}

/// Exact transaction-bounded Accepted read receipt used by owner-free work.
///
/// Absence is represented by a query with no matching producer/spender row;
/// presence retains the exact owner incarnation. The maps are populated only
/// after all query-bounded capacity is reserved outside the authority guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AcceptedOverlay {
    producers: HashMap<RawTxHash, AcceptedProducerObservation>,
    spent_inputs: HashMap<OutPoint, AcceptedOwnerObservation>,
    queries: HashSet<CellQuery>,
}

impl AcceptedOverlay {
    fn prepare(tx: &TransactionView, max_edges: usize) -> Result<Self, ResolutionExecutionKind> {
        let direct_edges = tx
            .inputs()
            .len()
            .checked_add(tx.cell_deps().len())
            .and_then(|count| count.checked_add(tx.header_deps().len()))
            .ok_or(ResolutionExecutionKind::ResourceUnavailable)?;
        if direct_edges > max_edges {
            return Err(ResolutionExecutionKind::ComputeBudget);
        }

        let mut overlay = Self {
            producers: HashMap::new(),
            spent_inputs: HashMap::new(),
            queries: HashSet::new(),
        };
        overlay
            .producers
            .try_reserve(direct_edges)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        overlay
            .spent_inputs
            .try_reserve(tx.inputs().len())
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        overlay
            .queries
            .try_reserve(direct_edges)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        overlay.queries.extend(
            tx.input_pts_iter()
                .map(|out_point| CellQuery::new(out_point, CellRole::Input)),
        );
        overlay.queries.extend(
            tx.cell_deps_iter()
                .map(|cell_dep| CellQuery::new(cell_dep.out_point(), CellRole::ProducerOnly)),
        );

        Ok(overlay)
    }

    pub(super) fn prepare_resolved(
        payload: &ResolvedPayload,
    ) -> Result<Self, CellLocationReceiptError> {
        let resolved = payload.resolved_transaction();
        let total_cells = resolved
            .resolved_inputs
            .len()
            .checked_add(resolved.resolved_cell_deps.len())
            .and_then(|count| count.checked_add(resolved.resolved_dep_groups.len()))
            .ok_or(CellLocationReceiptError::Arithmetic)?;
        let mut overlay = Self {
            producers: HashMap::new(),
            spent_inputs: HashMap::new(),
            queries: HashSet::new(),
        };
        overlay
            .producers
            .try_reserve(total_cells)
            .map_err(|_| CellLocationReceiptError::Allocation)?;
        overlay
            .queries
            .try_reserve(total_cells)
            .map_err(|_| CellLocationReceiptError::Allocation)?;
        overlay.queries.extend(
            resolved
                .resolved_inputs
                .iter()
                .map(|cell| CellQuery::new(cell.out_point.clone(), CellRole::ProducerOnly)),
        );
        overlay.queries.extend(
            resolved
                .resolved_cell_deps
                .iter()
                .chain(&resolved.resolved_dep_groups)
                .map(|cell| CellQuery::new(cell.out_point.clone(), CellRole::ProducerOnly)),
        );
        Ok(overlay)
    }

    pub(super) fn populate(&mut self, authority: &TxPoolAuthority) {
        let Self {
            producers,
            spent_inputs,
            queries,
        } = self;
        for query in queries.iter() {
            Self::capture_cell(producers, spent_inputs, authority, query);
        }
    }

    fn capture(
        authority: &TxPoolAuthority,
        tx: &TransactionView,
        max_edges: usize,
    ) -> Result<Self, ResolutionExecutionKind> {
        let mut overlay = Self::prepare(tx, max_edges)?;
        overlay.populate(authority);
        Ok(overlay)
    }

    fn reserve_enrichment(&mut self, missing_count: usize) -> Result<(), ResolutionExecutionKind> {
        self.producers
            .try_reserve(missing_count)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        self.spent_inputs
            .try_reserve(missing_count)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        self.queries
            .try_reserve(missing_count)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        Ok(())
    }

    /// Retained work already owns dependency invalidation. Its enrichment can
    /// extend the bounded overlay in place after capacity is reserved.
    fn observe_enrichment(&mut self, authority: &TxPoolAuthority, missing: &[CellQuery]) -> bool {
        let before_producers = self.producers.len();
        let before_spends = self.spent_inputs.len();
        for cell in missing {
            self.queries.insert(cell.clone());
            Self::capture_cell(&mut self.producers, &mut self.spent_inputs, authority, cell);
        }
        self.producers.len() != before_producers || self.spent_inputs.len() != before_spends
    }

    fn prepare_refresh(
        &self,
        missing: &[CellQuery],
        max_edges: usize,
    ) -> Result<Self, ResolutionExecutionKind> {
        let upper = self
            .queries
            .len()
            .checked_add(missing.len())
            .ok_or(ResolutionExecutionKind::ResourceUnavailable)?;
        let mut refreshed = Self {
            producers: HashMap::new(),
            spent_inputs: HashMap::new(),
            queries: HashSet::new(),
        };
        refreshed
            .queries
            .try_reserve(upper)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        refreshed.queries.extend(self.queries.iter().cloned());
        refreshed.queries.extend(missing.iter().cloned());
        if refreshed.queries.len() > max_edges {
            return Err(ResolutionExecutionKind::ComputeBudget);
        }
        refreshed
            .producers
            .try_reserve(refreshed.queries.len())
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        let input_count = refreshed
            .queries
            .iter()
            .filter(|query| query.role == CellRole::Input)
            .count();
        refreshed
            .spent_inputs
            .try_reserve(input_count)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        Ok(refreshed)
    }

    pub(super) fn is_current(&self, authority: &TxPoolAuthority) -> bool {
        self.queries.iter().all(|query| {
            self.producer_version(query) == Self::current_producer_version(authority, query)
                && (query.role != CellRole::Input
                    || self.spent_inputs.get(&query.out_point)
                        == Self::current_spender(authority, &query.out_point).as_ref())
        })
    }

    fn same_observations(&self, other: &Self) -> bool {
        other.queries.iter().all(|query| {
            self.producer_version(query) == other.producer_version(query)
                && (query.role != CellRole::Input
                    || self.spent_inputs.get(&query.out_point)
                        == other.spent_inputs.get(&query.out_point))
        })
    }

    fn producer_version(&self, query: &CellQuery) -> Option<EntryVersion> {
        let producer = self.producers.get(&query.producer)?;
        let index: u32 = query.out_point.index().unpack();
        let index = usize::try_from(index).ok()?;
        (index < producer.tx.outputs().len()).then_some(producer.owner.version)
    }

    fn current_producer_version(
        authority: &TxPoolAuthority,
        query: &CellQuery,
    ) -> Option<EntryVersion> {
        let owner = authority.entry_guard(&query.producer);
        let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
            return None;
        };
        let index: u32 = query.out_point.index().unpack();
        let index = usize::try_from(index).ok()?;
        (index < entry.record.tx.outputs().len()).then_some(entry.record.version)
    }

    fn current_spender(
        authority: &TxPoolAuthority,
        out_point: &OutPoint,
    ) -> Option<AcceptedOwnerObservation> {
        let key = authority.accepted_spender(out_point)?;
        let owner = authority.entry_guard(&key);
        let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
            return None;
        };
        Some(AcceptedOwnerObservation {
            key,
            version: entry.record.version,
        })
    }

    pub(super) fn is_accepted_output(&self, out_point: &OutPoint) -> bool {
        let query = CellQuery::new(out_point.clone(), CellRole::ProducerOnly);
        self.producer_version(&query).is_some()
    }

    fn capture_cell(
        producers: &mut HashMap<RawTxHash, AcceptedProducerObservation>,
        spent_inputs: &mut HashMap<OutPoint, AcceptedOwnerObservation>,
        authority: &TxPoolAuthority,
        query: &CellQuery,
    ) {
        if query.role == CellRole::Input
            && let Some(spender) = Self::current_spender(authority, &query.out_point)
        {
            spent_inputs.insert(query.out_point.clone(), spender);
        }
        if producers.contains_key(&query.producer) {
            return;
        }
        let owner = authority.entry_guard(&query.producer);
        let Some(OwnedTx::Accepted(entry)) = owner.as_deref() else {
            return;
        };
        let index: u32 = query.out_point.index().unpack();
        let Some(index) = usize::try_from(index).ok() else {
            return;
        };
        if index < entry.record.tx.outputs().len() {
            producers.insert(
                query.producer.clone(),
                AcceptedProducerObservation {
                    owner: AcceptedOwnerObservation {
                        key: query.producer.clone(),
                        version: entry.record.version,
                    },
                    tx: Arc::clone(&entry.record.tx),
                },
            );
        }
    }

    fn is_spent(&self, out_point: &OutPoint) -> bool {
        self.spent_inputs.contains_key(out_point)
    }
}

struct SparsePoolCellProvider<'a> {
    overlay: &'a AcceptedOverlay,
    observe_spends: bool,
}

impl CellProvider for SparsePoolCellProvider<'_> {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        if self.observe_spends && self.overlay.is_spent(out_point) {
            return CellStatus::Dead;
        }
        let hash = RawTxHash(out_point.tx_hash());
        let Some(producer) = self.overlay.producers.get(&hash) else {
            return CellStatus::Unknown;
        };
        let index: u32 = out_point.index().unpack();
        let Some(index) = usize::try_from(index).ok() else {
            return CellStatus::Unknown;
        };
        let Some((output, data)) = producer.tx.output_with_data(index) else {
            return CellStatus::Unknown;
        };
        CellStatus::live_cell(
            CellMetaBuilder::from_cell_output(output, data)
                .out_point(out_point.clone())
                .build(),
        )
    }
}

#[derive(Debug)]
enum ResolveLeaseWork {
    Resolve(ResolveWork),
    Continuous(ContinuousResolveWork),
}

impl ResolveLeaseWork {
    fn transaction(&self) -> &TransactionView {
        match self {
            Self::Resolve(work) => work.transaction(),
            Self::Continuous(work) => work.transaction(),
        }
    }

    fn grant_edges(&self) -> usize {
        match self {
            Self::Resolve(work) => work.resolution_grant().max_edges(),
            Self::Continuous(work) => work.resolution_grant().max_edges(),
        }
    }

    fn payload_policy(&self) -> PayloadPolicy {
        match self {
            Self::Resolve(work) => work.payload_policy(),
            Self::Continuous(work) => work.payload_policy(),
        }
    }

    fn chain_view(&self) -> &super::state::ChainViewId {
        match self {
            Self::Resolve(work) => work.chain_view(),
            Self::Continuous(work) => work.chain_view(),
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "resolution failure returns the exact unboxed compute settlement capability; boxing would allocate on hostile but valid outcomes"
    )]
    fn resolved(
        self,
        evidence: ResolutionEvidence,
        snapshot: Arc<Snapshot>,
    ) -> Result<ResolutionEvaluation, ResolutionExecutionFailure> {
        match self {
            Self::Resolve(work) => work
                .resolved(evidence)
                .map(ResolutionEvaluation::Settle)
                .map_err(ResolutionExecutionFailure::from_resolution_receipt),
            Self::Continuous(work) => match work.resolved(evidence) {
                Ok(ContinuousResolution::Settle(settlement)) => {
                    Ok(ResolutionEvaluation::Settle(settlement))
                }
                Ok(ContinuousResolution::Verify(work)) => Ok(ResolutionEvaluation::Verify(
                    VerificationJob::from_continuation(work, snapshot),
                )),
                Err(failure) => Err(ResolutionExecutionFailure::from_resolution_receipt(failure)),
            },
        }
    }

    #[expect(
        clippy::result_large_err,
        reason = "missing-dependency failure returns the exact unboxed compute settlement capability; boxing would allocate on a peer-controlled path"
    )]
    fn missing(
        self,
        missing: Vec<DependencyKey>,
    ) -> Result<ComputeSettlement, ResolutionExecutionFailure> {
        match self {
            Self::Resolve(work) => work.missing(missing),
            Self::Continuous(work) => work.missing(missing),
        }
        .map_err(ResolutionExecutionFailure::from_resolution_receipt)
    }

    fn rejected(self, reason: Reject) -> ComputeSettlement {
        match self {
            Self::Resolve(work) => work.rejected(reason),
            Self::Continuous(work) => work.rejected(reason),
        }
    }

    fn resource_denied(self) -> ComputeSettlement {
        match self {
            Self::Resolve(work) => work.resource_denied(),
            Self::Continuous(work) => work.resource_denied(),
        }
    }

    fn retry(self) -> ComputeSettlement {
        match self {
            Self::Resolve(work) => work.internal_failure(),
            Self::Continuous(work) => work.internal_failure(),
        }
    }
}

/// One checked-out resolve capability paired with the only snapshot and sparse
/// Accepted overlay it may consume.
#[derive(Debug)]
#[must_use = "resolution ownership must settle, continue verification, or request bounded enrichment"]
pub(super) struct ResolutionJob {
    work: ResolveLeaseWork,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
}

/// Owner-free synchronous resolution against one paired chain/Accepted cut.
/// Local and TestAccept share this capability; it cannot settle retained work
/// or mutate the authority.
#[derive(Debug)]
#[must_use = "direct resolution must verify, reject, or complete bounded enrichment"]
pub(super) struct DirectResolutionJob {
    tx: Arc<TransactionView>,
    command: DirectCommand,
    view: ChainViewId,
    dependency_cut: DependencyCut,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
    max_resident_bytes: usize,
    max_edges: usize,
}

#[derive(Debug)]
#[must_use = "prepared direct resolution must complete against one authority cut"]
pub(super) struct PreparedDirectResolutionJob {
    tx: Arc<TransactionView>,
    command: DirectCommand,
    overlay: AcceptedOverlay,
    max_resident_bytes: usize,
    max_edges: usize,
}

#[derive(Debug)]
#[must_use = "direct preparation must continue or return its exact policy rejection"]
pub(super) enum DirectResolutionPreparation {
    Prepared(PreparedDirectResolutionJob),
    Rejected(DirectTransactionRejection),
}

#[derive(Debug)]
#[must_use = "direct resolution output must be verified, rejected, or enriched"]
pub(super) enum DirectResolutionEvaluation {
    Verify(DirectVerificationRequest),
    Rejected(DirectTransactionRejection),
    Enrich(DirectResolutionProbe),
}

#[derive(Debug)]
#[must_use = "direct missing resolution must be observed or rejected"]
pub(super) struct DirectResolutionProbe {
    job: DirectResolutionJob,
    missing: Vec<CellQuery>,
}

#[derive(Debug)]
#[must_use = "prepared direct observation must be completed against one authority cut"]
pub(super) struct PreparedDirectResolutionProbe {
    job: DirectResolutionJob,
    first_missing: OutPoint,
    refreshed: AcceptedOverlay,
}

#[derive(Debug)]
enum DirectResolutionProbeCut {
    Retry(DirectResolutionJob),
    Reject(DirectResolutionJob, OutPoint),
    Stale(DirectResolutionJob, AcceptedOverlay),
}

#[derive(Debug)]
#[must_use = "discarded read receipts must retire outside the authority guard"]
pub(super) struct DirectResolutionProbeRecheck {
    cut: DirectResolutionProbeCut,
    discarded: Option<AcceptedOverlay>,
}

#[derive(Debug)]
struct DirectResolvedCandidate {
    tx: Arc<TransactionView>,
    command: DirectCommand,
    accepted_reads: AcceptedOverlay,
    dependency_cut: DependencyCut,
    snapshot: Arc<Snapshot>,
    payload: Arc<ResolvedPayload>,
    location: CellLocationReceipt,
}

#[derive(Debug)]
#[must_use = "direct verification request must execute under its sealed rules"]
pub(crate) struct DirectVerificationRequest {
    candidate: DirectResolvedCandidate,
    environment: Arc<TxVerifyEnv>,
    cache_key: TxVerificationCacheKey,
    max_cycles: u64,
    budget: TxPoolVerificationBudget,
}

#[derive(Debug)]
#[must_use = "direct verification must feed validation or return its exact rejection"]
pub(super) enum DirectVerificationOutcome {
    Candidate(DirectVerifiedCandidate),
    Rejected(DirectTransactionRejection),
}

/// One verified direct candidate and its still-sealed cache consequence.
///
/// The cache update is intentionally inseparable from the admission work here.
/// Only the Local committing boundary may release it after an Accepted Apply;
/// TestAccept and every non-accepting disposition consume it without
/// publication. This makes cache publication follow authoritative membership
/// instead of relying on a service-call ordering convention.
#[derive(Debug)]
#[must_use = "verified direct evidence must be committed or evaluated"]
pub(super) struct DirectVerifiedCandidate {
    command: DirectCommand,
    work: DirectAdmissionWork,
    cache_update: Option<VerificationCacheUpdate>,
}

impl DirectVerifiedCandidate {
    pub(super) fn into_parts(
        self,
    ) -> (
        DirectCommand,
        DirectAdmissionWork,
        Option<VerificationCacheUpdate>,
    ) {
        (self.command, self.work, self.cache_update)
    }
}

#[derive(Debug)]
#[must_use = "a missing resolution must be enriched or settled"]
pub(super) struct ResolutionProbe {
    job: ResolutionJob,
    missing: Vec<CellQuery>,
}

#[derive(Debug)]
#[must_use = "resolution output owns the checked-out capability"]
pub(super) enum ResolutionEvaluation {
    Settle(ComputeSettlement),
    Verify(VerificationJob),
    Enrich(ResolutionProbe),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolutionExecutionKind {
    StaleView,
    ComputeBudget,
    ResourceUnavailable,
    InvalidReceipt(ResolutionReceiptDefect),
}

/// Closed programmer-defect subset of resolution receipt failures.
///
/// Allocation pressure is deliberately absent: the resolver converts it to
/// `ResourceUnavailable` before this type is constructed, so a worker cannot
/// accidentally promote a legal allocator failure to generation invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolutionReceiptDefect {
    TransactionMismatch,
    InvalidEvidence(InputEvidenceError),
    EmptyDependencies,
}

#[derive(Debug)]
#[must_use = "a failed execution still owns the exact retry settlement"]
pub(super) struct ResolutionExecutionFailure {
    kind: ResolutionExecutionKind,
    settlement: ComputeSettlement,
}

impl ResolutionExecutionFailure {
    fn from_resolution_receipt(failure: ReceiptFailure<ResolutionReceiptError>) -> Self {
        let kind = match *failure.error() {
            ResolutionReceiptError::DependencyAllocation => {
                ResolutionExecutionKind::ResourceUnavailable
            }
            ResolutionReceiptError::TransactionMismatch => ResolutionExecutionKind::InvalidReceipt(
                ResolutionReceiptDefect::TransactionMismatch,
            ),
            ResolutionReceiptError::InvalidEvidence(error) => {
                ResolutionExecutionKind::InvalidReceipt(ResolutionReceiptDefect::InvalidEvidence(
                    error,
                ))
            }
            ResolutionReceiptError::EmptyDependencies => {
                ResolutionExecutionKind::InvalidReceipt(ResolutionReceiptDefect::EmptyDependencies)
            }
        };
        Self {
            kind,
            settlement: failure.into_settlement(),
        }
    }

    pub(super) fn kind(&self) -> ResolutionExecutionKind {
        self.kind
    }

    pub(super) fn into_settlement(self) -> ComputeSettlement {
        self.settlement
    }
}

enum ResolutionAttempt {
    Resolved(ResolvedTransaction),
    Missing { permissive_inputs: bool },
    Rejected(OutPointError),
    ResourceUnavailable,
}

enum ResolveAgainstCutError {
    Rejected(OutPointError),
    ResourceUnavailable,
}

struct ResolvedComputation {
    transaction: Arc<ResolvedTransaction>,
    fee: Capacity,
    resident_bytes: usize,
}

enum FinishResolutionError {
    Rejected(Reject),
    ResourceUnavailable,
}

impl ResolutionJob {
    pub(super) fn retry(self) -> ComputeSettlement {
        self.work.retry()
    }

    #[expect(
        clippy::result_large_err,
        reason = "capture failure returns the exact unboxed compute settlement capability; boxing would allocate on stale or hostile work"
    )]
    pub(super) fn capture_resolve(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: ResolveWork,
    ) -> Result<Self, ResolutionExecutionFailure> {
        Self::capture(authority, snapshot, ResolveLeaseWork::Resolve(work))
    }

    #[expect(
        clippy::result_large_err,
        reason = "capture failure returns the exact unboxed compute settlement capability; boxing would allocate on stale or hostile work"
    )]
    pub(super) fn capture_continuous(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: ContinuousResolveWork,
    ) -> Result<Self, ResolutionExecutionFailure> {
        Self::capture(authority, snapshot, ResolveLeaseWork::Continuous(work))
    }

    #[expect(
        clippy::result_large_err,
        reason = "capture failure returns the exact unboxed compute settlement capability; boxing would allocate on stale or hostile work"
    )]
    fn capture(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: ResolveLeaseWork,
    ) -> Result<Self, ResolutionExecutionFailure> {
        if snapshot.tip_hash() != work.chain_view().tip().0 {
            return Err(ResolutionExecutionFailure {
                kind: ResolutionExecutionKind::StaleView,
                settlement: work.retry(),
            });
        }
        let overlay =
            match AcceptedOverlay::capture(authority, work.transaction(), work.grant_edges()) {
                Ok(overlay) => overlay,
                Err(kind) => {
                    return Err(ResolutionExecutionFailure {
                        kind,
                        settlement: match kind {
                            ResolutionExecutionKind::ComputeBudget => work.resource_denied(),
                            ResolutionExecutionKind::StaleView
                            | ResolutionExecutionKind::ResourceUnavailable
                            | ResolutionExecutionKind::InvalidReceipt(_) => work.retry(),
                        },
                    });
                }
            };
        Ok(Self {
            work,
            snapshot,
            overlay,
        })
    }

    #[expect(
        clippy::result_large_err,
        reason = "resolution failure returns the exact unboxed compute settlement capability; boxing would allocate on hostile but valid outcomes"
    )]
    pub(super) fn evaluate(
        self,
        min_fee_rate: FeeRate,
        large_cycle_threshold: u64,
    ) -> Result<ResolutionEvaluation, ResolutionExecutionFailure> {
        let resolved =
            match resolve_candidate(self.work.transaction(), &self.snapshot, &self.overlay) {
                ResolutionAttempt::Resolved(resolved) => resolved,
                ResolutionAttempt::Missing { permissive_inputs } => {
                    return self.missing_probe(permissive_inputs);
                }
                ResolutionAttempt::Rejected(error) => {
                    return Ok(ResolutionEvaluation::Settle(
                        self.work.rejected(Reject::Resolve(error)),
                    ));
                }
                ResolutionAttempt::ResourceUnavailable => {
                    return Err(ResolutionExecutionFailure {
                        kind: ResolutionExecutionKind::ResourceUnavailable,
                        settlement: self.work.retry(),
                    });
                }
            };
        let resolved = match finish_resolution(
            &self.snapshot,
            resolved,
            self.work.transaction().data().serialized_size_in_block(),
            min_fee_rate,
        ) {
            Ok(resolved) => resolved,
            Err(FinishResolutionError::Rejected(reject)) => {
                return Ok(ResolutionEvaluation::Settle(self.work.rejected(reject)));
            }
            Err(FinishResolutionError::ResourceUnavailable) => {
                return Err(ResolutionExecutionFailure {
                    kind: ResolutionExecutionKind::ResourceUnavailable,
                    settlement: self.work.retry(),
                });
            }
        };
        let verify_class = self
            .work
            .payload_policy()
            .verify_cycle_class(large_cycle_threshold);
        self.work.resolved(
            ResolutionEvidence::from_resolution(
                ResolutionEvidenceSeal(()),
                resolved.transaction,
                resolved.fee,
                resolved.resident_bytes,
                verify_class,
            ),
            self.snapshot,
        )
    }

    #[expect(
        clippy::result_large_err,
        reason = "missing-frontier failure returns the exact unboxed compute settlement capability; boxing would allocate on a peer-controlled path"
    )]
    fn missing_probe(
        self,
        permissive_inputs: bool,
    ) -> Result<ResolutionEvaluation, ResolutionExecutionFailure> {
        match self.collect_missing(permissive_inputs) {
            Ok(missing) => Ok(ResolutionEvaluation::Enrich(ResolutionProbe {
                job: self,
                missing,
            })),
            Err(MissingScanError::Reject(error)) => Ok(ResolutionEvaluation::Settle(
                self.work.rejected(Reject::Resolve(error)),
            )),
            Err(MissingScanError::ComputeBudget) => {
                Ok(ResolutionEvaluation::Settle(self.work.resource_denied()))
            }
            Err(MissingScanError::ResourceUnavailable) => Err(ResolutionExecutionFailure {
                kind: ResolutionExecutionKind::ResourceUnavailable,
                settlement: self.work.retry(),
            }),
        }
    }

    fn collect_missing(&self, permissive_inputs: bool) -> Result<Vec<CellQuery>, MissingScanError> {
        collect_missing_against_cut(
            self.work.transaction(),
            &self.snapshot,
            &self.overlay,
            self.work.grant_edges(),
            permissive_inputs,
        )
    }
}

fn resolve_against_cut(
    tx: &TransactionView,
    snapshot: &Snapshot,
    overlay: &AcceptedOverlay,
    permissive_inputs: bool,
) -> Result<ResolvedTransaction, ResolveAgainstCutError> {
    let input_overlay = SparsePoolCellProvider {
        overlay,
        observe_spends: !permissive_inputs,
    };
    let dependency_overlay = SparsePoolCellProvider {
        overlay,
        observe_spends: false,
    };
    let input_provider = OverlayCellProvider::new(&input_overlay, snapshot);
    let dependency_provider = OverlayCellProvider::new(&dependency_overlay, snapshot);
    let mut seen_inputs = HashSet::new();
    seen_inputs
        .try_reserve(tx.inputs().len())
        .map_err(|_| ResolveAgainstCutError::ResourceUnavailable)?;
    resolve_transaction_with_cell_providers(
        tx.clone(),
        &mut seen_inputs,
        &input_provider,
        &dependency_provider,
        snapshot,
    )
    .map_err(ResolveAgainstCutError::Rejected)
}

/// Resolve strict and permissive RBF evidence through one shared decision
/// table. Retained work and owner-free direct work must not drift on whether
/// an early unknown input can hide a later Accepted conflict.
fn resolve_candidate(
    tx: &TransactionView,
    snapshot: &Snapshot,
    overlay: &AcceptedOverlay,
) -> ResolutionAttempt {
    let has_pool_conflict = tx
        .input_pts_iter()
        .any(|out_point| overlay.is_spent(&out_point));
    match resolve_against_cut(tx, snapshot, overlay, false) {
        Ok(resolved) => ResolutionAttempt::Resolved(resolved),
        Err(ResolveAgainstCutError::Rejected(OutPointError::Dead(out_point)))
            if overlay.is_spent(&out_point) =>
        {
            permissive_resolution(tx, snapshot, overlay)
        }
        // The consensus resolver stops on the first missing input. A later
        // input may already be spent by Accepted membership, so `Unknown`
        // alone cannot classify the transaction as a non-RBF orphan.
        Err(ResolveAgainstCutError::Rejected(OutPointError::Unknown(_))) if has_pool_conflict => {
            permissive_resolution(tx, snapshot, overlay)
        }
        Err(ResolveAgainstCutError::Rejected(OutPointError::Unknown(_))) => {
            ResolutionAttempt::Missing {
                permissive_inputs: false,
            }
        }
        Err(ResolveAgainstCutError::Rejected(error)) => ResolutionAttempt::Rejected(error),
        Err(ResolveAgainstCutError::ResourceUnavailable) => ResolutionAttempt::ResourceUnavailable,
    }
}

fn permissive_resolution(
    tx: &TransactionView,
    snapshot: &Snapshot,
    overlay: &AcceptedOverlay,
) -> ResolutionAttempt {
    match resolve_against_cut(tx, snapshot, overlay, true) {
        Ok(resolved) => ResolutionAttempt::Resolved(resolved),
        Err(ResolveAgainstCutError::Rejected(OutPointError::Unknown(_))) => {
            ResolutionAttempt::Missing {
                permissive_inputs: true,
            }
        }
        Err(ResolveAgainstCutError::Rejected(error)) => ResolutionAttempt::Rejected(error),
        Err(ResolveAgainstCutError::ResourceUnavailable) => ResolutionAttempt::ResourceUnavailable,
    }
}

fn finish_resolution(
    snapshot: &Snapshot,
    resolved: ResolvedTransaction,
    tx_size: usize,
    min_fee_rate: FeeRate,
) -> Result<ResolvedComputation, FinishResolutionError> {
    let transaction = super::residency::compact_after_resolution(resolved)
        .map_err(|_| FinishResolutionError::ResourceUnavailable)?;
    let fee = check_tx_fee_with_min_fee_rate(snapshot, &transaction, tx_size, min_fee_rate)
        .map_err(FinishResolutionError::Rejected)?;
    let resident_bytes = resolved_transaction_charge_bytes(tx_size, &transaction);
    Ok(ResolvedComputation {
        transaction,
        fee,
        resident_bytes,
    })
}

fn collect_missing_against_cut(
    tx: &TransactionView,
    snapshot: &Snapshot,
    overlay: &AcceptedOverlay,
    max_edges: usize,
    permissive_inputs: bool,
) -> Result<Vec<CellQuery>, MissingScanError> {
    let direct_edges = tx
        .inputs()
        .len()
        .checked_add(tx.cell_deps().len())
        .and_then(|count| count.checked_add(tx.header_deps().len()))
        .ok_or(MissingScanError::ResourceUnavailable)?;
    if direct_edges > max_edges {
        return Err(MissingScanError::ComputeBudget);
    }
    let mut missing = Vec::new();
    missing
        .try_reserve(direct_edges)
        .map_err(|_| MissingScanError::ResourceUnavailable)?;

    let input_overlay = SparsePoolCellProvider {
        overlay,
        observe_spends: !permissive_inputs,
    };
    let dependency_overlay = SparsePoolCellProvider {
        overlay,
        observe_spends: false,
    };
    let input_provider = OverlayCellProvider::new(&input_overlay, snapshot);
    let dependency_provider = OverlayCellProvider::new(&dependency_overlay, snapshot);

    for out_point in tx.input_pts_iter() {
        collect_cell_status(
            input_provider.cell(&out_point, false),
            out_point,
            CellRole::Input,
            &mut missing,
        )?;
    }

    let mut edge_count = tx
        .inputs()
        .len()
        .checked_add(tx.header_deps().len())
        .ok_or(MissingScanError::ResourceUnavailable)?;
    for cell_dep in tx.cell_deps_iter() {
        if SYSTEM_CELL
            .get()
            .is_some_and(|system| system.contains_key(&cell_dep))
        {
            let cached_edges = match SYSTEM_CELL.get().and_then(|system| system.get(&cell_dep)) {
                Some(ResolvedDep::Cell(_)) => 1,
                Some(ResolvedDep::Group(_, cells)) => cells
                    .len()
                    .checked_add(1)
                    .ok_or(MissingScanError::ResourceUnavailable)?,
                None => 0,
            };
            edge_count = edge_count
                .checked_add(cached_edges)
                .ok_or(MissingScanError::ResourceUnavailable)?;
            if edge_count > max_edges {
                return Err(MissingScanError::ComputeBudget);
            }
            continue;
        }

        let out_point = cell_dep.out_point();
        let eager_load = cell_dep.dep_type() == DepType::DepGroup.into();
        let direct = dependency_provider.cell(&out_point, eager_load);
        edge_count = edge_count
            .checked_add(1)
            .ok_or(MissingScanError::ResourceUnavailable)?;
        if edge_count > max_edges {
            return Err(MissingScanError::ComputeBudget);
        }
        let CellStatus::Live(cell) = direct else {
            collect_cell_status(direct, out_point, CellRole::ProducerOnly, &mut missing)?;
            continue;
        };
        if !eager_load {
            continue;
        }
        let Some(data) = cell.mem_cell_data.as_ref() else {
            return Err(MissingScanError::Reject(OutPointError::InvalidDepGroup(
                out_point,
            )));
        };
        let members = OutPointVec::from_slice(data).map_err(|_| {
            MissingScanError::Reject(OutPointError::InvalidDepGroup(out_point.clone()))
        })?;
        if members.is_empty() {
            return Err(MissingScanError::Reject(OutPointError::InvalidDepGroup(
                out_point,
            )));
        }
        edge_count = edge_count
            .checked_add(members.len())
            .ok_or(MissingScanError::ResourceUnavailable)?;
        if edge_count > max_edges {
            return Err(MissingScanError::ComputeBudget);
        }
        missing
            .try_reserve(members.len())
            .map_err(|_| MissingScanError::ResourceUnavailable)?;
        for member in members.into_iter() {
            collect_cell_status(
                dependency_provider.cell(&member, false),
                member,
                CellRole::ProducerOnly,
                &mut missing,
            )?;
        }
    }

    for header in tx.header_deps_iter() {
        if let Err(error) = snapshot.check_valid(&header) {
            return Err(MissingScanError::Reject(error));
        }
    }
    if missing.is_empty() {
        return Err(MissingScanError::ResourceUnavailable);
    }
    missing.sort_unstable_by(|left, right| {
        left.out_point
            .cmp(&right.out_point)
            .then_with(|| left.role.cmp(&right.role))
    });
    missing.dedup();
    Ok(missing)
}

impl DirectResolutionJob {
    pub(super) fn prepare(
        direct: DirectTransaction,
        max_resident_bytes: usize,
        max_edges: usize,
    ) -> Result<DirectResolutionPreparation, DirectComputationError> {
        let (tx, command) = direct.into_parts();
        let overlay = match AcceptedOverlay::prepare(&tx, max_edges) {
            Ok(overlay) => overlay,
            Err(ResolutionExecutionKind::ComputeBudget) => {
                return Ok(DirectResolutionPreparation::Rejected(
                    direct_resource_rejection(tx, command),
                ));
            }
            Err(ResolutionExecutionKind::ResourceUnavailable) => {
                return Err(DirectComputationError::ResourceUnavailable);
            }
            Err(
                ResolutionExecutionKind::StaleView | ResolutionExecutionKind::InvalidReceipt(_),
            ) => return Err(DirectComputationError::InvalidEvidence),
        };
        Ok(DirectResolutionPreparation::Prepared(
            PreparedDirectResolutionJob {
                tx,
                command,
                overlay,
                max_resident_bytes,
                max_edges,
            },
        ))
    }

    pub(super) fn evaluate(
        self,
        min_fee_rate: FeeRate,
        budget: TxPoolVerificationBudget,
    ) -> Result<DirectResolutionEvaluation, DirectComputationError> {
        let resolved = match resolve_candidate(&self.tx, &self.snapshot, &self.overlay) {
            ResolutionAttempt::Resolved(resolved) => resolved,
            ResolutionAttempt::Missing { permissive_inputs } => {
                return self.missing_probe(permissive_inputs);
            }
            ResolutionAttempt::Rejected(error) => {
                return Ok(self.rejected(Reject::Resolve(error)));
            }
            ResolutionAttempt::ResourceUnavailable => {
                return Err(DirectComputationError::ResourceUnavailable);
            }
        };
        let resolved = match finish_resolution(
            &self.snapshot,
            resolved,
            self.tx.data().serialized_size_in_block(),
            min_fee_rate,
        ) {
            Ok(resolved) => resolved,
            Err(FinishResolutionError::Rejected(reject)) => return Ok(self.rejected(reject)),
            Err(FinishResolutionError::ResourceUnavailable) => {
                return Err(DirectComputationError::ResourceUnavailable);
            }
        };
        if resolved.resident_bytes > self.max_resident_bytes {
            return Ok(self.resource_rejected());
        }
        let payload = match ResolvedPayload::from_direct_resolution(
            DirectResolutionSeal(()),
            resolved.transaction,
            self.max_edges,
            resolved.fee,
            resolved.resident_bytes,
        ) {
            Ok(payload) => Arc::new(payload),
            Err(error) => match error.disposition() {
                InputEvidenceDisposition::MalformedTransaction => {
                    return Ok(self.rejected(duplicate_inputs_reject()));
                }
                InputEvidenceDisposition::ResourceDenied => {
                    return Ok(self.resource_rejected());
                }
                InputEvidenceDisposition::ResourceUnavailable => {
                    return Err(DirectComputationError::ResourceUnavailable);
                }
                InputEvidenceDisposition::Structural => {
                    return Err(DirectComputationError::InvalidEvidence);
                }
            },
        };
        let location = CellLocationReceipt::from_resolution(self.view, &payload).map_err(
            |error| match error {
                CellLocationReceiptError::Allocation => DirectComputationError::ResourceUnavailable,
                CellLocationReceiptError::Arithmetic => DirectComputationError::InvalidEvidence,
            },
        )?;
        let status = proposal_status(&self.snapshot, &self.tx.proposal_short_id());
        let environment = Arc::new(verification_environment(status, &self.snapshot));
        let rules = ScriptVerificationRules::from_env(self.snapshot.consensus(), &environment);
        let cache_key = TxVerificationCacheKey::from_transaction(&self.tx, rules);
        let max_cycles = self.snapshot.consensus().max_block_cycles();
        Ok(DirectResolutionEvaluation::Verify(
            DirectVerificationRequest {
                candidate: DirectResolvedCandidate {
                    tx: self.tx,
                    command: self.command,
                    accepted_reads: self.overlay,
                    dependency_cut: self.dependency_cut,
                    snapshot: self.snapshot,
                    payload,
                    location,
                },
                environment,
                cache_key,
                max_cycles,
                budget,
            },
        ))
    }

    fn missing_probe(
        self,
        permissive_inputs: bool,
    ) -> Result<DirectResolutionEvaluation, DirectComputationError> {
        match collect_missing_against_cut(
            &self.tx,
            &self.snapshot,
            &self.overlay,
            self.max_edges,
            permissive_inputs,
        ) {
            Ok(missing) => Ok(DirectResolutionEvaluation::Enrich(DirectResolutionProbe {
                job: self,
                missing,
            })),
            Err(MissingScanError::Reject(error)) => Ok(self.rejected(Reject::Resolve(error))),
            Err(MissingScanError::ComputeBudget) => Ok(self.resource_rejected()),
            Err(MissingScanError::ResourceUnavailable) => {
                Err(DirectComputationError::ResourceUnavailable)
            }
        }
    }

    fn rejected(self, reason: Reject) -> DirectResolutionEvaluation {
        DirectResolutionEvaluation::Rejected(self.into_rejection(reason))
    }

    fn into_rejection(self, reason: Reject) -> DirectTransactionRejection {
        DirectTransactionRejection::accepted_reads(
            self.tx,
            self.command,
            reason,
            self.view,
            self.overlay,
        )
    }

    fn resource_rejected(self) -> DirectResolutionEvaluation {
        self.rejected(Reject::Full(
            "transaction exceeds the tx-pool compute residency envelope".to_owned(),
        ))
    }
}

fn direct_resource_rejection(
    tx: Arc<TransactionView>,
    command: DirectCommand,
) -> DirectTransactionRejection {
    DirectTransactionRejection::stable(
        tx,
        command,
        Reject::Full("transaction exceeds the tx-pool compute residency envelope".to_owned()),
    )
}

impl PreparedDirectResolutionJob {
    pub(super) fn complete(
        mut self,
        _seal: super::runtime::AuthorityStoreCaptureSeal,
        snapshot: Arc<Snapshot>,
        authority: &TxPoolAuthority,
    ) -> DirectResolutionJob {
        self.overlay.populate(authority);
        DirectResolutionJob {
            tx: self.tx,
            command: self.command,
            view: authority.chain_view().clone(),
            dependency_cut: authority.dependency_observation_cut(),
            snapshot,
            overlay: self.overlay,
            max_resident_bytes: self.max_resident_bytes,
            max_edges: self.max_edges,
        }
    }
}

#[derive(Debug)]
#[must_use = "direct enrichment observation must retry or reject the immutable request"]
pub(super) enum DirectResolutionProbeObservation {
    Retry(DirectResolutionJob),
    Rejected(DirectTransactionRejection),
}

impl DirectResolutionProbe {
    pub(super) fn prepare_enrichment(
        self,
    ) -> Result<PreparedDirectResolutionProbe, DirectComputationError> {
        let first_missing = self
            .missing
            .first()
            .map(|query| query.out_point.clone())
            .ok_or(DirectComputationError::InvalidEvidence)?;
        let refreshed = self
            .job
            .overlay
            .prepare_refresh(&self.missing, self.job.max_edges)
            .map_err(|kind| match kind {
                ResolutionExecutionKind::ResourceUnavailable => {
                    DirectComputationError::ResourceUnavailable
                }
                ResolutionExecutionKind::StaleView
                | ResolutionExecutionKind::ComputeBudget
                | ResolutionExecutionKind::InvalidReceipt(_) => {
                    DirectComputationError::InvalidEvidence
                }
            })?;
        Ok(PreparedDirectResolutionProbe {
            job: self.job,
            first_missing,
            refreshed,
        })
    }
}

impl PreparedDirectResolutionProbe {
    pub(super) fn observe(mut self, authority: &TxPoolAuthority) -> DirectResolutionProbeRecheck {
        if authority.chain_view() != &self.job.view {
            return DirectResolutionProbeRecheck {
                cut: DirectResolutionProbeCut::Stale(self.job, self.refreshed),
                discarded: None,
            };
        }
        self.refreshed.populate(authority);
        let changed = !self.job.overlay.same_observations(&self.refreshed);
        let discarded = std::mem::replace(&mut self.job.overlay, self.refreshed);
        let cut = if changed {
            self.job.dependency_cut = authority.dependency_observation_cut();
            DirectResolutionProbeCut::Retry(self.job)
        } else {
            DirectResolutionProbeCut::Reject(self.job, self.first_missing)
        };
        DirectResolutionProbeRecheck {
            cut,
            discarded: Some(discarded),
        }
    }
}

impl DirectResolutionProbeRecheck {
    pub(super) fn finish(self) -> Result<DirectResolutionProbeObservation, DirectComputationError> {
        let Self { cut, discarded } = self;
        drop(discarded);
        match cut {
            DirectResolutionProbeCut::Retry(job) => {
                Ok(DirectResolutionProbeObservation::Retry(job))
            }
            DirectResolutionProbeCut::Reject(job, first_missing) => {
                Ok(DirectResolutionProbeObservation::Rejected(
                    job.into_rejection(Reject::Resolve(OutPointError::Unknown(first_missing))),
                ))
            }
            DirectResolutionProbeCut::Stale(job, refreshed) => {
                drop((job, refreshed));
                Err(DirectComputationError::StaleView)
            }
        }
    }
}

impl ResolutionProbe {
    /// Reserve fallible collection growth before the authority read cut. A
    /// failure retains the exact settlement capability for an ordinary retry.
    #[expect(
        clippy::result_large_err,
        reason = "enrichment failure returns the exact unboxed compute settlement capability; boxing would allocate on missing-dependency paths"
    )]
    pub(super) fn prepare_enrichment(mut self) -> Result<Self, ResolutionExecutionFailure> {
        if let Err(kind) = self.job.overlay.reserve_enrichment(self.missing.len()) {
            return Err(ResolutionExecutionFailure {
                kind,
                settlement: self.job.work.retry(),
            });
        }
        Ok(self)
    }

    /// Perform the allocation-free Accepted observation under the authority
    /// read guard. Missing-set canonicalization remains outside that guard.
    pub(super) fn observe(mut self, authority: &TxPoolAuthority) -> ResolutionProbeObservation {
        if self
            .job
            .overlay
            .observe_enrichment(authority, &self.missing)
        {
            ResolutionProbeObservation::Retry(self.job)
        } else {
            ResolutionProbeObservation::Missing(self)
        }
    }

    /// Compile the complete missing set after the authority guard opens.
    #[expect(
        clippy::result_large_err,
        reason = "settlement failure returns the exact unboxed compute capability; boxing would allocate on missing-dependency paths"
    )]
    pub(super) fn settle_missing(self) -> Result<ComputeSettlement, ResolutionExecutionFailure> {
        let Self { job, missing } = self;
        let mut keys = Vec::new();
        if keys.try_reserve_exact(missing.len()).is_err() {
            return Err(ResolutionExecutionFailure {
                kind: ResolutionExecutionKind::ResourceUnavailable,
                settlement: job.work.retry(),
            });
        }
        for cell in missing {
            let out_point = match crate::util::try_compact_packed(&cell.out_point) {
                Ok(out_point) => out_point,
                Err(_) => {
                    return Err(ResolutionExecutionFailure {
                        kind: ResolutionExecutionKind::ResourceUnavailable,
                        settlement: job.work.retry(),
                    });
                }
            };
            keys.push(DependencyKey::Cell(out_point));
        }
        job.work.missing(keys)
    }
}

#[derive(Debug)]
#[must_use = "an enrichment observation must retry resolution or compile its missing settlement"]
pub(super) enum ResolutionProbeObservation {
    Retry(ResolutionJob),
    Missing(ResolutionProbe),
}

enum MissingScanError {
    Reject(OutPointError),
    ComputeBudget,
    ResourceUnavailable,
}

fn collect_cell_status(
    status: CellStatus,
    out_point: OutPoint,
    role: CellRole,
    missing: &mut Vec<CellQuery>,
) -> Result<(), MissingScanError> {
    match status {
        CellStatus::Live(_) => Ok(()),
        CellStatus::Dead => Err(MissingScanError::Reject(OutPointError::Dead(out_point))),
        CellStatus::Unknown => {
            missing.push(CellQuery::new(out_point, role));
            Ok(())
        }
    }
}

/// Script-verification capability paired with the snapshot used to resolve its
/// retained cell metadata. The production runner derives the verification
/// environment and cache rules from this object; callers cannot pass a nearby
/// tip or an unbound rules identifier.
#[derive(Debug)]
#[must_use = "verification work must produce one settlement"]
pub(super) struct VerificationJob {
    work: SnapshotBoundVerifyWork,
    snapshot: Arc<Snapshot>,
}

/// Tx-pool-only script request derived from one snapshot-bound verification
/// capability. The environment, hard-fork rules and witness-key identity are
/// sealed together here; this evidence must not be reused by block validation
/// or constructed from independently sampled chain state.
#[derive(Debug)]
#[must_use = "a prepared verification request still owns the checked-out capability"]
pub(crate) struct TxPoolVerificationRequest {
    job: VerificationJob,
    environment: Arc<TxVerifyEnv>,
    cache_key: TxVerificationCacheKey,
    max_cycles: ckb_types::core::Cycle,
    started_at: AsyncProcessStart,
    budget: TxPoolVerificationBudget,
}

/// A retained verification capability paired with the only cache entry whose
/// witness identity and script rules match that capability. Construction
/// consumes the request and performs the lookup itself, so an independently
/// sampled cache result cannot be supplied to VM execution.
#[derive(Debug)]
#[must_use = "cache-bound verification still owns the checked-out capability"]
pub(in crate::authority) struct CacheBoundTxPoolVerification {
    request: TxPoolVerificationRequest,
    cache_entry: Option<ScriptVerificationProof>,
}

/// Owner-free direct verification paired with its exact cache lookup. This is
/// tx-pool-only evidence; it is not valid for block verification and cannot be
/// assembled from a nearby witness hash or hard-fork rule generation.
#[derive(Debug)]
#[must_use = "cache-bound direct verification must execute or be discarded"]
pub(crate) struct CacheBoundDirectVerification {
    request: DirectVerificationRequest,
    cache_entry: Option<ScriptVerificationProof>,
}

#[derive(Debug)]
pub(crate) struct VerificationCacheUpdate {
    proof: ScriptVerificationProof,
}

impl VerificationCacheUpdate {
    pub(crate) fn into_proof(self) -> ScriptVerificationProof {
        self.proof
    }
}

#[derive(Debug)]
#[must_use = "verification completion must be settled and its optional cache effect published"]
pub(in crate::authority) struct VerificationExecution {
    pub(in crate::authority) settlement: ComputeSettlement,
    pub(in crate::authority) cache_update: Option<VerificationCacheUpdate>,
}

impl VerificationJob {
    #[expect(
        clippy::result_large_err,
        reason = "a stale capture returns the exact unboxed settlement capability instead of allocating on the worker handoff"
    )]
    pub(super) fn from_checkout(
        work: VerifyWork,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, ResolutionExecutionFailure> {
        let work = work
            .bind_current(&snapshot.tip_hash())
            .map_err(|settlement| ResolutionExecutionFailure {
                kind: ResolutionExecutionKind::StaleView,
                settlement,
            })?;
        Ok(Self { work, snapshot })
    }

    fn from_continuation(work: ContinuousVerifyWork, snapshot: Arc<Snapshot>) -> Self {
        Self {
            work: work.into_current(),
            snapshot,
        }
    }

    pub(super) fn transaction(&self) -> &TransactionView {
        self.work.transaction()
    }

    pub(super) fn payload_policy(&self) -> PayloadPolicy {
        self.work.payload_policy()
    }

    pub(super) fn prepare(self, budget: TxPoolVerificationBudget) -> TxPoolVerificationRequest {
        let status = proposal_status(&self.snapshot, &self.transaction().proposal_short_id());
        let environment = Arc::new(verification_environment(status, &self.snapshot));
        let rules = ScriptVerificationRules::from_env(self.snapshot.consensus(), &environment);
        let cache_key = TxVerificationCacheKey::from_transaction(self.transaction(), rules);
        let max_cycles = match self.payload_policy() {
            PayloadPolicy::RemoteDeclaredCycles(limit) => limit.declared(),
            PayloadPolicy::Trusted => self.snapshot.consensus().max_block_cycles(),
        };
        TxPoolVerificationRequest {
            job: self,
            environment,
            cache_key,
            max_cycles,
            started_at: AsyncProcessStart::now(),
            budget,
        }
    }

    pub(super) fn retry(self) -> ComputeSettlement {
        self.work.internal_failure()
    }
}

impl TxPoolVerificationRequest {
    /// Consume this exact request while its matching cache entry is sampled.
    /// The copied value lets the cache guard open before VM execution.
    pub(in crate::authority) fn bind_cache(
        self,
        cache: &TxVerificationCache,
    ) -> CacheBoundTxPoolVerification {
        let cache_entry = cache.lookup(&self.cache_key);
        CacheBoundTxPoolVerification {
            request: self,
            cache_entry,
        }
    }

    /// Return the still-linear verification capability to ordinary resolve
    /// scheduling. This is used only when the sealed worker topology detects
    /// an impossible lane mismatch before VM execution; callers cannot
    /// extract or reconstruct the underlying lease token.
    pub(in crate::authority) fn retry(self) -> ComputeSettlement {
        self.job.retry()
    }
}

impl CacheBoundTxPoolVerification {
    pub(in crate::authority) async fn execute(
        self,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> VerificationExecution {
        let CacheBoundTxPoolVerification {
            request:
                TxPoolVerificationRequest {
                    job,
                    environment,
                    cache_key,
                    max_cycles,
                    started_at,
                    budget,
                },
            cache_entry,
        } = self;
        let VerificationJob { work, snapshot } = job;
        let policy = work.payload_policy();
        let resolved = Arc::clone(work.resolved_transaction());
        let verified = verify_rtx(
            snapshot,
            resolved,
            environment,
            cache_entry,
            max_cycles,
            command_rx,
            budget,
        )
        .await;
        let outcome = match verified {
            Ok(TxPoolVerificationOutcome::Verified(outcome)) => outcome,
            Ok(
                TxPoolVerificationOutcome::DeadlineExceeded
                | TxPoolVerificationOutcome::InitialLoadExceeded,
            ) => {
                return VerificationExecution {
                    settlement: work.rejected(Reject::ExcessiveVerifyTime),
                    cache_update: None,
                };
            }
            Err(reject) => {
                return VerificationExecution {
                    settlement: work.rejected(reject),
                    cache_update: None,
                };
            }
        };
        let policy_accepts_cycles = match policy {
            PayloadPolicy::RemoteDeclaredCycles(limit) => limit.declared() == outcome.cycles(),
            PayloadPolicy::Trusted => true,
        };
        let settlement = work.verified_with_time_context(
            outcome.cycles(),
            super::chain::TimeContextReceipt::from_validation(cache_key.script_rules()),
            started_at,
        );
        let cache_update = policy_accepts_cycles
            .then(|| outcome.executed_proof())
            .flatten()
            .map(|proof| VerificationCacheUpdate { proof });
        VerificationExecution {
            settlement,
            cache_update,
        }
    }
}

impl DirectVerificationRequest {
    /// Bind owner-free direct validation to the exact tx-pool cache key sealed
    /// during snapshot capture. The result is copied out before any await.
    pub(crate) fn bind_cache(self, cache: &TxVerificationCache) -> CacheBoundDirectVerification {
        let cache_entry = cache.lookup(&self.cache_key);
        CacheBoundDirectVerification {
            request: self,
            cache_entry,
        }
    }
}

#[cfg(test)]
#[path = "tests/support/resolver.rs"]
mod test_support;

impl CacheBoundDirectVerification {
    pub(crate) async fn execute(
        self,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<DirectVerificationOutcome, DirectComputationError> {
        let CacheBoundDirectVerification {
            request:
                DirectVerificationRequest {
                    candidate,
                    environment,
                    cache_key,
                    max_cycles,
                    budget,
                },
            cache_entry,
        } = self;
        let DirectResolvedCandidate {
            tx,
            command,
            accepted_reads,
            dependency_cut,
            snapshot,
            payload,
            location,
        } = candidate;
        let outcome = match verify_rtx(
            snapshot,
            Arc::clone(payload.resolved_transaction()),
            environment,
            cache_entry,
            max_cycles,
            command_rx,
            budget,
        )
        .await
        {
            Ok(TxPoolVerificationOutcome::Verified(outcome)) => outcome,
            Ok(
                TxPoolVerificationOutcome::DeadlineExceeded
                | TxPoolVerificationOutcome::InitialLoadExceeded,
            ) => {
                return Ok(DirectVerificationOutcome::Rejected(
                    DirectTransactionRejection::accepted_reads(
                        tx,
                        command,
                        Reject::ExcessiveVerifyTime,
                        location.into_view(),
                        accepted_reads,
                    ),
                ));
            }
            Err(reason) => {
                return Ok(DirectVerificationOutcome::Rejected(
                    DirectTransactionRejection::accepted_reads(
                        tx,
                        command,
                        reason,
                        location.into_view(),
                        accepted_reads,
                    ),
                ));
            }
        };
        let context = VerificationContextReceipt::from_validation(
            location,
            TimeContextReceipt::from_validation(cache_key.script_rules()),
        );
        let fee = payload.fee();
        let serialized_bytes = payload.serialized_bytes();
        let (payload, accepted_resident_bytes) =
            ResolvedPayload::compact_after_direct_verification(payload, DirectVerificationSeal(()));
        let metrics = CandidateMetrics {
            fee,
            cost: AcceptedCost::new(serialized_bytes, accepted_resident_bytes, outcome.cycles()),
        };
        let verified = super::state::VerifiedFacts::from_direct_verification(
            DirectVerificationSeal(()),
            dependency_cut,
            payload,
            context,
            metrics,
        );
        drop(accepted_reads);
        let work = DirectAdmissionWork::new(tx, verified)
            .map_err(|_| DirectComputationError::InvalidEvidence)?;
        let cache_update = outcome
            .executed_proof()
            .map(|proof| VerificationCacheUpdate { proof });
        Ok(DirectVerificationOutcome::Candidate(
            DirectVerifiedCandidate {
                command,
                work,
                cache_update,
            },
        ))
    }
}
