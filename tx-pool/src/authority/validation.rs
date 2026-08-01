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
        AcceptedChainSensitivity, FinalAdmissionReceipt, FinalAdmissionRejection,
        FinalAdmissionRetry, FinalAdmissionSubject, FinalAdmissionWork, MembershipReceipt,
        ReadyPayloadRelation, TimeContextReceipt, VerificationContextReceipt,
    },
    plan::TxPoolAuthority,
    rejection::CommittedPublicReject,
    runtime::AuthorityStoreCaptureSeal,
    state::{AcceptedAtMillis, AcceptedStatus, OwnedTx, RawTxHash, ResolvedPayload},
};
use crate::{
    constants::{GAP_PROPOSAL_INDEX, PROPOSED_PROPOSAL_INDEX},
    error::Reject,
    util::{block_offload, revalidate_tx_context},
};
use ckb_script::TxVerifyEnv;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        TransactionInfo,
        cell::{CellMeta, CellProvider, CellStatus, HeaderChecker, ResolvedTransaction},
    },
    packed::OutPoint,
    prelude::Unpack,
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
pub(super) struct FinalAdmissionSeal(());

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "every validated candidate outcome needs one authoritative disposition"]
pub(super) enum FinalAdmissionValidationOutcome {
    Candidate(FinalAdmissionReceipt),
    Rejected(FinalAdmissionRejection),
    Reresolve(FinalAdmissionRetry),
}

#[derive(Debug)]
pub(super) enum FinalAdmissionValidationError {
    StaleView,
    Allocation,
    Arithmetic,
    MissingChainLocation(OutPoint),
    CellContentMismatch(OutPoint),
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

/// The exact Accepted-output projection needed by one candidate.
///
/// It is immutable, bounded by the resolved footprint, and has no independent
/// publication or invalidation protocol. Dependency cuts and final membership
/// OCC make a later owner change stale.
#[derive(Debug)]
struct AcceptedOriginOverlay {
    // One bit per resolved input/cell-dep/expanded dep-group cell, in that
    // exact order. This avoids cloning and sorting peer-controlled outpoints
    // while the authority guard is held.
    pool_origin: Vec<bool>,
}

impl AcceptedOriginOverlay {
    fn capture(
        authority: &TxPoolAuthority,
        payload: &ResolvedPayload,
    ) -> Result<Self, FinalAdmissionValidationError> {
        let resolved = payload.resolved_transaction();
        let total_cells = resolved
            .resolved_inputs
            .len()
            .checked_add(resolved.resolved_cell_deps.len())
            .and_then(|count| count.checked_add(resolved.resolved_dep_groups.len()))
            .ok_or(FinalAdmissionValidationError::Arithmetic)?;
        let mut pool_origin = Vec::new();
        pool_origin
            .try_reserve_exact(total_cells)
            .map_err(|_| FinalAdmissionValidationError::Allocation)?;
        for cell in resolved
            .resolved_inputs
            .iter()
            .chain(&resolved.resolved_cell_deps)
            .chain(&resolved.resolved_dep_groups)
        {
            pool_origin.push(is_accepted_output(authority, &cell.out_point));
        }
        Ok(Self { pool_origin })
    }

    fn origins(&self) -> impl Iterator<Item = bool> + '_ {
        self.pool_origin.iter().copied()
    }
}

fn is_accepted_output(authority: &TxPoolAuthority, out_point: &OutPoint) -> bool {
    let producer = RawTxHash(out_point.tx_hash());
    let Some(OwnedTx::Accepted(entry)) = authority.entry(&producer) else {
        return false;
    };
    let index: u32 = out_point.index().unpack();
    usize::try_from(index)
        .ok()
        .is_some_and(|index| index < entry.record.tx.outputs().len())
}

/// A complete lock-external final-admission validation job.
///
/// `work`, `snapshot`, and `overlay` are captured from one `AuthorityStore`
/// guard. No raw liveness Boolean or proposal status crosses this boundary.
#[must_use = "final admission validation must produce a receipt or a typed outcome"]
pub(super) struct FinalAdmissionValidation {
    work: FinalAdmissionWork,
    snapshot: Arc<Snapshot>,
    overlay: AcceptedOriginOverlay,
}

impl FinalAdmissionValidation {
    pub(super) fn capture(
        _seal: AuthorityStoreCaptureSeal,
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        Self::capture_inner(authority, snapshot, work)
    }

