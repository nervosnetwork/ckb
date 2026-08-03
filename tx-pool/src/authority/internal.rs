//! Sealed feature-internal transaction injection.
//!
//! `PlugEntry` is an established test and instrumentation hook: its caller
//! supplies a `TxEntry` that is already considered resolved and verified.
//! This module preserves that behavior without exposing a second production
//! admission authority. The synthetic premise is represented by a private
//! seal, then the candidate still enters the ordinary membership, resource,
//! relation, source-version, and template Plan/Apply compiler.

use super::{
    chain::{
        CellLocationReceipt, DirectAdmissionReceipt, TimeContextReceipt, VerificationContextReceipt,
    },
    resources::AcceptedCost,
    state::{
        AcceptedAtMillis, AcceptedStatus, CandidateMetrics, ChainViewId, DependencyCut,
        InputEvidenceError, ResolvedPayload, VerifiedFacts,
    },
};
use crate::component::entry::TxEntry;
use ckb_snapshot::Snapshot;
use ckb_verification::cache::ScriptVerificationRules;
use std::sync::Arc;

/// Unforgeable capability for the feature-internal synthetic evidence path.
/// The tuple field is private to this module; sibling modules can consume but
/// cannot construct the capability.
pub(super) struct InternalPlugSeal(());

#[derive(Debug)]
pub(super) enum InternalPlugBuildError {
    Evidence(InputEvidenceError),
    Allocation,
    Context,
}

/// Build immutable synthetic evidence outside the authority guard. The
/// snapshot and `view` must come from the same runtime-store capture; Plan
/// rechecks both the view and dependency cut before any ownership changes.
pub(super) fn build_receipt(
    entry: &TxEntry,
    status: AcceptedStatus,
    view: ChainViewId,
    dependency_cut: DependencyCut,
    snapshot: &Snapshot,
    max_edges: usize,
) -> Result<DirectAdmissionReceipt, InternalPlugBuildError> {
    let payload = ResolvedPayload::from_internal_plug(
        InternalPlugSeal(()),
        Arc::clone(&entry.rtx),
        max_edges,
        entry.fee,
        entry.size,
        entry.resident_size(),
    )
    .map(Arc::new)
    .map_err(InternalPlugBuildError::Evidence)?;
    let location = CellLocationReceipt::from_internal_plug(InternalPlugSeal(()), &view, &payload)
        .map_err(|()| InternalPlugBuildError::Allocation)?;
    let environment = super::validation::verification_environment(status, snapshot);
    let rules = ScriptVerificationRules::from_env(snapshot.consensus(), &environment);
    let context = VerificationContextReceipt::from_validation(
        view,
        location,
        TimeContextReceipt::from_validation(rules),
    )
    .map_err(|_| InternalPlugBuildError::Context)?;
    let metrics = CandidateMetrics {
        fee: entry.fee,
        cost: AcceptedCost::new(entry.size, entry.resident_size(), entry.cycles),
    };
    let verified = VerifiedFacts::from_internal_plug(
        InternalPlugSeal(()),
        dependency_cut,
        Arc::clone(&payload),
        context,
        metrics,
    );
    Ok(DirectAdmissionReceipt::from_internal_plug(
        InternalPlugSeal(()),
        Arc::new(payload.resolved_transaction().transaction.clone()),
        verified,
        status,
        AcceptedAtMillis(entry.timestamp),
    ))
}
