//! Lock-external validation for resolved tx-pool candidates.
//!
//! The authority guard captures only a transaction-bounded projection of
//! Accepted producers. Snapshot reads, location refresh, time/DAO validation,
//! and payload destruction happen after the guard is released. Accepted
//! spenders are intentionally absent: current conflict and RBF policy remain
//! decisions of the single membership Plan, not a duplicated validation
//! authority.

use super::{
    chain::{
        AcceptedChainSensitivity, CellLocationReceiptError, DirectAdmissionReceipt,
        DirectAdmissionRejection, DirectAdmissionRetry, DirectAdmissionSubject,
        DirectAdmissionWork, FinalAdmissionReceipt, FinalAdmissionRejection, FinalAdmissionRetry,
        FinalAdmissionSubject, FinalAdmissionWork, MembershipReceipt, MembershipValidationWork,
        ReadyPayloadRelation, TimeContextReceipt, VerificationContextReceipt,
        proposal_context_receipt,
    },
    plan::TxPoolAuthority,
    rejection::CommittedPublicReject,
    resolver::AcceptedOverlay,
    runtime::AuthorityStoreCaptureSeal,
    state::{AcceptedAtMillis, AcceptedStatus, RawTxHash, ResolvedPayload},
};
use crate::{
    constants::GAP_PROPOSAL_INDEX,
    error::Reject,
    util::{block_offload, check_tx_fee_with_min_fee_rate, revalidate_tx_context},
};
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        FeeRate, TransactionInfo,
        cell::{CellMeta, CellProvider, CellStatus, HeaderChecker, ResolvedTransaction},
    },
    packed::OutPoint,
};
use ckb_verification::cache::ScriptVerificationRules;
use std::sync::Arc;

/// Capability proving that only tip-relative `CellMeta::transaction_info`
/// fields were refreshed. Its field is private to this module; transaction
/// content cannot be substituted through the location-refresh API.
#[derive(Clone, Copy)]
pub(super) struct LocationRefreshSeal(());

/// Construction capability for every final-admission outcome. Keeping the
/// field private to this module prevents sibling modules from hand-stamping a
/// membership receipt, rejection, or typed re-resolution without running this
/// validator.
#[derive(Clone, Copy)]
pub(super) struct AdmissionValidationSeal(());

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "every validated candidate outcome needs one authoritative disposition"]
pub(super) enum FinalAdmissionValidationOutcome {
    Candidate(FinalAdmissionReceipt),
    Rejected(FinalAdmissionRejection),
    Reresolve(FinalAdmissionRetry),
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "every direct validation outcome needs a local or read-only disposition"]
pub(super) enum DirectAdmissionValidationOutcome {
    Candidate(DirectAdmissionReceipt),
    Rejected(DirectAdmissionRejection),
    Reresolve(DirectAdmissionRetry),
}

#[derive(Debug)]
pub(super) enum FinalAdmissionValidationError {
    StaleView,
    Allocation,
    Arithmetic,
    MissingChainLocation(
        #[expect(
            dead_code,
            reason = "the exact outpoint is retained for structural-fault diagnostics"
        )]
        OutPoint,
    ),
    CellContentMismatch(
        #[expect(
            dead_code,
            reason = "the exact outpoint is retained for structural-fault diagnostics"
        )]
        OutPoint,
    ),
    ContextReceipt,
}

/// Private helper result: a transaction-level rejection cannot cross the
/// validator boundary as a bare error and must become a sealed disposition.
enum CandidateValidationError {
    Rejected(Reject),
    Fault(FinalAdmissionValidationError),
}

impl From<FinalAdmissionValidationError> for CandidateValidationError {
    fn from(error: FinalAdmissionValidationError) -> Self {
        Self::Fault(error)
    }
}

fn prepare_accepted_overlay(
    payload: &ResolvedPayload,
) -> Result<AcceptedOverlay, FinalAdmissionValidationError> {
    AcceptedOverlay::prepare_resolved(payload).map_err(|error| match error {
        CellLocationReceiptError::Allocation => FinalAdmissionValidationError::Allocation,
        CellLocationReceiptError::Arithmetic => FinalAdmissionValidationError::Arithmetic,
    })
}

/// A complete lock-external final-admission validation job.
///
/// `work`, `snapshot`, and `overlay` are captured from one `AuthorityStore`
/// guard. No raw liveness Boolean or proposal status crosses this boundary.
#[must_use = "final admission validation must produce a receipt or a typed outcome"]
pub(super) struct FinalAdmissionValidation {
    work: FinalAdmissionWork,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
    min_fee_rate: FeeRate,
}

