//! Tx-pool-only resolution against one immutable chain/Accepted-state cut.
//!
//! The consensus resolver remains the single implementation of CKB cell and
//! dep-group semantics. This module contributes only the role-specific
//! Accepted overlay and a complete, bounded missing-frontier pass needed by
//! transaction relay. The overlay is an owned read receipt for one checked-out
//! capability; it is never a second membership authority or a persistent
//! cache.

use super::{
    plan::TxPoolAuthority,
    state::{DependencyKey, OwnedTx, PayloadPolicy, RawTxHash, VerifyCycleClass},
    validation::{proposal_status, verification_environment},
    work::{
        ComputeSettlement, ContinuousResolution, ContinuousResolveWork, ContinuousVerifyWork,
        ReceiptFailure, ResolutionEvidence, ResolutionReceiptError, ResolveWork,
        VerificationReceiptError, VerifyWork,
    },
};
use crate::{
    component::entry::resolved_transaction_charge_bytes,
    error::Reject,
    util::{check_tx_fee_with_min_fee_rate, compact_packed, verify_rtx},
};
use ckb_script::{ChunkCommand, TxVerifyEnv};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        DepType, FeeRate, TransactionView,
        cell::{
            CellMetaBuilder, CellProvider, CellStatus, HeaderChecker, OverlayCellProvider,
            ResolvedDep, ResolvedTransaction, SYSTEM_CELL, resolve_transaction_with_cell_providers,
        },
        error::OutPointError,
    },
    packed::{OutPoint, OutPointVec},
    prelude::{Entity, Unpack},
};
use ckb_verification::cache::{Completed, ScriptVerificationRules, TxVerificationCacheKey};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellRole {
    Input,
    Dependency,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MissingCell {
    out_point: OutPoint,
    role: CellRole,
}

#[derive(Debug)]
struct AcceptedOverlay {
    producers: HashMap<RawTxHash, Arc<TransactionView>>,
    spent_inputs: HashSet<OutPoint>,
}

impl AcceptedOverlay {
    fn capture(
        authority: &TxPoolAuthority,
        tx: &TransactionView,
        max_edges: usize,
    ) -> Result<Self, ResolutionExecutionKind> {
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
            spent_inputs: HashSet::new(),
        };
        overlay
            .producers
            .try_reserve(direct_edges)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        overlay
            .spent_inputs
            .try_reserve(tx.inputs().len())
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;

        for out_point in tx.input_pts_iter() {
            overlay.capture_cell(authority, &out_point, CellRole::Input);
        }
        for cell_dep in tx.cell_deps_iter() {
            overlay.capture_cell(authority, &cell_dep.out_point(), CellRole::Dependency);
        }
        Ok(overlay)
    }

    fn reserve_enrichment(&mut self, missing_count: usize) -> Result<(), ResolutionExecutionKind> {
        self.producers
            .try_reserve(missing_count)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        self.spent_inputs
            .try_reserve(missing_count)
            .map_err(|_| ResolutionExecutionKind::ResourceUnavailable)?;
        Ok(())
    }

    /// Observe only the transaction-bounded Accepted cut. Capacity was
    /// reserved before the authority guard was acquired, so this read cannot
    /// allocate while it blocks an authoritative writer.
    fn observe_enrichment(&mut self, authority: &TxPoolAuthority, missing: &[MissingCell]) -> bool {
        let before_producers = self.producers.len();
        let before_spends = self.spent_inputs.len();
        for cell in missing {
            self.capture_cell(authority, &cell.out_point, cell.role);
        }
        self.producers.len() != before_producers || self.spent_inputs.len() != before_spends
    }

    fn capture_cell(&mut self, authority: &TxPoolAuthority, out_point: &OutPoint, role: CellRole) {
        if role == CellRole::Input && authority.accepted_spender(out_point).is_some() {
            self.spent_inputs.insert(compact_packed(out_point));
        }
        let hash = RawTxHash(compact_packed(&out_point.tx_hash()));
        if self.producers.contains_key(&hash) {
            return;
        }
        let Some(OwnedTx::Accepted(entry)) = authority.entry(&hash) else {
            return;
        };
        let index: u32 = out_point.index().unpack();
        let Some(index) = usize::try_from(index).ok() else {
            return;
        };
        if index < entry.record.tx.outputs().len() {
            self.producers.insert(hash, Arc::clone(&entry.record.tx));
        }
    }

    fn is_spent(&self, out_point: &OutPoint) -> bool {
        self.spent_inputs.contains(out_point)
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
        let Some(tx) = self.overlay.producers.get(&hash) else {
            return CellStatus::Unknown;
        };
        let index: u32 = out_point.index().unpack();
        let Some(index) = usize::try_from(index).ok() else {
            return CellStatus::Unknown;
        };
        let Some((output, data)) = tx.output_with_data(index) else {
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
            Self::Resolve(work) => work.resolution_grant().max_edges,
            Self::Continuous(work) => work.resolution_grant().max_edges,
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

#[derive(Debug)]
#[must_use = "a missing resolution must be enriched or settled"]
pub(super) struct ResolutionProbe {
    job: ResolutionJob,
    missing: Vec<MissingCell>,
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
    InvalidReceipt(ResolutionReceiptError),
}

#[derive(Debug)]
#[must_use = "a failed execution still owns the exact retry settlement"]
pub(super) struct ResolutionExecutionFailure {
    kind: ResolutionExecutionKind,
    settlement: ComputeSettlement,
}

impl ResolutionExecutionFailure {
    fn from_resolution_receipt(failure: ReceiptFailure<ResolutionReceiptError>) -> Self {
        let kind = ResolutionExecutionKind::InvalidReceipt(*failure.error());
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

impl ResolutionJob {
    pub(super) fn capture_resolve(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: ResolveWork,
    ) -> Result<Self, ResolutionExecutionFailure> {
        Self::capture(authority, snapshot, ResolveLeaseWork::Resolve(work))
    }

    pub(super) fn capture_continuous(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: ContinuousResolveWork,
    ) -> Result<Self, ResolutionExecutionFailure> {
        Self::capture(authority, snapshot, ResolveLeaseWork::Continuous(work))
    }

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

    pub(super) fn evaluate(
        self,
        min_fee_rate: FeeRate,
        large_cycle_threshold: u64,
    ) -> Result<ResolutionEvaluation, ResolutionExecutionFailure> {
        let has_pool_conflict = self
            .work
            .transaction()
            .input_pts_iter()
            .any(|out_point| self.overlay.is_spent(&out_point));
        let strict = self.resolve(false);
        let resolved = match strict {
            Ok(resolved) => resolved,
            Err(OutPointError::Dead(out_point)) if self.overlay.is_spent(&out_point) => {
                match self.resolve(true) {
                    Ok(resolved) => resolved,
                    Err(OutPointError::Unknown(_)) => return self.missing_probe(true),
                    Err(error) => {
                        return Ok(ResolutionEvaluation::Settle(
                            self.work.rejected(Reject::Resolve(error)),
                        ));
                    }
                }
            }
            // The consensus resolver stops on the first missing input. A
            // later input may already be spent by Accepted membership, so an
            // `Unknown` result does not prove this is a non-RBF orphan. The
            // bounded overlay knows the complete direct input set and makes
            // that distinction without a second authority read.
            Err(OutPointError::Unknown(_)) if has_pool_conflict => match self.resolve(true) {
                Ok(resolved) => resolved,
                Err(OutPointError::Unknown(_)) => return self.missing_probe(true),
                Err(error) => {
                    return Ok(ResolutionEvaluation::Settle(
                        self.work.rejected(Reject::Resolve(error)),
                    ));
                }
            },
            Err(OutPointError::Unknown(_)) => return self.missing_probe(false),
            Err(error) => {
                return Ok(ResolutionEvaluation::Settle(
                    self.work.rejected(Reject::Resolve(error)),
                ));
            }
        };

        let tx_size = self.work.transaction().data().serialized_size_in_block();
        let resolved =
            crate::resolved_tx::compact_resolved_transaction_for_residency(Arc::new(resolved));
        let fee = match check_tx_fee_with_min_fee_rate(
            &self.snapshot,
            &resolved,
            tx_size,
            min_fee_rate,
        ) {
            Ok(fee) => fee,
            Err(reject) => {
                return Ok(ResolutionEvaluation::Settle(self.work.rejected(reject)));
            }
        };
        let verify_class = match self.work.payload_policy() {
            PayloadPolicy::RemoteDeclaredCycles(cycles) if cycles > large_cycle_threshold => {
                VerifyCycleClass::Large
            }
            PayloadPolicy::RemoteDeclaredCycles(_) | PayloadPolicy::Trusted => {
                VerifyCycleClass::Small
            }
        };
        let resident_bytes = resolved_transaction_charge_bytes(tx_size, &resolved);
        self.work.resolved(
            ResolutionEvidence::new(resolved, fee, resident_bytes, verify_class),
            self.snapshot,
        )
    }

    /// Cancellation before evaluation is an ordinary retry of the exact
    /// checked-out capability; it cannot leave the owner in `Computing`.
    pub(super) fn retry(self) -> ComputeSettlement {
        self.work.retry()
    }

    fn resolve(&self, permissive_inputs: bool) -> Result<ResolvedTransaction, OutPointError> {
        let input_overlay = SparsePoolCellProvider {
            overlay: &self.overlay,
            observe_spends: !permissive_inputs,
        };
        let dependency_overlay = SparsePoolCellProvider {
            overlay: &self.overlay,
            observe_spends: false,
        };
        let input_provider = OverlayCellProvider::new(&input_overlay, self.snapshot.as_ref());
        let dependency_provider =
            OverlayCellProvider::new(&dependency_overlay, self.snapshot.as_ref());
        let mut seen_inputs = HashSet::with_capacity(self.work.transaction().inputs().len());
        resolve_transaction_with_cell_providers(
            self.work.transaction().clone(),
            &mut seen_inputs,
            &input_provider,
            &dependency_provider,
            self.snapshot.as_ref(),
        )
    }

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

    fn collect_missing(
        &self,
        permissive_inputs: bool,
    ) -> Result<Vec<MissingCell>, MissingScanError> {
        let tx = self.work.transaction();
        let max_edges = self.work.grant_edges();
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
            overlay: &self.overlay,
            observe_spends: !permissive_inputs,
        };
        let dependency_overlay = SparsePoolCellProvider {
            overlay: &self.overlay,
            observe_spends: false,
        };
        let input_provider = OverlayCellProvider::new(&input_overlay, self.snapshot.as_ref());
        let dependency_provider =
            OverlayCellProvider::new(&dependency_overlay, self.snapshot.as_ref());

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
                let cached_edges = match SYSTEM_CELL.get().and_then(|system| system.get(&cell_dep))
                {
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
                collect_cell_status(direct, out_point, CellRole::Dependency, &mut missing)?;
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
                    CellRole::Dependency,
                    &mut missing,
                )?;
            }
        }

        for header in tx.header_deps_iter() {
            if let Err(error) = self.snapshot.check_valid(&header) {
                return Err(MissingScanError::Reject(error));
            }
        }
        if missing.is_empty() {
            return Err(MissingScanError::ResourceUnavailable);
        }
        missing.sort_unstable_by(|left, right| {
            left.out_point
                .cmp(&right.out_point)
                .then_with(|| (left.role as u8).cmp(&(right.role as u8)))
        });
        missing.dedup();
        Ok(missing)
    }
}

impl ResolutionProbe {
    /// Reserve fallible collection growth before the authority read cut. A
    /// failure retains the exact settlement capability for an ordinary retry.
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
    pub(super) fn settle_missing(self) -> Result<ComputeSettlement, ResolutionExecutionFailure> {
        let keys = self
            .missing
            .into_iter()
            .map(|cell| DependencyKey::Cell(compact_packed(&cell.out_point)))
            .collect();
        self.job.work.missing(keys)
    }

    #[cfg(test)]
    pub(super) fn missing_keys_for_foundation(&self) -> Vec<DependencyKey> {
        self.missing
            .iter()
            .map(|cell| DependencyKey::Cell(cell.out_point.clone()))
            .collect()
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
    missing: &mut Vec<MissingCell>,
) -> Result<(), MissingScanError> {
    match status {
        CellStatus::Live(_) => Ok(()),
        CellStatus::Dead => Err(MissingScanError::Reject(OutPointError::Dead(out_point))),
        CellStatus::Unknown => {
            missing.push(MissingCell { out_point, role });
            Ok(())
        }
    }
}

#[derive(Debug)]
enum VerifyLeaseWork {
    Verify(VerifyWork),
    Continuous(ContinuousVerifyWork),
}

impl VerifyLeaseWork {
    fn transaction(&self) -> &TransactionView {
        match self {
            Self::Verify(work) => work.transaction(),
            Self::Continuous(work) => work.transaction(),
        }
    }

    fn resolved_transaction(&self) -> &Arc<ResolvedTransaction> {
        match self {
            Self::Verify(work) => work.resolved_transaction(),
            Self::Continuous(work) => work.resolved_transaction(),
        }
    }

    fn payload_policy(&self) -> PayloadPolicy {
        match self {
            Self::Verify(work) => work.payload_policy(),
            Self::Continuous(work) => work.payload_policy(),
        }
    }

    fn chain_view(&self) -> &super::state::ChainViewId {
        match self {
            Self::Verify(work) => work.chain_view(),
            Self::Continuous(work) => work.chain_view(),
        }
    }

    fn verified(
        self,
        cycles: u64,
        time: super::chain::TimeContextReceipt,
    ) -> Result<ComputeSettlement, super::work::ReceiptFailure<super::work::VerificationReceiptError>>
    {
        match self {
            Self::Verify(work) => work.verified_with_time_context(cycles, time),
            Self::Continuous(work) => work.verified_with_time_context(cycles, time),
        }
    }

    fn rejected(self, reason: Reject) -> ComputeSettlement {
        match self {
            Self::Verify(work) => work.rejected(reason),
            Self::Continuous(work) => work.rejected(reason),
        }
    }

    fn retry(self) -> ComputeSettlement {
        match self {
            Self::Verify(work) => work.internal_failure(),
            Self::Continuous(work) => work.internal_failure(),
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
    work: VerifyLeaseWork,
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
}

#[derive(Debug)]
pub(crate) struct VerificationCacheUpdate {
    pub(crate) key: TxVerificationCacheKey,
    pub(crate) completed: Completed,
}

#[derive(Debug)]
#[must_use = "verification completion must be settled and its optional cache effect published"]
pub(in crate::authority) struct VerificationExecution {
    pub(in crate::authority) settlement: ComputeSettlement,
    pub(in crate::authority) cache_update: Option<VerificationCacheUpdate>,
    pub(in crate::authority) cache_hit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerificationExecutionKind {
    InvalidReceipt(VerificationReceiptError),
}

#[derive(Debug)]
#[must_use = "failed verification execution still owns the exact retry settlement"]
pub(in crate::authority) struct VerificationExecutionFailure {
    kind: VerificationExecutionKind,
    settlement: ComputeSettlement,
}

impl VerificationExecutionFailure {
    pub(in crate::authority) fn kind(&self) -> VerificationExecutionKind {
        self.kind
    }

    pub(in crate::authority) fn into_settlement(self) -> ComputeSettlement {
        self.settlement
    }
}

impl VerificationJob {
    pub(super) fn from_checkout(
        work: VerifyWork,
        snapshot: Arc<Snapshot>,
    ) -> Result<Self, ResolutionExecutionFailure> {
        let work = VerifyLeaseWork::Verify(work);
        if snapshot.tip_hash() != work.chain_view().tip().0 {
            return Err(ResolutionExecutionFailure {
                kind: ResolutionExecutionKind::StaleView,
                settlement: work.retry(),
            });
        }
        Ok(Self { work, snapshot })
    }

    fn from_continuation(work: ContinuousVerifyWork, snapshot: Arc<Snapshot>) -> Self {
        Self {
            work: VerifyLeaseWork::Continuous(work),
            snapshot,
        }
    }

    pub(super) fn transaction(&self) -> &TransactionView {
        self.work.transaction()
    }

    pub(super) fn resolved_transaction(&self) -> &Arc<ResolvedTransaction> {
        self.work.resolved_transaction()
    }

    pub(super) fn snapshot(&self) -> &Arc<Snapshot> {
        &self.snapshot
    }

    pub(super) fn payload_policy(&self) -> PayloadPolicy {
        self.work.payload_policy()
    }

    pub(super) fn prepare(self) -> TxPoolVerificationRequest {
        let status = proposal_status(&self.snapshot, &self.transaction().proposal_short_id());
        let environment = Arc::new(verification_environment(status, &self.snapshot));
        let rules = ScriptVerificationRules::from_env(self.snapshot.consensus(), &environment);
        let cache_key = TxVerificationCacheKey::from_transaction(self.transaction(), rules);
        let max_cycles = match self.payload_policy() {
            PayloadPolicy::RemoteDeclaredCycles(cycles) => cycles,
            PayloadPolicy::Trusted => self.snapshot.consensus().max_block_cycles(),
        };
        TxPoolVerificationRequest {
            job: self,
            environment,
            cache_key,
            max_cycles,
        }
    }

    pub(super) fn verified(
        self,
        cycles: u64,
        rules: ckb_verification::cache::ScriptVerificationRules,
    ) -> Result<ComputeSettlement, super::work::ReceiptFailure<super::work::VerificationReceiptError>>
    {
        self.work.verified(
            cycles,
            super::chain::TimeContextReceipt::from_validation(rules),
        )
    }

    pub(super) fn rejected(self, reason: Reject) -> ComputeSettlement {
        self.work.rejected(reason)
    }

    pub(super) fn retry(self) -> ComputeSettlement {
        self.work.retry()
    }
}

impl TxPoolVerificationRequest {
    pub(crate) fn cache_key(&self) -> &TxVerificationCacheKey {
        &self.cache_key
    }

    pub(in crate::authority) async fn execute(
        self,
        cache_entry: Option<Completed>,
        command_rx: Option<&mut watch::Receiver<ChunkCommand>>,
    ) -> Result<VerificationExecution, VerificationExecutionFailure> {
        let Self {
            job,
            environment,
            cache_key,
            max_cycles,
        } = self;
        let VerificationJob { work, snapshot } = job;
        let policy = work.payload_policy();
        let resolved = Arc::clone(work.resolved_transaction());
        let verified = verify_rtx(
            snapshot,
            resolved,
            environment,
            &cache_entry,
            max_cycles,
            command_rx,
        )
        .await;
        let completed = match verified {
            Ok(completed) => completed,
            Err(reject) => {
                return Ok(VerificationExecution {
                    settlement: work.rejected(reject),
                    cache_update: None,
                    cache_hit: cache_entry.is_some(),
                });
            }
        };
        let policy_accepts_cycles = match policy {
            PayloadPolicy::RemoteDeclaredCycles(declared) => declared == completed.cycles,
            PayloadPolicy::Trusted => true,
        };
        let settlement = match work.verified(
            completed.cycles,
            super::chain::TimeContextReceipt::from_validation(cache_key.script_rules()),
        ) {
            Ok(settlement) => settlement,
            Err(failure) => {
                let kind = VerificationExecutionKind::InvalidReceipt(*failure.error());
                return Err(VerificationExecutionFailure {
                    kind,
                    settlement: failure.into_settlement(),
                });
            }
        };
        let cache_update =
            (cache_entry.is_none() && policy_accepts_cycles).then_some(VerificationCacheUpdate {
                key: cache_key,
                completed,
            });
        Ok(VerificationExecution {
            settlement,
            cache_update,
            cache_hit: cache_entry.is_some(),
        })
    }
}
