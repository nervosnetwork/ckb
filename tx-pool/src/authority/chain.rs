//! Typed chain and final-admission evidence.
//!
//! These receipts deliberately separate reusable transaction content and
//! script work from location, proposal and time facts invalidated by a tip
//! change. Constructors stay inside the authority boundary so callers cannot
//! assemble a membership proof from unrelated booleans or snapshots.

use super::state::{
    AcceptedStatus, CandidateMetrics, ChainTipHash, ChainViewId, DependencyCut, EntryVersion,
    InputEvidenceError, RawTxHash, ResolvedPayload, VerifiedFacts,
};
use ckb_types::packed::OutPoint;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct ValidationRulesId(pub(super) u64);

impl ValidationRulesId {
    #[cfg(test)]
    pub(super) const FOUNDATION: Self = Self(0);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CellContentReceipt {
    payload: Arc<ResolvedPayload>,
}

impl CellContentReceipt {
    pub(super) fn from_resolution(payload: Arc<ResolvedPayload>) -> Self {
        Self { payload }
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        &self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CellLocationReceipt {
    // The tip, rather than the monotonically changing revision, is the
    // lifetime of positive chain-location evidence.
    tip: ChainTipHash,
    chain_inputs: Arc<[OutPoint]>,
}

impl CellLocationReceipt {
    #[cfg(test)]
    pub(super) fn empty_for_foundation(view: &ChainViewId) -> Self {
        Self {
            tip: view.tip().clone(),
            chain_inputs: Arc::from([]),
        }
    }

    pub(super) fn from_resolution(
        view: &ChainViewId,
        payload: &ResolvedPayload,
        mut chain_inputs: Vec<OutPoint>,
    ) -> Result<Self, InputEvidenceError> {
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        if chain_inputs
            .iter()
            .any(|input| payload.footprint.inputs().binary_search(input).is_err())
        {
            return Err(InputEvidenceError::NotAnInput);
        }
        Ok(Self {
            tip: view.tip().clone(),
            chain_inputs: chain_inputs.into(),
        })
    }

    pub(super) fn is_for(&self, view: &ChainViewId) -> bool {
        &self.tip == view.tip()
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }

    #[cfg(test)]
    fn refreshed_for_foundation(&self, view: &ChainViewId) -> Self {
        // This test-only seam represents a successful location revalidation;
        // production must construct the receipt from the current snapshot.
        Self {
            tip: view.tip().clone(),
            chain_inputs: Arc::clone(&self.chain_inputs),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TimeContextReceipt {
    // The enclosing VerificationContextReceipt owns the common ChainViewId,
    // so this role receipt does not duplicate it.
    rules: ValidationRulesId,
}

impl TimeContextReceipt {
    pub(super) fn from_validation(rules: ValidationRulesId) -> Self {
        Self { rules }
    }

    fn rules(&self) -> ValidationRulesId {
        self.rules
    }
}

/// Chain-sensitive validation consumed by one script-verification result.
/// Content may have been resolved at an older tip, but location and time are
/// always refreshed together against `view` before this receipt is created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct VerificationContextReceipt {
    view: ChainViewId,
    chain_inputs: Arc<[OutPoint]>,
    time: TimeContextReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerificationContextError {
    LocationViewMismatch,
}

impl VerificationContextReceipt {
    fn from_validation(
        view: ChainViewId,
        location: CellLocationReceipt,
        time: TimeContextReceipt,
    ) -> Result<Self, VerificationContextError> {
        if !location.is_for(&view) {
            return Err(VerificationContextError::LocationViewMismatch);
        }
        Ok(Self {
            view,
            chain_inputs: location.chain_inputs,
            time,
        })
    }

    #[cfg(test)]
    pub(super) fn refresh_for_foundation(
        view: ChainViewId,
        previous_location: CellLocationReceipt,
        rules: ValidationRulesId,
    ) -> Result<Self, VerificationContextError> {
        // The harness has no snapshot validator. Retargeting here stands for
        // a completed validation, not permission to stamp old evidence fresh.
        let location = previous_location.refreshed_for_foundation(&view);
        Self::from_validation(view, location, TimeContextReceipt::from_validation(rules))
    }

    #[cfg(test)]
    pub(super) fn empty_for_foundation(view: ChainViewId, rules: ValidationRulesId) -> Self {
        Self {
            view,
            chain_inputs: Arc::from([]),
            time: TimeContextReceipt::from_validation(rules),
        }
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.view
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }

    pub(super) fn rules(&self) -> ValidationRulesId {
        self.time.rules()
    }

    pub(super) fn is_for(&self, view: &ChainViewId) -> bool {
        &self.view == view
    }

    #[cfg(test)]
    fn refreshed_for_foundation(&self, view: ChainViewId, rules: ValidationRulesId) -> Self {
        Self {
            view,
            chain_inputs: Arc::clone(&self.chain_inputs),
            time: TimeContextReceipt::from_validation(rules),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScriptReceipt {
    // VerifiedFacts stores this beside the immutable payload identity, so the
    // script receipt needs only the ruleset that controls reusability.
    rules: ValidationRulesId,
}

impl ScriptReceipt {
    pub(super) fn from_verification(rules: ValidationRulesId) -> Self {
        Self { rules }
    }

    pub(super) fn is_reusable_under(&self, rules: ValidationRulesId) -> bool {
        self.rules == rules
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProposalContextReceipt {
    // AcceptedProof's verification context owns the common final ChainViewId.
    status: AcceptedStatus,
}

impl ProposalContextReceipt {
    fn from_validation(status: AcceptedStatus) -> Self {
        Self { status }
    }

    pub(super) fn status(&self) -> AcceptedStatus {
        self.status
    }
}

/// Proof retained by accepted membership. Its location/proposal/time fields
/// all come from the final validation view, while content and script work may
/// have originated at an equivalent earlier chain state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AcceptedProof {
    verified: VerifiedFacts,
    proposal: ProposalContextReceipt,
}

impl AcceptedProof {
    #[cfg(test)]
    pub(super) fn for_foundation(verified: VerifiedFacts, status: AcceptedStatus) -> Self {
        Self {
            proposal: ProposalContextReceipt::from_validation(status),
            verified,
        }
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        self.verified.payload()
    }

    pub(super) fn metrics(&self) -> &CandidateMetrics {
        self.verified.metrics()
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.verified.dependency_cut()
    }

    pub(super) fn admission_view(&self) -> &ChainViewId {
        self.verified.chain_view()
    }

    pub(super) fn chain_revision(&self) -> super::state::ChainRevision {
        self.admission_view().revision()
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.verified.is_chain_input(input)
    }

    pub(super) fn is_for(&self, view: &ChainViewId) -> bool {
        self.verified.context_is_for(view)
    }
}

/// Read-only candidate capability. It does not own or reserve membership;
/// `EntryVersion` makes concurrent final validations ordinary OCC attempts.
#[derive(Clone, Debug)]
pub(super) struct FinalAdmissionWork {
    key: RawTxHash,
    expected: EntryVersion,
    view: ChainViewId,
    verified: VerifiedFacts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FinalAdmissionError {
    ScriptRulesChanged,
}

impl FinalAdmissionWork {
    pub(super) fn new(
        key: RawTxHash,
        expected: EntryVersion,
        view: ChainViewId,
        verified: VerifiedFacts,
    ) -> Self {
        Self {
            key,
            expected,
            view,
            verified,
        }
    }

    /// Target-harness constructor for the result of real snapshot, overlay,
    /// proposal and time validation. G5 replaces this test seam with the
    /// tx-pool validator; production callers never receive the raw fields.
    #[cfg(test)]
    pub(super) fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ValidationRulesId,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        let context = self
            .verified
            .verification_context()
            .refreshed_for_foundation(self.view.clone(), rules);
        let verified = self
            .verified
            .with_context(context)
            .ok_or(FinalAdmissionError::ScriptRulesChanged)?;
        let proposal = ProposalContextReceipt::from_validation(status);
        Ok(FinalAdmissionReceipt {
            key: self.key,
            expected: self.expected,
            proof: AcceptedProof { verified, proposal },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "final admission evidence must be applied or discarded as stale"]
pub(super) struct FinalAdmissionReceipt {
    key: RawTxHash,
    expected: EntryVersion,
    proof: AcceptedProof,
}

impl FinalAdmissionReceipt {
    pub(super) fn key(&self) -> &RawTxHash {
        &self.key
    }

    pub(super) fn expected(&self) -> EntryVersion {
        self.expected
    }

    pub(super) fn view(&self) -> &ChainViewId {
        self.proof.admission_view()
    }

    pub(super) fn status(&self) -> AcceptedStatus {
        self.proof.proposal.status()
    }

    pub(super) fn proof(&self) -> &AcceptedProof {
        &self.proof
    }

    pub(super) fn into_proof(self) -> AcceptedProof {
        self.proof
    }
}