/// A complete lock-external validation job shared by synchronous Local and
/// read-only TestAccept. It owns no authority mutation or publication token.
#[must_use = "direct validation must produce an immutable evaluation outcome"]
pub(super) struct DirectAdmissionValidation {
    work: DirectAdmissionWork,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
    dependency_cut: super::state::DependencyCut,
    min_fee_rate: FeeRate,
}

/// First half of an OCC capture. The candidate identifies how much overlay
/// storage is needed, allocation happens with no authority guard, and a
/// second read cut revalidates the exact Ready version before populating it.
#[must_use = "prepared validation must be completed against the rechecked authority cut"]
pub(super) struct PreparedFinalAdmissionValidation {
    key: RawTxHash,
    expected: super::state::EntryVersion,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
    min_fee_rate: FeeRate,
}

/// Preallocated direct-validation capture. The resolved transaction defines
/// the bounded overlay size outside the authority guard; completion fills the
/// bits from one coherent authority/snapshot cut.
#[must_use = "prepared direct validation must complete against the authority cut"]
pub(super) struct PreparedDirectAdmissionValidation {
    work: DirectAdmissionWork,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
    min_fee_rate: FeeRate,
}

enum MembershipValidationOutcome {
    Candidate {
        membership: MembershipReceipt,
        payload_relation: ReadyPayloadRelation,
    },
    Rejected {
        reason: CommittedPublicReject,
        accepted_reads: AcceptedOverlay,
    },
    Reresolve,
}

impl PreparedFinalAdmissionValidation {
    pub(super) fn key(&self) -> &RawTxHash {
        &self.key
    }

    pub(super) fn expected(&self) -> super::state::EntryVersion {
        self.expected
    }

    pub(super) fn complete(
        self,
        _seal: AuthorityStoreCaptureSeal,
        authority: &TxPoolAuthority,
        work: FinalAdmissionWork,
    ) -> Result<FinalAdmissionValidation, FinalAdmissionValidationError> {
        self.complete_inner(authority, work)
    }

    fn complete_inner(
        mut self,
        authority: &TxPoolAuthority,
        work: FinalAdmissionWork,
    ) -> Result<FinalAdmissionValidation, FinalAdmissionValidationError> {
        if work.key() != &self.key
            || work.expected() != self.expected
            || authority.chain_view() != work.view()
            || self.snapshot.tip_hash() != work.view().tip().0
        {
            return Err(FinalAdmissionValidationError::StaleView);
        }
        self.overlay.populate(authority);
        Ok(FinalAdmissionValidation {
            work,
            snapshot: self.snapshot,
            overlay: self.overlay,
            min_fee_rate: self.min_fee_rate,
        })
    }
}

impl PreparedDirectAdmissionValidation {
    pub(super) fn complete(
        self,
        _seal: AuthorityStoreCaptureSeal,
        authority: &TxPoolAuthority,
    ) -> Result<DirectAdmissionValidation, FinalAdmissionValidationError> {
        self.complete_inner(authority)
    }

    fn complete_inner(
        mut self,
        authority: &TxPoolAuthority,
    ) -> Result<DirectAdmissionValidation, FinalAdmissionValidationError> {
        if authority.chain_view() != self.work.view()
            || self.snapshot.tip_hash() != self.work.view().tip().0
        {
            return Err(FinalAdmissionValidationError::StaleView);
        }
        self.overlay.populate(authority);
        Ok(DirectAdmissionValidation {
            work: self.work,
            snapshot: self.snapshot,
            overlay: self.overlay,
            dependency_cut: authority.dependency_observation_cut(),
            min_fee_rate: self.min_fee_rate,
        })
    }
}

impl FinalAdmissionValidation {
    pub(super) fn prepare(
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
        min_fee_rate: FeeRate,
    ) -> Result<PreparedFinalAdmissionValidation, FinalAdmissionValidationError> {
        if snapshot.tip_hash() != work.view().tip().0 {
            return Err(FinalAdmissionValidationError::StaleView);
        }
        let overlay = prepare_accepted_overlay(work.payload())?;
        Ok(PreparedFinalAdmissionValidation {
            key: work.key().clone(),
            expected: work.expected(),
            snapshot,
            overlay,
            min_fee_rate,
        })
    }

