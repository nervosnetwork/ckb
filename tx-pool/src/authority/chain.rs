//! Typed chain and final-admission evidence.
//!
//! These receipts deliberately separate reusable transaction content and
//! script work from location, proposal and time facts invalidated by a tip
//! change. Constructors stay inside the authority boundary so callers cannot
//! assemble a membership proof from unrelated booleans or snapshots.

use super::rejection::CommittedPublicReject;
use super::state::{
    AcceptedAtMillis, AcceptedStatus, AdmissionValidationError, ApplySequence, CandidateMetrics,
    ChainTipHash, ChainViewId, DependencyCut, EntryVersion, PoolGeneration, PreAcceptedSource,
    ProposalBase, ProposalId, RawTxHash, ResolvedPayload, ValidatedAdmission, VerifiedFacts,
};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::TransactionView,
    packed::{Byte32, OutPoint},
};
use ckb_verification::cache::ScriptVerificationRules;
use std::collections::HashSet;
use std::sync::Arc;

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

    pub(super) fn payload_arc(&self) -> &Arc<ResolvedPayload> {
        &self.payload
    }

    pub(super) fn into_payload(self) -> Arc<ResolvedPayload> {
        self.payload
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CellLocationReceipt {
    // The tip, rather than the monotonically changing revision, is the
    // lifetime of positive chain-location evidence.
    tip: ChainTipHash,
    chain_inputs: Arc<[OutPoint]>,
    chain_dependencies: Arc<[OutPoint]>,
}

impl CellLocationReceipt {
    #[cfg(test)]
    pub(super) fn empty_for_foundation(view: &ChainViewId) -> Self {
        Self {
            tip: view.tip().clone(),
            chain_inputs: Arc::from([]),
            chain_dependencies: Arc::from([]),
        }
    }

    /// Derive tx-pool-only positive location evidence from the exact resolved
    /// input metadata. A chain input has `transaction_info`; a pool-produced
    /// input does not. This receipt is never used by block validation, whose
    /// resolver and liveness rules remain independent.
    pub(super) fn from_resolution(view: &ChainViewId, payload: &ResolvedPayload) -> Self {
        let mut chain_inputs = payload
            .resolved_transaction()
            .resolved_inputs
            .iter()
            .filter(|cell| cell.transaction_info.is_some())
            .map(|cell| cell.out_point.clone())
            .collect::<Vec<_>>();
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        let mut chain_dependencies = payload
            .resolved_transaction()
            .resolved_cell_deps
            .iter()
            .chain(payload.resolved_transaction().resolved_dep_groups.iter())
            .filter(|cell| cell.transaction_info.is_some())
            .map(|cell| cell.out_point.clone())
            .collect::<Vec<_>>();
        chain_dependencies.sort_unstable();
        chain_dependencies.dedup();
        Self {
            tip: view.tip().clone(),
            chain_inputs: chain_inputs.into(),
            chain_dependencies: chain_dependencies.into(),
        }
    }

    pub(super) fn is_for(&self, view: &ChainViewId) -> bool {
        &self.tip == view.tip()
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }

    pub(super) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.chain_dependencies.binary_search(dependency).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TimeContextReceipt {
    // The enclosing VerificationContextReceipt owns the common ChainViewId,
    // so this role receipt does not duplicate it.
    rules: ScriptVerificationRules,
}

impl TimeContextReceipt {
    pub(super) fn from_validation(rules: ScriptVerificationRules) -> Self {
        Self { rules }
    }

    fn rules(&self) -> ScriptVerificationRules {
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
    chain_dependencies: Arc<[OutPoint]>,
    time: TimeContextReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerificationContextError {
    LocationViewMismatch,
}

impl VerificationContextReceipt {
    pub(super) fn from_validation(
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
            chain_dependencies: location.chain_dependencies,
            time,
        })
    }

    /// Consume location evidence created inside the same sealed resolution
    /// fact as `view`. Unlike final-admission refresh, this constructor cannot
    /// receive independently sampled values, so no runtime mismatch state is
    /// representable on the verification path.
    pub(super) fn from_resolved(
        _seal: super::work::VerificationSeal,
        view: ChainViewId,
        location: CellLocationReceipt,
        time: TimeContextReceipt,
    ) -> Self {
        Self {
            view,
            chain_inputs: location.chain_inputs,
            chain_dependencies: location.chain_dependencies,
            time,
        }
    }

    #[cfg(test)]
    pub(super) fn empty_for_foundation(view: ChainViewId, rules: ScriptVerificationRules) -> Self {
        Self {
            view,
            chain_inputs: Arc::from([]),
            chain_dependencies: Arc::from([]),
            time: TimeContextReceipt::from_validation(rules),
        }
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.view
    }

    pub(super) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }

    pub(super) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.chain_dependencies.binary_search(dependency).is_ok()
    }

    pub(super) fn rules(&self) -> ScriptVerificationRules {
        self.time.rules()
    }

    pub(super) fn is_for(&self, view: &ChainViewId) -> bool {
        &self.view == view
    }

    #[cfg(test)]
    fn refreshed_for_foundation(&self, view: ChainViewId, rules: ScriptVerificationRules) -> Self {
        Self {
            view,
            chain_inputs: Arc::clone(&self.chain_inputs),
            chain_dependencies: Arc::clone(&self.chain_dependencies),
            time: TimeContextReceipt::from_validation(rules),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScriptReceipt {
    // VerifiedFacts stores this beside the immutable payload identity, so the
    // script receipt needs only the ruleset that controls reusability.
    rules: ScriptVerificationRules,
}

impl ScriptReceipt {
    pub(super) fn from_verification(rules: ScriptVerificationRules) -> Self {
        Self { rules }
    }

    pub(super) fn is_reusable_under(&self, rules: ScriptVerificationRules) -> bool {
        self.rules == rules
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProposalContextReceipt {
    // AcceptedProof's verification context owns the common final ChainViewId.
    status: AcceptedStatus,
}

impl ProposalContextReceipt {
    pub(super) fn from_validation(status: AcceptedStatus) -> Self {
        Self { status }
    }

    pub(super) fn status(&self) -> AcceptedStatus {
        self.status
    }
}

/// Proof retained by accepted membership. Its location/proposal/time fields
/// all come from the final validation view, while content and script work may
/// have originated at an equivalent earlier chain state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AcceptedChainSensitivity {
    Stable,
    TipContext,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct AcceptedProof {
    verified: VerifiedFacts,
    sensitivity: AcceptedChainSensitivity,
}

impl AcceptedProof {
    #[cfg(test)]
    pub(super) fn for_foundation(verified: VerifiedFacts) -> Self {
        Self {
            verified,
            sensitivity: AcceptedChainSensitivity::Stable,
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

    pub(super) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.verified.is_chain_dependency(dependency)
    }

    pub(super) fn is_for(&self, view: &ChainViewId) -> bool {
        self.verified.context_is_for(view)
    }

    pub(super) fn sensitivity(&self) -> AcceptedChainSensitivity {
        self.sensitivity
    }
}

impl AcceptedChainSensitivity {
    pub(super) fn requires_reorg_revalidation(self) -> bool {
        match self {
            Self::Stable => false,
            Self::TipContext => true,
        }
    }
}

/// Read-only candidate capability. It does not own or reserve membership;
/// `EntryVersion` makes concurrent final validations ordinary OCC attempts.
#[derive(Clone, Debug)]
pub(super) struct FinalAdmissionWork {
    key: RawTxHash,
    expected: EntryVersion,
    validation: MembershipValidationWork,
}

/// Read-only validation work for a synchronous trusted admission. Unlike
/// [`FinalAdmissionWork`], it has no resident PreAccepted owner or version:
/// the exact transaction and its verification facts are sealed together and
/// membership is decided by one later authority Plan/Apply.
#[derive(Clone, Debug)]
pub(super) struct DirectAdmissionWork {
    tx: Arc<TransactionView>,
    validation: MembershipValidationWork,
}

#[derive(Clone, Debug)]
pub(super) struct MembershipValidationWork {
    view: ChainViewId,
    verified: VerifiedFacts,
}

/// Whether final validation retained the Ready owner's resolved payload or
/// installed a location-refreshed payload. The latter cannot use the bounded
/// inline shell-retirement path because it may release the last reference to
/// the previous resolved cells under the authority guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ReadyPayloadRelation {
    Shared,
    LocationRefreshed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AdmissionEvidenceError {
    TransactionIdentityMismatch,
    ScriptRulesChanged,
}

pub(super) type FinalAdmissionError = AdmissionEvidenceError;
pub(super) type DirectAdmissionError = AdmissionEvidenceError;

impl MembershipValidationWork {
    fn new(view: ChainViewId, verified: VerifiedFacts) -> Self {
        Self { view, verified }
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        self.verified.payload()
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.view
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.verified.dependency_cut()
    }

    pub(super) fn with_validated_dependency_cut(
        self,
        seal: super::validation::AdmissionValidationSeal,
        dependency_cut: DependencyCut,
    ) -> Self {
        Self {
            view: self.view,
            verified: self
                .verified
                .with_validated_dependency_cut(seal, dependency_cut),
        }
    }

    pub(super) fn into_parts(self) -> (ChainViewId, VerifiedFacts) {
        (self.view, self.verified)
    }

    #[cfg(test)]
    fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        sensitivity: AcceptedChainSensitivity,
    ) -> Result<MembershipReceipt, AdmissionEvidenceError> {
        self.validate_at_for_foundation(status, rules, sensitivity, AcceptedAtMillis::FOUNDATION)
    }

    #[cfg(test)]
    fn validate_at_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        sensitivity: AcceptedChainSensitivity,
        accepted_at: AcceptedAtMillis,
    ) -> Result<MembershipReceipt, AdmissionEvidenceError> {
        let context = self
            .verified
            .verification_context()
            .refreshed_for_foundation(self.view, rules);
        let verified = self
            .verified
            .with_context(context)
            .ok_or(AdmissionEvidenceError::ScriptRulesChanged)?;
        Ok(MembershipReceipt {
            proof: AcceptedProof {
                verified,
                sensitivity,
            },
            proposal: ProposalContextReceipt::from_validation(status),
            accepted_at,
        })
    }
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
            validation: MembershipValidationWork::new(view, verified),
        }
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        self.validation.payload()
    }

    pub(super) fn key(&self) -> &RawTxHash {
        &self.key
    }

    pub(super) fn expected(&self) -> EntryVersion {
        self.expected
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.validation.view
    }

    pub(super) fn into_validation_parts(
        self,
    ) -> (RawTxHash, EntryVersion, MembershipValidationWork) {
        (self.key, self.expected, self.validation)
    }

    /// Target-harness constructor for the result of real snapshot, overlay,
    /// proposal and time validation. G5 replaces this test seam with the
    /// tx-pool validator; production callers never receive the raw fields.
    #[cfg(test)]
    pub(super) fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        self.validate_with_sensitivity_for_foundation(
            status,
            rules,
            AcceptedChainSensitivity::Stable,
        )
    }

    #[cfg(test)]
    pub(super) fn validate_at_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        accepted_at: AcceptedAtMillis,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        Ok(FinalAdmissionReceipt {
            key: self.key,
            expected: self.expected,
            membership: self.validation.validate_at_for_foundation(
                status,
                rules,
                AcceptedChainSensitivity::Stable,
                accepted_at,
            )?,
            payload_relation: ReadyPayloadRelation::Shared,
        })
    }

    #[cfg(test)]
    pub(super) fn validate_context_sensitive_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        self.validate_with_sensitivity_for_foundation(
            status,
            rules,
            AcceptedChainSensitivity::TipContext,
        )
    }

    #[cfg(test)]
    fn validate_with_sensitivity_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        sensitivity: AcceptedChainSensitivity,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        Ok(FinalAdmissionReceipt {
            key: self.key,
            expected: self.expected,
            membership: self
                .validation
                .validate_for_foundation(status, rules, sensitivity)?,
            payload_relation: ReadyPayloadRelation::Shared,
        })
    }
}