    #[cfg(test)]
    pub(super) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        Self::capture_inner(authority, snapshot, work)
    }

    fn capture_inner(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        if authority.chain_view() != work.view() || snapshot.tip_hash() != work.view().tip().0 {
            return Err(FinalAdmissionValidationError::StaleView);
        }
        let overlay = AcceptedOriginOverlay::capture(authority, work.payload())?;
        Ok(Self {
            work,
            snapshot,
            overlay,
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
        } = self;
        let (key, expected, validation) = work.into_validation_parts();
        let (view, verified) = validation.into_parts();
        if snapshot.tip_hash() != view.tip().0 {
            return Err(FinalAdmissionValidationError::StaleView);
        }
        let seal = FinalAdmissionSeal(());
        let subject = FinalAdmissionSubject::new(
            seal,
            key.clone(),
            expected,
            view.clone(),
            verified.dependency_cut(),
        );

        let status = proposal_status(&snapshot, &verified.payload().identity().proposal.0);
        let environment = verification_environment(status, &snapshot);
        let rules = ScriptVerificationRules::from_env(snapshot.consensus(), &environment);
        if verified.verification_context().rules() != rules {
            // Successful Verify compacts dependency scripts/data. A rules
            // transition cannot honestly reuse that payload for another VM
            // run, so the exact Ready owner returns to Resolve instead.
            return Ok(FinalAdmissionValidationOutcome::Reresolve(
                FinalAdmissionRetry::new(seal, subject),
            ));
        }

        let same_chain_state = verified.chain_view().has_same_chain_state(&view);
        let location_result = if same_chain_state {
            refresh_locations(verified.payload_arc(), true, &snapshot, &overlay)
        } else {
            // Header/cell reads can hit RocksDB. The authority guard was
            // released before this job was created, and the entire changed-tip
            // lookup slice runs off the async executor.
            block_offload(|| {
                validate_header_dependencies(verified.payload(), &snapshot)?;
                refresh_locations(verified.payload_arc(), false, &snapshot, &overlay)
            })
        };
        let (payload, payload_relation) = match location_result {
            Ok(value) => value,
            Err(CandidateValidationError::Rejected(reason)) => {
                return Ok(FinalAdmissionValidationOutcome::Rejected(
                    FinalAdmissionRejection::new(seal, subject, CommittedPublicReject::new(reason)),
                ));
            }
            Err(CandidateValidationError::Fault(error)) => return Err(error),
        };
        let location = super::chain::CellLocationReceipt::from_resolution(&view, &payload);

        let context_is_reusable =
            payload_relation == ReadyPayloadRelation::Shared && verified.context_is_for(&view);
        if !context_is_reusable
            && let Err(reason) = revalidate_tx_context(
                Arc::clone(&snapshot),
                Arc::clone(payload.resolved_transaction()),
                Arc::new(environment),
            )
        {
            return Ok(FinalAdmissionValidationOutcome::Rejected(
                FinalAdmissionRejection::new(seal, subject, CommittedPublicReject::new(reason)),
            ));
        }

        let context = VerificationContextReceipt::from_validation(
            view,
            location,
            TimeContextReceipt::from_validation(rules),
        )
        .map_err(|_| FinalAdmissionValidationError::ContextReceipt)?;
        let sensitivity = chain_sensitivity(payload.resolved_transaction());
        let verified = match payload_relation {
            ReadyPayloadRelation::Shared => verified,
            ReadyPayloadRelation::LocationRefreshed => {
                verified.with_refreshed_locations(LocationRefreshSeal(()), Arc::clone(&payload))
            }
        };
        let verified = verified
            .with_context(context)
            .ok_or(FinalAdmissionValidationError::ContextReceipt)?;
        let membership = MembershipReceipt::from_validation(
            seal,
            verified,
            sensitivity,
            status,
            AcceptedAtMillis(ckb_systemtime::unix_time_as_millis()),
        );
        Ok(FinalAdmissionValidationOutcome::Candidate(
            FinalAdmissionReceipt::from_validation(
                seal,
                key,
                expected,
                membership,
                payload_relation,
            ),
        ))
    }
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
    if snapshot.proposals().contains_proposed(proposal) {
        AcceptedStatus::Proposed
    } else if snapshot.proposals().contains_gap(proposal) {
        AcceptedStatus::Gap
    } else {
        AcceptedStatus::Pending
    }
}

pub(super) fn verification_environment(status: AcceptedStatus, snapshot: &Snapshot) -> TxVerifyEnv {
    match status {
        AcceptedStatus::Pending => TxVerifyEnv::new_submit(snapshot.tip_header()),
        AcceptedStatus::Gap => TxVerifyEnv::new_proposed(snapshot.tip_header(), GAP_PROPOSAL_INDEX),
        AcceptedStatus::Proposed => {
            TxVerifyEnv::new_proposed(snapshot.tip_header(), PROPOSED_PROPOSAL_INDEX)
        }
    }
}

fn chain_sensitivity(resolved: &ResolvedTransaction) -> AcceptedChainSensitivity {
    let has_since = resolved
        .transaction
        .inputs()
        .into_iter()
        .any(|input| Into::<u64>::into(input.since()) != 0);
    let has_chain_cellbase = resolved
        .resolved_inputs
        .iter()
        .chain(&resolved.resolved_cell_deps)
        .chain(&resolved.resolved_dep_groups)
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
    overlay: &AcceptedOriginOverlay,
) -> Result<(Arc<ResolvedPayload>, ReadyPayloadRelation), CandidateValidationError> {
    let resolved = payload.resolved_transaction();
    let total_cells = resolved
        .resolved_inputs
        .len()
        .checked_add(resolved.resolved_cell_deps.len())
        .and_then(|count| count.checked_add(resolved.resolved_dep_groups.len()))
        .ok_or(FinalAdmissionValidationError::Arithmetic)?;
    let mut changes = Vec::new();
    let mut origins = overlay.origins();

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
            let pool_origin = origins
                .next()
                .ok_or(FinalAdmissionValidationError::ContextReceipt)?;
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
    if origins.next().is_some() {
        return Err(FinalAdmissionValidationError::ContextReceipt.into());
    }

    if changes.is_empty() {
        return Ok((Arc::clone(payload), ReadyPayloadRelation::Shared));
    }

    let mut refreshed = resolved.as_ref().clone();
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
    let payload = payload.with_refreshed_locations(LocationRefreshSeal(()), Arc::new(refreshed));
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