    /// Validate location, time, DAO, proposal position, and script-rule reuse
    /// without holding the authority guard.
    pub(super) fn validate(
        self,
    ) -> Result<FinalAdmissionValidationOutcome, FinalAdmissionValidationError> {
        let Self {
            work,
            snapshot,
            overlay,
            min_fee_rate,
        } = self;
        let (key, expected, validation) = work.into_validation_parts();
        let view = validation.view().clone();
        let dependency_cut = validation.dependency_cut();
        let seal = AdmissionValidationSeal(());
        let subject = FinalAdmissionSubject::new(seal, key, expected, view, dependency_cut);
        match validate_membership(validation, snapshot, overlay, min_fee_rate, seal)? {
            MembershipValidationOutcome::Candidate {
                membership,
                payload_relation,
            } => Ok(FinalAdmissionValidationOutcome::Candidate(
                FinalAdmissionReceipt::from_validation(
                    seal,
                    expected,
                    membership,
                    payload_relation,
                ),
            )),
            MembershipValidationOutcome::Rejected {
                reason,
                accepted_reads,
            } => {
                drop(accepted_reads);
                Ok(FinalAdmissionValidationOutcome::Rejected(
                    FinalAdmissionRejection::new(seal, subject, reason),
                ))
            }
            MembershipValidationOutcome::Reresolve => Ok(
                FinalAdmissionValidationOutcome::Reresolve(FinalAdmissionRetry::new(seal, subject)),
            ),
        }
    }
}

impl DirectAdmissionValidation {
    pub(super) fn prepare(
        snapshot: Arc<Snapshot>,
        work: DirectAdmissionWork,
        min_fee_rate: FeeRate,
    ) -> Result<PreparedDirectAdmissionValidation, FinalAdmissionValidationError> {
        if snapshot.tip_hash() != work.view().tip().0 {
            return Err(FinalAdmissionValidationError::StaleView);
        }
        let overlay = prepare_accepted_overlay(work.payload())?;
        Ok(PreparedDirectAdmissionValidation {
            work,
            snapshot,
            overlay,
            min_fee_rate,
        })
    }

    pub(super) fn validate(
        self,
    ) -> Result<DirectAdmissionValidationOutcome, FinalAdmissionValidationError> {
        let Self {
            work,
            snapshot,
            overlay,
            dependency_cut,
            min_fee_rate,
        } = self;
        let (tx, validation) = work.into_validation_parts();
        let seal = AdmissionValidationSeal(());
        let validation = validation.with_validated_dependency_cut(seal, dependency_cut);
        let view = validation.view().clone();
        match validate_membership(validation, snapshot, overlay, min_fee_rate, seal)? {
            MembershipValidationOutcome::Candidate { membership, .. } => {
                Ok(DirectAdmissionValidationOutcome::Candidate(
                    DirectAdmissionReceipt::from_validation(seal, tx, membership),
                ))
            }
            MembershipValidationOutcome::Rejected {
                reason,
                accepted_reads,
            } => {
                let subject =
                    DirectAdmissionSubject::new(seal, Arc::clone(&tx), view, accepted_reads);
                Ok(DirectAdmissionValidationOutcome::Rejected(
                    DirectAdmissionRejection::new(seal, subject, reason),
                ))
            }
            MembershipValidationOutcome::Reresolve => Ok(
                DirectAdmissionValidationOutcome::Reresolve(DirectAdmissionRetry::new(seal, tx)),
            ),
        }
    }
}

#[cfg(test)]
#[path = "tests/support/validation.rs"]
mod test_support;