impl DirectAdmissionWork {
    pub(super) fn new(
        tx: Arc<TransactionView>,
        view: ChainViewId,
        verified: VerifiedFacts,
    ) -> Result<Self, DirectAdmissionError> {
        if verified.payload().identity() != &super::state::TxIdentity::from_transaction(&tx) {
            return Err(DirectAdmissionError::TransactionIdentityMismatch);
        }
        Ok(Self {
            tx,
            validation: MembershipValidationWork::new(view, verified),
        })
    }

    pub(super) fn payload(&self) -> &ResolvedPayload {
        self.validation.payload()
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.validation.view
    }

    pub(super) fn into_validation_parts(self) -> (Arc<TransactionView>, MembershipValidationWork) {
        (self.tx, self.validation)
    }

    #[cfg(test)]
    pub(super) fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
    ) -> Result<DirectAdmissionReceipt, DirectAdmissionError> {
        Ok(DirectAdmissionReceipt {
            tx: self.tx,
            membership: self.validation.validate_for_foundation(
                status,
                rules,
                AcceptedChainSensitivity::Stable,
            )?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MembershipReceipt {
    proof: AcceptedProof,
    proposal: ProposalContextReceipt,
    accepted_at: AcceptedAtMillis,
}

impl MembershipReceipt {
    pub(super) fn from_validation(
        _seal: super::validation::AdmissionValidationSeal,
        verified: VerifiedFacts,
        sensitivity: AcceptedChainSensitivity,
        status: AcceptedStatus,
        accepted_at: AcceptedAtMillis,
    ) -> Self {
        Self {
            proof: AcceptedProof {
                verified,
                sensitivity,
            },
            proposal: ProposalContextReceipt::from_validation(status),
            accepted_at,
        }
    }

    fn view(&self) -> &ChainViewId {
        self.proof.admission_view()
    }

    fn proof(&self) -> &AcceptedProof {
        &self.proof
    }

    fn into_parts(self) -> (AcceptedProof, ProposalContextReceipt, AcceptedAtMillis) {
        (self.proof, self.proposal, self.accepted_at)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "final admission evidence must be applied or discarded as stale"]
pub(super) struct FinalAdmissionReceipt {
    key: RawTxHash,
    expected: EntryVersion,
    membership: MembershipReceipt,
    payload_relation: ReadyPayloadRelation,
}

/// The immutable authority cut against which a lock-external final outcome
/// was validated. Keeping these fields together prevents a caller from
/// terminalizing or requeueing a different Ready incarnation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FinalAdmissionSubject {
    key: RawTxHash,
    expected: EntryVersion,
    view: ChainViewId,
    dependency_cut: DependencyCut,
}

impl FinalAdmissionSubject {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        key: RawTxHash,
        expected: EntryVersion,
        view: ChainViewId,
        dependency_cut: DependencyCut,
    ) -> Self {
        Self {
            key,
            expected,
            view,
            dependency_cut,
        }
    }

    pub(super) fn key(&self) -> &RawTxHash {
        &self.key
    }

    pub(super) fn expected(&self) -> EntryVersion {
        self.expected
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.view
    }

    pub(super) fn dependency_cut(&self) -> DependencyCut {
        self.dependency_cut
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FinalAdmissionRejection {
    subject: FinalAdmissionSubject,
    reason: CommittedPublicReject,
}

impl FinalAdmissionRejection {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        subject: FinalAdmissionSubject,
        reason: CommittedPublicReject,
    ) -> Self {
        Self { subject, reason }
    }

    pub(super) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }

    pub(super) fn into_parts(self) -> (FinalAdmissionSubject, CommittedPublicReject) {
        (self.subject, self.reason)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FinalAdmissionRetry {
    subject: FinalAdmissionSubject,
}

impl FinalAdmissionRetry {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        subject: FinalAdmissionSubject,
    ) -> Self {
        Self { subject }
    }

    pub(super) fn into_subject(self) -> FinalAdmissionSubject {
        self.subject
    }
}

impl FinalAdmissionReceipt {
    pub(super) fn from_validation(
        _seal: super::validation::AdmissionValidationSeal,
        key: RawTxHash,
        expected: EntryVersion,
        membership: MembershipReceipt,
        payload_relation: ReadyPayloadRelation,
    ) -> Self {
        Self {
            key,
            expected,
            membership,
            payload_relation,
        }
    }

    pub(super) fn key(&self) -> &RawTxHash {
        &self.key
    }

    pub(super) fn expected(&self) -> EntryVersion {
        self.expected
    }

    pub(super) fn view(&self) -> &ChainViewId {
        self.membership.view()
    }

    pub(super) fn proof(&self) -> &AcceptedProof {
        self.membership.proof()
    }

    pub(super) fn payload_relation(&self) -> ReadyPayloadRelation {
        self.payload_relation
    }

    pub(super) fn into_membership_parts(
        self,
    ) -> (AcceptedProof, ProposalContextReceipt, AcceptedAtMillis) {
        self.membership.into_parts()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "direct admission evidence must be applied or discarded as stale"]
pub(super) struct DirectAdmissionReceipt {
    tx: Arc<TransactionView>,
    membership: MembershipReceipt,
}

impl DirectAdmissionReceipt {
    pub(super) fn from_validation(
        _seal: super::validation::AdmissionValidationSeal,
        tx: Arc<TransactionView>,
        membership: MembershipReceipt,
    ) -> Self {
        Self { tx, membership }
    }

    pub(super) fn key(&self) -> &RawTxHash {
        &self.membership.proof().payload().identity().raw
    }

    pub(super) fn transaction(&self) -> &Arc<TransactionView> {
        &self.tx
    }

    pub(super) fn view(&self) -> &ChainViewId {
        self.membership.view()
    }

    pub(super) fn proof(&self) -> &AcceptedProof {
        self.membership.proof()
    }

    pub(super) fn completed(&self) -> ckb_types::core::EntryCompleted {
        let metrics = self.proof().metrics();
        ckb_types::core::EntryCompleted {
            cycles: metrics.cost.cycles,
            fee: metrics.fee,
        }
    }

    pub(super) fn into_membership_parts(
        self,
    ) -> (
        Arc<TransactionView>,
        AcceptedProof,
        ProposalContextReceipt,
        AcceptedAtMillis,
    ) {
        let (proof, proposal, accepted_at) = self.membership.into_parts();
        (self.tx, proof, proposal, accepted_at)
    }
}

/// Immutable direct-validation subject. It contains no membership, resource,
/// clock, or effect capability; Local may later consume it through the
/// authority planner, while TestAccept can return the same evaluation without
/// acquiring mutation authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectAdmissionSubject {
    tx: Arc<TransactionView>,
    view: ChainViewId,
    accepted_source: ApplySequence,
}

impl DirectAdmissionSubject {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        tx: Arc<TransactionView>,
        view: ChainViewId,
        accepted_source: ApplySequence,
    ) -> Self {
        Self {
            tx,
            view,
            accepted_source,
        }
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.view
    }

    pub(super) fn accepted_source(&self) -> ApplySequence {
        self.accepted_source
    }

    pub(super) fn into_transaction(self) -> Arc<TransactionView> {
        self.tx
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectAdmissionRejection {
    subject: DirectAdmissionSubject,
    reason: CommittedPublicReject,
}

impl DirectAdmissionRejection {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        subject: DirectAdmissionSubject,
        reason: CommittedPublicReject,
    ) -> Self {
        Self { subject, reason }
    }

    pub(super) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }

    pub(super) fn into_parts(self) -> (DirectAdmissionSubject, CommittedPublicReject) {
        (self.subject, self.reason)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectAdmissionRetry {
    subject: DirectAdmissionSubject,
}

impl DirectAdmissionRetry {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        subject: DirectAdmissionSubject,
    ) -> Self {
        Self { subject }
    }

    pub(super) fn into_subject(self) -> DirectAdmissionSubject {
        self.subject
    }
}

/// Whether the local node materializes proposal-window promotions for block
/// construction. Demotions remain chain-correctness work in either mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainPackagingMode {
    Package,
    ObserveOnly,
}

/// Inductive validity proof for Accepted membership across one chain cut.
/// Production construction may choose `Preserved` only after proving a
/// monotonic extension under the same validation rules. Tip-context changes
/// and script-rule changes remain distinct because the latter invalidates
/// every Accepted proof, not only the context-sensitive projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AcceptedValidityTransition {
    Preserved,
    /// Tip location/time changed while script rules stayed stable. Only
    /// validation-proven context-sensitive Accepted owners must re-enter.
    ContextChanged,
    /// Script verification rules changed. Every Accepted proof was produced
    /// under the old rules and must re-enter regardless of cell sensitivity.
    RulesChanged,
}

/// Final position of one proposal id in the new snapshot. This is a closed
/// state rather than two booleans, so `proposed && gap` is unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProposalWindowPosition {
    Proposed,
    Gap,
    Outside,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainFactsError {
    DuplicateTransaction,
    DuplicateHeader,
    Allocation,
}

/// The block-derived half of a chain transition. Grouping these four causal
/// fact sets keeps callers from confusing them with proposal-window inputs.
#[derive(Debug)]
pub(super) struct ChainBlockChanges {
    attached: Vec<TransactionView>,
    detached: Vec<TransactionView>,
    attached_headers: Vec<Byte32>,
    detached_headers: Vec<Byte32>,
}

impl ChainBlockChanges {
    pub(super) fn from_chain_update(
        attached: Vec<TransactionView>,
        detached: Vec<TransactionView>,
        attached_headers: Vec<Byte32>,
        detached_headers: Vec<Byte32>,
    ) -> Self {
        Self {
            attached,
            detached,
            attached_headers,
            detached_headers,
        }
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        attached: Vec<TransactionView>,
        detached: Vec<TransactionView>,
        attached_headers: Vec<Byte32>,
        detached_headers: Vec<Byte32>,
    ) -> Self {
        Self::from_chain_update(attached, detached, attached_headers, detached_headers)
    }
}

/// Bounded facts extracted from attached/detached blocks before taking the
/// authority guard. The new snapshot itself is deliberately not retained.
#[derive(Debug)]
pub(super) struct ChainTransitionFacts {
    pub(super) new_view: ChainViewId,
    pub(super) attached: Vec<TransactionView>,
    pub(super) detached: Vec<TransactionView>,
    pub(super) relocated: Vec<RawTxHash>,
    pub(super) attached_headers: Vec<Byte32>,
    pub(super) detached_headers: Vec<Byte32>,
    pub(super) changed_proposals: Vec<ProposalId>,
    pub(super) detached_proposals: Vec<ProposalId>,
    pub(super) accepted_validity: AcceptedValidityTransition,
    pub(super) packaging: ChainPackagingMode,
}

impl ChainTransitionFacts {
    pub(super) fn from_chain_update(
        new_view: ChainViewId,
        blocks: ChainBlockChanges,
        changed_proposals: Vec<ProposalId>,
        detached_proposals: Vec<ProposalId>,
        accepted_validity: AcceptedValidityTransition,
        packaging: ChainPackagingMode,
    ) -> Result<Self, ChainFactsError> {
        let ChainBlockChanges {
            attached,
            detached,
            attached_headers,
            detached_headers,
        } = blocks;
        let attached = canonical_transactions(attached)?;
        let mut attached_hashes = HashSet::new();
        attached_hashes
            .try_reserve(attached.len())
            .map_err(|_| ChainFactsError::Allocation)?;
        attached_hashes.extend(
            attached
                .iter()
                .map(|transaction| RawTxHash(transaction.hash())),
        );
        let mut detached = canonical_transactions(detached)?;
        let mut relocated = Vec::new();
        relocated
            .try_reserve(detached.len().min(attached_hashes.len()))
            .map_err(|_| ChainFactsError::Allocation)?;
        relocated.extend(
            detached
                .iter()
                .map(|transaction| RawTxHash(transaction.hash()))
                .filter(|hash| attached_hashes.contains(hash)),
        );
        detached.retain(|transaction| !attached_hashes.contains(&RawTxHash(transaction.hash())));
        let attached_headers = canonical_headers(attached_headers)?;
        let mut attached_header_set = HashSet::new();
        attached_header_set
            .try_reserve(attached_headers.len())
            .map_err(|_| ChainFactsError::Allocation)?;
        attached_header_set.extend(attached_headers.iter().cloned());
        let mut detached_headers = canonical_headers(detached_headers)?;
        detached_headers.retain(|header| !attached_header_set.contains(header));
        Ok(Self {
            new_view,
            attached,
            detached,
            relocated,
            attached_headers,
            detached_headers,
            changed_proposals: canonical_proposals(changed_proposals),
            detached_proposals: canonical_proposals(detached_proposals),
            accepted_validity,
            packaging,
        })
    }

    #[cfg(test)]
    pub(super) fn for_foundation(
        new_view: ChainViewId,
        blocks: ChainBlockChanges,
        changed_proposals: Vec<ProposalId>,
        detached_proposals: Vec<ProposalId>,
        packaging: ChainPackagingMode,
    ) -> Result<Self, ChainFactsError> {
        let ChainBlockChanges {
            attached,
            detached,
            attached_headers,
            detached_headers,
        } = blocks;
        let had_detached_transactions = !detached.is_empty();
        let had_detached_headers = !detached_headers.is_empty();
        Self::from_chain_update(
            new_view,
            ChainBlockChanges {
                attached,
                detached,
                attached_headers,
                detached_headers,
            },
            changed_proposals,
            detached_proposals,
            if had_detached_transactions || had_detached_headers {
                AcceptedValidityTransition::ContextChanged
            } else {
                AcceptedValidityTransition::Preserved
            },
            packaging,
        )
    }

    /// Represents a rules-only context transition in the target harness.
    /// Production obtains this classification from the same old/new snapshot
    /// comparison that creates `new_view`; callers never pass a raw flag.
    #[cfg(test)]
    pub(super) fn revalidate_all_for_foundation(mut self) -> Self {
        self.accepted_validity = AcceptedValidityTransition::RulesChanged;
        self
    }
}

fn canonical_transactions(
    mut transactions: Vec<TransactionView>,
) -> Result<Vec<TransactionView>, ChainFactsError> {
    // Block extraction is allowed to pass complete transaction lists; the
    // chain authority, rather than every caller, owns the cellbase exclusion.
    transactions.retain(|transaction| !transaction.is_cellbase());
    transactions.sort_unstable_by_key(TransactionView::hash);
    if transactions.windows(2).any(|pair| match pair {
        [left, right] => left.hash() == right.hash(),
        _ => false,
    }) {
        return Err(ChainFactsError::DuplicateTransaction);
    }
    Ok(transactions)
}

fn canonical_headers(mut headers: Vec<Byte32>) -> Result<Vec<Byte32>, ChainFactsError> {
    headers.sort_unstable();
    if headers.windows(2).any(|pair| match pair {
        [left, right] => left == right,
        _ => false,
    }) {
        return Err(ChainFactsError::DuplicateHeader);
    }
    Ok(headers)
}

fn canonical_proposals(mut proposals: Vec<ProposalId>) -> Vec<ProposalId> {
    proposals.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    proposals.dedup();
    proposals
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExpectedPreAcceptedOwner {
    pub(super) version: EntryVersion,
    pub(super) source: PreAcceptedSource,
}

/// Exact owner fact for a detached transaction that re-enters admission. A
/// vacant owner is evidence too: a later admission cannot be overwritten by
/// recovery when validation and Apply are separated in the foundation seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainRecoveryOwner {
    Vacant,
    PreAccepted(ExpectedPreAcceptedOwner),
    Accepted(EntryVersion),
    ReplacementHistory(EntryVersion),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainCommittedOwner {
    PreAccepted(ExpectedPreAcceptedOwner),
    Accepted(EntryVersion),
    ReplacementHistory(EntryVersion),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainConflictOwner {
    PreAccepted(ExpectedPreAcceptedOwner),
    Accepted(EntryVersion),
}

/// A chain removal carries only owner kinds legal for its cause. Invalid
/// `cause x owner` combinations therefore cannot reach Plan as runtime data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ChainRemoval {
    Committed {
        hash: RawTxHash,
        expected: ChainCommittedOwner,
    },
    Recovery {
        hash: RawTxHash,
        expected: EntryVersion,
    },
    /// The canonical cell consumed by the attached chain transition. Keeping
    /// the evidence in the cause makes a conflict removal incapable of losing
    /// the public `Resolve(Dead(out_point))` reason before effect publication.
    ChainConflict {
        hash: RawTxHash,
        expected: ChainConflictOwner,
        out_point: OutPoint,
    },
    ProposalWindowExpired {
        hash: RawTxHash,
        expected: ExpectedPreAcceptedOwner,
    },
}

impl ChainRemoval {
    pub(super) fn hash(&self) -> &RawTxHash {
        match self {
            Self::Committed { hash, .. }
            | Self::Recovery { hash, .. }
            | Self::ChainConflict { hash, .. }
            | Self::ProposalWindowExpired { hash, .. } => hash,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ChainStatusSubject {
    pub(super) hash: RawTxHash,
    pub(super) expected: EntryVersion,
    pub(super) proposal: ProposalId,
    pub(super) before: AcceptedStatus,
    pub(super) baseline: ProposalStatusBaseline,
}

#[derive(Clone, Debug)]
pub(super) struct ChainProposalSubject {
    pub(super) hash: RawTxHash,
    pub(super) expected: ExpectedPreAcceptedOwner,
    pub(super) proposal: ProposalId,
    pub(super) base: ProposalBase,
}

/// Why proposal-window reconciliation starts from the current status or from
/// Pending. A detached proposal is a semantic cause, not a Boolean option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProposalStatusBaseline {
    Current,
    DetachedProposal,
}

/// Read-only, lock-outside validation work. It carries only the bounded owner
/// slice and immutable transactions selected under the authority cut.
#[derive(Debug)]
#[must_use = "chain validation work must be validated or discarded"]
pub(super) struct ChainValidationWork {
    pub(super) generation: PoolGeneration,
    pub(super) old_view: ChainViewId,
    pub(super) new_view: ChainViewId,
    pub(super) committed: Vec<RawTxHash>,
    pub(super) removals: Vec<ChainRemoval>,
    pub(super) recoveries: Vec<ChainRecoveryWork>,
    pub(super) status_subjects: Vec<ChainStatusSubject>,
    pub(super) proposal_subjects: Vec<ChainProposalSubject>,
    pub(super) available: Vec<super::state::DependencyKey>,
    pub(super) lost: Vec<super::state::DependencyKey>,
    pub(super) packaging: ChainPackagingMode,
}

/// Recovery validation is provenance-sensitive. A transaction taken from a
/// detached block or Accepted membership is trusted chain-derived input;
/// merely depending on one does not promote an unverified preaccepted owner.
#[derive(Debug)]
pub(super) enum ChainRecoveryWork {
    Trusted {
        transaction: TransactionView,
        expected: ChainRecoveryOwner,
    },
    RequeueExisting {
        hash: RawTxHash,
        expected: ExpectedPreAcceptedOwner,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ChainValidationError {
    SnapshotMismatch,
    MissingProposalPosition,
    UnexpectedProposalPosition,
    DuplicateProposalPosition,
    RecoveryAdmission(AdmissionValidationError),
    Allocation,
}

#[derive(Debug)]
pub(super) enum ChainRecoveryReceipt {
    Trusted {
        admission: ValidatedAdmission,
        expected: ChainRecoveryOwner,
    },
    RequeueExisting {
        hash: RawTxHash,
        expected: ExpectedPreAcceptedOwner,
    },
}

impl ChainRecoveryReceipt {
    pub(super) fn key(&self) -> &RawTxHash {
        match self {
            Self::Trusted { admission, .. } => &admission.identity.raw,
            Self::RequeueExisting { hash, .. } => hash,
        }
    }
}

impl ChainValidationWork {
    pub(super) fn required_proposals(&self) -> Result<Vec<ProposalId>, ChainValidationError> {
        let mut proposals = Vec::new();
        proposals
            .try_reserve(
                self.status_subjects
                    .len()
                    .checked_add(self.proposal_subjects.len())
                    .ok_or(ChainValidationError::Allocation)?,
            )
            .map_err(|_| ChainValidationError::Allocation)?;
        proposals.extend(
            self.status_subjects
                .iter()
                .map(|subject| subject.proposal.clone()),
        );
        proposals.extend(
            self.proposal_subjects
                .iter()
                .map(|subject| subject.proposal.clone()),
        );
        Ok(canonical_proposals(proposals))
    }

    /// Validate every selected proposal subject against the exact snapshot
    /// whose tip is named by `new_view`. Proposal-table lookup is immutable
    /// in-memory work and performs no database or VM I/O.
    pub(super) fn validate(
        self,
        snapshot: &Snapshot,
    ) -> Result<ChainTransitionReceipt, ChainValidationError> {
        if self.new_view.tip().0 != snapshot.tip_hash() {
            return Err(ChainValidationError::SnapshotMismatch);
        }
        let required = self.required_proposals()?;
        let mut positions = Vec::new();
        positions
            .try_reserve(required.len())
            .map_err(|_| ChainValidationError::Allocation)?;
        positions.extend(required.into_iter().map(|proposal| {
            let position = if snapshot.proposals().contains_proposed(&proposal.0) {
                ProposalWindowPosition::Proposed
            } else if snapshot.proposals().contains_gap(&proposal.0) {
                ProposalWindowPosition::Gap
            } else {
                ProposalWindowPosition::Outside
            };
            (proposal, position)
        }));
        self.validate_positions(positions)
    }

    /// Foundation seam for supplying exact synthetic proposal positions. The
    /// positions must cover exactly the requested ids; extra or missing facts
    /// cannot be silently ignored.
    #[cfg(test)]
    pub(super) fn validate_for_foundation(
        self,
        positions: Vec<(ProposalId, ProposalWindowPosition)>,
    ) -> Result<ChainTransitionReceipt, ChainValidationError> {
        self.validate_positions(positions)
    }

    fn validate_positions(
        self,
        mut positions: Vec<(ProposalId, ProposalWindowPosition)>,
    ) -> Result<ChainTransitionReceipt, ChainValidationError> {
        positions.sort_unstable_by(|left, right| left.0.0.cmp(&right.0.0));
        if positions.windows(2).any(|pair| match pair {
            [left, right] => left.0 == right.0,
            _ => false,
        }) {
            return Err(ChainValidationError::DuplicateProposalPosition);
        }
        let required = self.required_proposals()?;
        if positions
            .iter()
            .any(|(proposal, _)| required.binary_search(proposal).is_err())
        {
            return Err(ChainValidationError::UnexpectedProposalPosition);
        }
        if required.iter().any(|proposal| {
            positions
                .binary_search_by(|item| item.0.cmp(proposal))
                .is_err()
        }) {
            return Err(ChainValidationError::MissingProposalPosition);
        }

        let mut statuses = Vec::new();
        statuses
            .try_reserve(self.status_subjects.len())
            .map_err(|_| ChainValidationError::Allocation)?;
        for subject in self.status_subjects {
            let position = positions
                .binary_search_by(|item| item.0.cmp(&subject.proposal))
                .ok()
                .and_then(|index| positions.get(index))
                .map(|item| item.1)
                .ok_or(ChainValidationError::MissingProposalPosition)?;
            let after = reconcile_proposal_status(
                subject.before,
                position,
                self.packaging,
                subject.baseline,
            );
            if after != subject.before {
                statuses.push(ChainStatusChange {
                    hash: subject.hash,
                    expected: subject.expected,
                    after,
                });
            }
        }
        statuses.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));

        let mut removals = self.removals;
        removals
            .try_reserve(self.proposal_subjects.len())
            .map_err(|_| ChainValidationError::Allocation)?;
        let mut proposal_demotions = Vec::new();
        proposal_demotions
            .try_reserve(self.proposal_subjects.len())
            .map_err(|_| ChainValidationError::Allocation)?;
        for subject in self.proposal_subjects {
            let position = positions
                .binary_search_by(|item| item.0.cmp(&subject.proposal))
                .ok()
                .and_then(|index| positions.get(index))
                .map(|item| item.1)
                .ok_or(ChainValidationError::MissingProposalPosition)?;
            if position != ProposalWindowPosition::Outside {
                continue;
            }
            match subject.base {
                ProposalBase::Remote(_) => proposal_demotions.push(ChainProposalDemotion {
                    hash: subject.hash,
                    expected: subject.expected,
                }),
                ProposalBase::Trusted => removals.push(ChainRemoval::ProposalWindowExpired {
                    hash: subject.hash,
                    expected: subject.expected,
                }),
            }
        }
        removals.sort_unstable_by(|left, right| left.hash().cmp(right.hash()));
        proposal_demotions.sort_unstable_by(|left, right| left.hash.cmp(&right.hash));

        let mut recoveries = Vec::new();
        recoveries
            .try_reserve(self.recoveries.len())
            .map_err(|_| ChainValidationError::Allocation)?;
        for recovery in self.recoveries {
            match recovery {
                ChainRecoveryWork::Trusted {
                    transaction,
                    expected,
                } => {
                    recoveries.push(ChainRecoveryReceipt::Trusted {
                        admission: ValidatedAdmission::recovery(transaction, self.generation)
                            .map_err(ChainValidationError::RecoveryAdmission)?,
                        expected,
                    });
                }
                ChainRecoveryWork::RequeueExisting { hash, expected } => {
                    recoveries.push(ChainRecoveryReceipt::RequeueExisting { hash, expected });
                }
            }
        }
        Ok(ChainTransitionReceipt {
            generation: self.generation,
            old_view: self.old_view,
            new_view: self.new_view,
            committed: self.committed,
            removals,
            recoveries,
            statuses,
            proposal_demotions,
            available: self.available,
            lost: self.lost,
        })
    }
}

fn reconcile_proposal_status(
    before: AcceptedStatus,
    position: ProposalWindowPosition,
    packaging: ChainPackagingMode,
    baseline: ProposalStatusBaseline,
) -> AcceptedStatus {
    let baseline = match baseline {
        ProposalStatusBaseline::Current => before,
        ProposalStatusBaseline::DetachedProposal => AcceptedStatus::Pending,
    };
    match position {
        ProposalWindowPosition::Proposed => match packaging {
            ChainPackagingMode::Package => AcceptedStatus::Proposed,
            ChainPackagingMode::ObserveOnly => baseline,
        },
        ProposalWindowPosition::Gap => match baseline {
            AcceptedStatus::Pending => match packaging {
                ChainPackagingMode::Package => AcceptedStatus::Gap,
                ChainPackagingMode::ObserveOnly => AcceptedStatus::Pending,
            },
            AcceptedStatus::Gap | AcceptedStatus::Proposed => AcceptedStatus::Gap,
        },
        ProposalWindowPosition::Outside => AcceptedStatus::Pending,
    }
}

/// Sealed result of validating the affected slice against one new snapshot.
/// It is move-only: Plan either consumes it atomically or returns stale.
#[derive(Debug)]
#[must_use = "a chain transition receipt must be applied or discarded as stale"]
pub(super) struct ChainTransitionReceipt {
    pub(super) generation: PoolGeneration,
    pub(super) old_view: ChainViewId,
    pub(super) new_view: ChainViewId,
    pub(super) committed: Vec<RawTxHash>,
    pub(super) removals: Vec<ChainRemoval>,
    pub(super) recoveries: Vec<ChainRecoveryReceipt>,
    pub(super) statuses: Vec<ChainStatusChange>,
    pub(super) proposal_demotions: Vec<ChainProposalDemotion>,
    pub(super) available: Vec<super::state::DependencyKey>,
    pub(super) lost: Vec<super::state::DependencyKey>,
}

#[derive(Clone, Debug)]
pub(super) struct ChainStatusChange {
    pub(super) hash: RawTxHash,
    pub(super) expected: EntryVersion,
    pub(super) after: AcceptedStatus,
}

#[derive(Clone, Debug)]
pub(super) struct ChainProposalDemotion {
    pub(super) hash: RawTxHash,
    pub(super) expected: ExpectedPreAcceptedOwner,
}