fn validate_membership(
    validation: MembershipValidationWork,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOverlay,
    min_fee_rate: FeeRate,
    seal: AdmissionValidationSeal,
) -> Result<MembershipValidationOutcome, FinalAdmissionValidationError> {
    let (view, verified) = validation.into_parts();
    if snapshot.tip_hash() != view.tip().0 {
        return Err(FinalAdmissionValidationError::StaleView);
    }
    let proposal = proposal_context_receipt(&snapshot, &verified.payload().identity().proposal.0);
    let status = proposal.status();
    let environment = verification_environment(status, &snapshot);
    let rules = ScriptVerificationRules::from_env(snapshot.consensus(), &environment);
    if verified.verification_context().rules() != rules {
        // Successful Verify compacts dependency scripts/data. A rules
        // transition cannot honestly reuse that payload for another VM run.
        return Ok(MembershipValidationOutcome::Reresolve);
    }

    let same_chain_state = verified.chain_view().has_same_chain_state(&view);
    let location_result = if same_chain_state {
        refresh_locations(
            verified.payload_arc(),
            true,
            &snapshot,
            &overlay,
            min_fee_rate,
        )
    } else {
        // Header/cell reads can hit RocksDB. No authority guard is held while
        // the complete changed-tip lookup slice runs off the async executor.
        block_offload(|| {
            validate_header_dependencies(verified.payload(), &snapshot)?;
            refresh_locations(
                verified.payload_arc(),
                false,
                &snapshot,
                &overlay,
                min_fee_rate,
            )
        })
    };
    let (payload, payload_relation) = match location_result {
        Ok(value) => value,
        Err(CandidateValidationError::Rejected(reason)) => {
            return Ok(MembershipValidationOutcome::Rejected {
                reason: CommittedPublicReject::new(reason),
                accepted_reads: overlay,
            });
        }
        Err(CandidateValidationError::Fault(error)) => return Err(error),
    };
    let context_is_reusable =
        payload_relation == ReadyPayloadRelation::Shared && verified.context_is_for(&view);
    if !context_is_reusable
        && let Err(reason) = revalidate_tx_context(
            Arc::clone(&snapshot),
            Arc::clone(payload.resolved_transaction()),
            Arc::new(environment),
        )
    {
        return Ok(MembershipValidationOutcome::Rejected {
            reason: CommittedPublicReject::new(reason),
            accepted_reads: overlay,
        });
    }

    let location =
        super::chain::CellLocationReceipt::from_resolution(view, &payload).map_err(|error| {
            match error {
                CellLocationReceiptError::Allocation => FinalAdmissionValidationError::Allocation,
                CellLocationReceiptError::Arithmetic => FinalAdmissionValidationError::Arithmetic,
            }
        })?;
    let context = VerificationContextReceipt::from_validation(
        location,
        TimeContextReceipt::from_validation(rules),
    );
    let sensitivity = chain_sensitivity(payload.resolved_transaction());
    let verified = verified
        .with_final_validation(LocationRefreshSeal(()), payload, context)
        .ok_or(FinalAdmissionValidationError::ContextReceipt)?;
    let membership = MembershipReceipt::from_validation(
        seal,
        verified,
        sensitivity,
        proposal,
        AcceptedAtMillis(ckb_systemtime::unix_time_as_millis()),
    );
    Ok(MembershipValidationOutcome::Candidate {
        membership,
        payload_relation,
    })
}

fn validate_header_dependencies(
    payload: &ResolvedPayload,
    snapshot: &Snapshot,
) -> Result<(), CandidateValidationError> {
    for block_hash in payload
        .resolved_transaction()
        .transaction
        .header_deps_iter()
    {
        snapshot
            .check_valid(&block_hash)
            .map_err(|error| CandidateValidationError::Rejected(Reject::Resolve(error)))?;
    }
    Ok(())
}

pub(super) fn proposal_status(
    snapshot: &Snapshot,
    proposal: &ckb_types::packed::ProposalShortId,
) -> AcceptedStatus {
    proposal_context_receipt(snapshot, proposal).status()
}

pub(super) fn verification_environment(status: AcceptedStatus, snapshot: &Snapshot) -> TxVerifyEnv {
    let header = snapshot.tip_header();
    match status {
        AcceptedStatus::Pending => TxVerifyEnv::new_submit(header),
        AcceptedStatus::Gap => TxVerifyEnv::new_proposed(header, GAP_PROPOSAL_INDEX),
        // Proposed proves that the next block may commit the transaction. The
        // age coordinate is closest - 1, not a default-window constant.
        AcceptedStatus::Proposed => TxVerifyEnv::new_proposed(
            header,
            snapshot
                .consensus()
                .tx_proposal_window()
                .closest()
                .saturating_sub(1),
        ),
    }
}

pub(super) fn chain_sensitivity(resolved: &ResolvedTransaction) -> AcceptedChainSensitivity {
    let has_since = resolved
        .transaction
        .inputs()
        .into_iter()
        .any(|input| Into::<u64>::into(input.since()) != 0);
    let has_chain_cellbase = resolved
        .resolved_inputs
        .iter()
        .chain(&resolved.resolved_cell_deps)
        .any(|cell| {
            cell.transaction_info
                .as_ref()
                .is_some_and(|info| info.block_number > 0 && info.is_cellbase())
        });
    if has_since || has_chain_cellbase {
        AcceptedChainSensitivity::TipContext
    } else {
        AcceptedChainSensitivity::Stable
    }
}

fn refresh_locations(
    payload: &Arc<ResolvedPayload>,
    same_chain_state: bool,
    snapshot: &Snapshot,
    overlay: &AcceptedOverlay,
    min_fee_rate: FeeRate,
) -> Result<(Arc<ResolvedPayload>, ReadyPayloadRelation), CandidateValidationError> {
    let resolved = payload.resolved_transaction();
    let total_cells = resolved
        .resolved_inputs
        .len()
        .checked_add(resolved.resolved_cell_deps.len())
        .and_then(|count| count.checked_add(resolved.resolved_dep_groups.len()))
        .ok_or(FinalAdmissionValidationError::Arithmetic)?;
    let mut changes = Vec::new();

    for (role, cells) in [
        (ResolvedCellRole::Input, resolved.resolved_inputs.as_slice()),
        (
            ResolvedCellRole::Dependency,
            resolved.resolved_cell_deps.as_slice(),
        ),
        (
            ResolvedCellRole::DependencyGroup,
            resolved.resolved_dep_groups.as_slice(),
        ),
    ] {
        for (index, cell) in cells.iter().enumerate() {
            let pool_origin = overlay.is_accepted_output(&cell.out_point);
            let current = current_location(cell, role, pool_origin, same_chain_state, snapshot)?;
            if current != cell.transaction_info {
                if changes.is_empty() {
                    changes
                        .try_reserve(total_cells)
                        .map_err(|_| FinalAdmissionValidationError::Allocation)?;
                }
                changes.push(LocationChange {
                    role,
                    index,
                    current,
                });
            }
        }
    }

    if changes.is_empty() {
        return Ok((Arc::clone(payload), ReadyPayloadRelation::Shared));
    }

    let mut refreshed = super::residency::try_clone_for_location_refresh(resolved)
        .map_err(|_| FinalAdmissionValidationError::Allocation)?;
    for change in changes {
        let cell = match change.role {
            ResolvedCellRole::Input => refreshed.resolved_inputs.get_mut(change.index),
            ResolvedCellRole::Dependency => refreshed.resolved_cell_deps.get_mut(change.index),
            ResolvedCellRole::DependencyGroup => {
                refreshed.resolved_dep_groups.get_mut(change.index)
            }
        }
        .ok_or(FinalAdmissionValidationError::ContextReceipt)?;
        cell.transaction_info = change.current;
    }
    let fee = check_tx_fee_with_min_fee_rate(
        snapshot,
        &refreshed,
        payload.serialized_bytes(),
        min_fee_rate,
    )
    .map_err(CandidateValidationError::Rejected)?;
    let payload =
        payload.with_refreshed_locations(LocationRefreshSeal(()), Arc::new(refreshed), fee);
    Ok((Arc::new(payload), ReadyPayloadRelation::LocationRefreshed))
}

fn current_location(
    previous: &CellMeta,
    role: ResolvedCellRole,
    pool_origin: bool,
    same_tip: bool,
    snapshot: &Snapshot,
) -> Result<Option<TransactionInfo>, CandidateValidationError> {
    if pool_origin {
        return Ok(None);
    }
    if same_tip {
        return previous.transaction_info.clone().map(Some).ok_or_else(|| {
            CandidateValidationError::Rejected(Reject::Resolve(
                ckb_types::core::error::OutPointError::Unknown(previous.out_point.clone()),
            ))
        });
    }
    let current = match snapshot.cell(&previous.out_point, false) {
        CellStatus::Live(current) => current,
        CellStatus::Dead => {
            return Err(CandidateValidationError::Rejected(Reject::Resolve(
                ckb_types::core::error::OutPointError::Dead(previous.out_point.clone()),
            )));
        }
        CellStatus::Unknown => {
            return Err(CandidateValidationError::Rejected(Reject::Resolve(
                ckb_types::core::error::OutPointError::Unknown(previous.out_point.clone()),
            )));
        }
    };
    // Successful Verify deliberately drops dependency scripts/data. Their
    // OutPoint commits to the producing transaction and output index, so a
    // live occurrence on another valid tip has identical immutable content;
    // only its transaction location may change. Inputs retain full payload
    // for DAO and keep the corruption-detection comparison below.
    if role == ResolvedCellRole::Input
        && (current.cell_output != previous.cell_output
            || current.data_bytes != previous.data_bytes)
    {
        return Err(
            FinalAdmissionValidationError::CellContentMismatch(previous.out_point.clone()).into(),
        );
    }
    current
        .transaction_info
        .ok_or_else(|| {
            FinalAdmissionValidationError::MissingChainLocation(previous.out_point.clone())
        })
        .map(Some)
        .map_err(Into::into)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolvedCellRole {
    Input,
    Dependency,
    DependencyGroup,
}

struct LocationChange {
    role: ResolvedCellRole,
    index: usize,
    current: Option<TransactionInfo>,
}
