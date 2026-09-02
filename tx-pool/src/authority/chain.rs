//! Typed chain and final-admission evidence.
//!
//! These receipts deliberately separate reusable transaction content and
//! script work from location, proposal and time facts invalidated by a tip
//! change. Constructors stay inside the authority boundary so callers cannot
//! assemble a membership proof from unrelated booleans or snapshots.

use super::rejection::{CommittedPublicReject, DirectRejectionValidity};
use super::resolver::AcceptedOverlay;
use super::state::{
    AcceptedAtMillis, AcceptedStatus, AsyncProcessStart, CandidateMetrics, ChainViewId,
    DependencyCut, EntryVersion, PoolGeneration, PreAcceptedSource, ProposalBase, ProposalId,
    RawTxHash, RecoveryAdmissionError, ResolvedPayload, ValidatedAdmission, VerifiedFacts,
};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::TransactionView,
    packed::{Byte32, OutPoint, ProposalShortId},
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
    // Positive chain-location evidence remains reusable while this view's tip
    // is current. Owning the complete view also makes the later verification
    // context provenance construction-safe instead of pairing two values at
    // runtime.
    view: ChainViewId,
    chain_inputs: Arc<Vec<OutPoint>>,
    chain_dependencies: Arc<Vec<OutPoint>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CellLocationReceiptError {
    Allocation,
    Arithmetic,
}

impl CellLocationReceipt {
    /// Derive tx-pool-only positive location evidence from the exact resolved
    /// input metadata. A chain input has `transaction_info`; a pool-produced
    /// input does not. This receipt is never used by block validation, whose
    /// resolver and liveness rules remain independent.
    pub(super) fn from_resolution(
        view: ChainViewId,
        payload: &ResolvedPayload,
    ) -> Result<Self, CellLocationReceiptError> {
        let resolved = payload.resolved_transaction();
        let mut chain_inputs = Vec::new();
        chain_inputs
            .try_reserve_exact(resolved.resolved_inputs.len())
            .map_err(|_| CellLocationReceiptError::Allocation)?;
        chain_inputs.extend(
            resolved
                .resolved_inputs
                .iter()
                .filter(|cell| cell.transaction_info.is_some())
                .map(|cell| cell.out_point.clone()),
        );
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        let dependency_count = resolved
            .resolved_cell_deps
            .len()
            .checked_add(resolved.resolved_dep_groups.len())
            .ok_or(CellLocationReceiptError::Arithmetic)?;
        let mut chain_dependencies = Vec::new();
        chain_dependencies
            .try_reserve_exact(dependency_count)
            .map_err(|_| CellLocationReceiptError::Allocation)?;
        chain_dependencies.extend(
            resolved
                .resolved_cell_deps
                .iter()
                .chain(resolved.resolved_dep_groups.iter())
                .filter(|cell| cell.transaction_info.is_some())
                .map(|cell| cell.out_point.clone()),
        );
        chain_dependencies.sort_unstable();
        chain_dependencies.dedup();
        Ok(Self {
            view,
            chain_inputs: Arc::new(chain_inputs),
            chain_dependencies: Arc::new(chain_dependencies),
        })
    }

    /// Bind the feature-internal synthetic `TxEntry` fixture to one authority
    /// view. Historical `PlugEntry` intentionally bypasses chain resolution,
    /// so every declared input/dependency is positive fixture evidence. The
    /// unforgeable seal keeps that premise out of every production admission
    /// and block-validation path.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn from_internal_plug(
        _seal: super::internal::InternalPlugSeal,
        view: ChainViewId,
        payload: &ResolvedPayload,
    ) -> Result<Self, ()> {
        let footprint = &payload.footprint;
        let mut chain_inputs = Vec::new();
        chain_inputs
            .try_reserve(footprint.inputs().len())
            .map_err(|_| ())?;
        chain_inputs.extend(footprint.inputs().iter().cloned());
        let mut chain_dependencies = Vec::new();
        chain_dependencies
            .try_reserve(footprint.dependencies().len())
            .map_err(|_| ())?;
        chain_dependencies.extend(footprint.dependencies().iter().cloned());
        Ok(Self {
            view,
            chain_inputs: Arc::new(chain_inputs),
            chain_dependencies: Arc::new(chain_dependencies),
        })
    }

    pub(super) fn view(&self) -> &ChainViewId {
        &self.view
    }

    pub(super) fn into_view(self) -> ChainViewId {
        self.view
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
    chain_inputs: Arc<Vec<OutPoint>>,
    chain_dependencies: Arc<Vec<OutPoint>>,
    time: TimeContextReceipt,
}

impl VerificationContextReceipt {
    pub(super) fn from_validation(location: CellLocationReceipt, time: TimeContextReceipt) -> Self {
        Self::from_location(location, time)
    }

    /// Consume location evidence created inside the same sealed resolution
    /// fact as the payload. Unlike final-admission refresh, this constructor
    /// cannot receive an independently sampled view, so no runtime mismatch
    /// state is representable on the verification path.
    pub(super) fn from_resolved(
        _seal: super::work::VerificationSeal,
        location: CellLocationReceipt,
        time: TimeContextReceipt,
    ) -> Self {
        Self::from_location(location, time)
    }

    fn from_location(location: CellLocationReceipt, time: TimeContextReceipt) -> Self {
        Self {
            view: location.view,
            chain_inputs: location.chain_inputs,
            chain_dependencies: location.chain_dependencies,
            time,
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
    fn from_position(position: ProposalWindowPosition) -> Self {
        let status = match position {
            ProposalWindowPosition::Proposed => AcceptedStatus::Proposed,
            ProposalWindowPosition::Gap => AcceptedStatus::Gap,
            ProposalWindowPosition::Outside => AcceptedStatus::Pending,
        };
        Self { status }
    }

    #[cfg(any(test, feature = "internal"))]
    pub(super) fn from_internal_status(status: AcceptedStatus) -> Self {
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

/// Minimal first-cut capability used only to allocate final-admission scratch
/// outside the authority guard. The second OCC cut still captures the full
/// [`FinalAdmissionWork`], so this value owns no reusable verification facts.
#[derive(Clone, Debug)]
pub(super) struct FinalAdmissionPreparation {
    key: RawTxHash,
    expected: EntryVersion,
    view: ChainViewId,
    payload: Arc<ResolvedPayload>,
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
pub(super) enum DirectAdmissionError {
    TransactionIdentityMismatch,
}

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

    #[cfg(test)]
    pub(in crate::authority) fn payload(&self) -> &ResolvedPayload {
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

    #[cfg(test)]
    pub(in crate::authority) fn preparation(&self) -> FinalAdmissionPreparation {
        FinalAdmissionPreparation::new(
            self.key.clone(),
            self.expected,
            self.validation.view.clone(),
            Arc::clone(self.validation.verified.payload_arc()),
        )
    }

    pub(super) fn into_validation_parts(
        self,
    ) -> (RawTxHash, EntryVersion, MembershipValidationWork) {
        (self.key, self.expected, self.validation)
    }
}

impl FinalAdmissionPreparation {
    pub(super) fn new(
        key: RawTxHash,
        expected: EntryVersion,
        view: ChainViewId,
        payload: Arc<ResolvedPayload>,
    ) -> Self {
        Self {
            key,
            expected,
            view,
            payload,
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

    pub(super) fn payload(&self) -> &ResolvedPayload {
        &self.payload
    }
}

impl DirectAdmissionWork {
    pub(super) fn new(
        tx: Arc<TransactionView>,
        verified: VerifiedFacts,
    ) -> Result<Self, DirectAdmissionError> {
        if verified.payload().identity() != &super::state::TxIdentity::from_transaction(&tx) {
            return Err(DirectAdmissionError::TransactionIdentityMismatch);
        }
        let view = verified.chain_view().clone();
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct MembershipReceipt {
    proof: AcceptedProof,
    proposal: ProposalContextReceipt,
    accepted_at: AcceptedAtMillis,
    async_process_start: Option<AsyncProcessStart>,
}

impl MembershipReceipt {
    pub(super) fn from_validation(
        _seal: super::validation::AdmissionValidationSeal,
        verified: VerifiedFacts,
        sensitivity: AcceptedChainSensitivity,
        proposal: ProposalContextReceipt,
        accepted_at: AcceptedAtMillis,
    ) -> Self {
        let (verified, async_process_start) = verified.into_accepted();
        Self {
            proof: AcceptedProof {
                verified,
                sensitivity,
            },
            proposal,
            accepted_at,
            async_process_start,
        }
    }

    fn view(&self) -> &ChainViewId {
        self.proof.admission_view()
    }

    fn proof(&self) -> &AcceptedProof {
        &self.proof
    }

    fn into_parts(
        self,
    ) -> (
        AcceptedProof,
        ProposalContextReceipt,
        AcceptedAtMillis,
        Option<AsyncProcessStart>,
    ) {
        (
            self.proof,
            self.proposal,
            self.accepted_at,
            self.async_process_start,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "final admission evidence must be applied or discarded as stale"]
pub(super) struct FinalAdmissionReceipt {
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
        expected: EntryVersion,
        membership: MembershipReceipt,
        payload_relation: ReadyPayloadRelation,
    ) -> Self {
        Self {
            expected,
            membership,
            payload_relation,
        }
    }

    pub(super) fn key(&self) -> &RawTxHash {
        &self.membership.proof().payload().identity().raw
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
    ) -> (
        AcceptedProof,
        ProposalContextReceipt,
        AcceptedAtMillis,
        Option<AsyncProcessStart>,
    ) {
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

    /// Construct the complete synthetic admission receipt owned by the
    /// feature-internal `PlugEntry` adapter. Keeping this as one sealed
    /// constructor prevents tests from assembling partially inconsistent
    /// proof, proposal, and timestamp fields.
    #[cfg(any(test, feature = "internal"))]
    pub(super) fn from_internal_plug(
        _seal: super::internal::InternalPlugSeal,
        tx: Arc<TransactionView>,
        verified: VerifiedFacts,
        status: AcceptedStatus,
        accepted_at: AcceptedAtMillis,
    ) -> Self {
        let (verified, async_process_start) = verified.into_accepted();
        Self {
            tx,
            membership: MembershipReceipt {
                proof: AcceptedProof {
                    verified,
                    sensitivity: AcceptedChainSensitivity::TipContext,
                },
                proposal: ProposalContextReceipt::from_internal_status(status),
                accepted_at,
                async_process_start,
            },
        }
    }

    pub(super) fn key(&self) -> &RawTxHash {
        &self.membership.proof().payload().identity().raw
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

    pub(super) fn retry_transaction(&self) -> Arc<TransactionView> {
        Arc::clone(&self.tx)
    }

    pub(super) fn into_membership_parts(
        self,
    ) -> (
        Arc<TransactionView>,
        AcceptedProof,
        ProposalContextReceipt,
        AcceptedAtMillis,
        Option<AsyncProcessStart>,
    ) {
        let (proof, proposal, accepted_at, async_process_start) = self.membership.into_parts();
        (self.tx, proof, proposal, accepted_at, async_process_start)
    }
}

/// Immutable direct-validation subject. It contains no membership, resource,
/// clock, or effect capability; Local may later consume it through the
/// authority planner, while TestAccept can return the same evaluation without
/// acquiring mutation authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectAdmissionSubject {
    tx: Arc<TransactionView>,
    validity: DirectRejectionValidity,
}

impl DirectAdmissionSubject {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        tx: Arc<TransactionView>,
        view: ChainViewId,
        accepted_reads: AcceptedOverlay,
    ) -> Self {
        Self {
            tx,
            validity: DirectRejectionValidity::AcceptedReads {
                view,
                reads: accepted_reads,
            },
        }
    }

    pub(super) fn validity(&self) -> &DirectRejectionValidity {
        &self.validity
    }

    pub(super) fn into_parts(self) -> (Arc<TransactionView>, DirectRejectionValidity) {
        (self.tx, self.validity)
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

    pub(super) fn into_parts(self) -> (DirectAdmissionSubject, CommittedPublicReject) {
        (self.subject, self.reason)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DirectAdmissionRetry {
    tx: Arc<TransactionView>,
}

impl DirectAdmissionRetry {
    pub(super) fn new(
        _seal: super::validation::AdmissionValidationSeal,
        tx: Arc<TransactionView>,
    ) -> Self {
        Self { tx }
    }

    pub(super) fn into_transaction(self) -> Arc<TransactionView> {
        self.tx
    }
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

pub(super) fn proposal_window_position(
    snapshot: &Snapshot,
    proposal: &ProposalShortId,
) -> ProposalWindowPosition {
    if snapshot.proposals().contains_proposed(proposal) {
        ProposalWindowPosition::Proposed
    } else if snapshot.proposals().contains_gap(proposal) {
        ProposalWindowPosition::Gap
    } else {
        ProposalWindowPosition::Outside
    }
}

pub(super) fn proposal_context_receipt(
    snapshot: &Snapshot,
    proposal: &ProposalShortId,
) -> ProposalContextReceipt {
    ProposalContextReceipt::from_position(proposal_window_position(snapshot, proposal))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainFactsError {
    DuplicateTransaction,
    DuplicateHeader,
    Allocation,
}

/// Exact proposal-position change between the paired old and new snapshots.
/// The constructor owns the complete retained proposal universe, so callers
/// cannot omit a promotion, expiry, reproposal, or uncle-derived position.
#[derive(Debug)]
pub(super) struct ProposalTransitionFacts {
    pub(super) changed: Vec<ProposalId>,
}

impl ProposalTransitionFacts {
    pub(super) fn between(
        old_snapshot: &Snapshot,
        new_snapshot: &Snapshot,
    ) -> Result<Self, ChainFactsError> {
        let old = old_snapshot.proposals();
        let new = new_snapshot.proposals();
        let mut changed = Vec::new();
        // The proposal-table accepts its sparse receipt only for this exact
        // predecessor identity; a skipped/reordered snapshot or reorg instead
        // merges both complete ordered id universes. Both paths yield every
        // and only externally visible position change in canonical id order.
        new.try_for_each_changed_from(old, |proposal| {
            changed
                .try_reserve(1)
                .map_err(|_| ChainFactsError::Allocation)?;
            changed.push(ProposalId(proposal));
            Ok(())
        })?;

        Ok(Self { changed })
    }
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

/// Canonical block-derived payload shared by production chain commands and
/// the transition compiler. It contains no authority revision or snapshot
/// policy, so a failed Plan can return the same move-only command for a later
/// attempt without rebuilding or cloning the block transaction set.
#[derive(Debug)]
pub(super) struct CanonicalChainFacts {
    pub(super) attached: Vec<TransactionView>,
    pub(super) detached: Vec<TransactionView>,
    pub(super) relocated: Vec<RawTxHash>,
    pub(super) attached_headers: Vec<Byte32>,
    pub(super) detached_headers: Vec<Byte32>,
}

/// One authority-context binding over immutable canonical chain facts. The
/// view cannot outlive its command, while the resulting validation work owns
/// every fact needed after the read-only compiler returns.
#[derive(Debug)]
pub(super) struct ChainTransitionFactsView<'facts> {
    pub(super) new_view: ChainViewId,
    pub(super) attached: &'facts [TransactionView],
    pub(super) detached: &'facts [TransactionView],
    pub(super) relocated: &'facts [RawTxHash],
    pub(super) attached_headers: &'facts [Byte32],
    pub(super) detached_headers: &'facts [Byte32],
    pub(super) changed_proposals: &'facts [ProposalId],
    pub(super) accepted_validity: AcceptedValidityTransition,
}

impl CanonicalChainFacts {
    pub(super) fn from_chain_update(blocks: ChainBlockChanges) -> Result<Self, ChainFactsError> {
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
            attached,
            detached,
            relocated,
            attached_headers,
            detached_headers,
        })
    }

    pub(super) fn bind<'facts>(
        &'facts self,
        new_view: ChainViewId,
        accepted_validity: AcceptedValidityTransition,
        proposals: &'facts ProposalTransitionFacts,
    ) -> ChainTransitionFactsView<'facts> {
        ChainTransitionFactsView {
            new_view,
            attached: &self.attached,
            detached: &self.detached,
            relocated: &self.relocated,
            attached_headers: &self.attached_headers,
            detached_headers: &self.detached_headers,
            changed_proposals: &proposals.changed,
            accepted_validity,
        }
    }
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
}

fn canonical_transactions(
    mut transactions: Vec<TransactionView>,
) -> Result<Vec<TransactionView>, ChainFactsError> {
    // Block extraction is allowed to pass complete transaction lists; the
    // chain authority, rather than every caller, owns the cellbase exclusion.
    transactions.retain(|transaction| !transaction.is_cellbase());
    transactions.sort_unstable_by_key(TransactionView::hash);
    if transactions
        .array_windows::<2>()
        .any(|[left, right]| left.hash() == right.hash())
    {
        return Err(ChainFactsError::DuplicateTransaction);
    }
    Ok(transactions)
}

fn canonical_headers(mut headers: Vec<Byte32>) -> Result<Vec<Byte32>, ChainFactsError> {
    headers.sort_unstable();
    if headers
        .array_windows::<2>()
        .any(|[left, right]| left == right)
    {
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
}

#[derive(Clone, Debug)]
pub(super) struct ChainProposalSubject {
    pub(super) hash: RawTxHash,
    pub(super) expected: ExpectedPreAcceptedOwner,
    pub(super) proposal: ProposalId,
    pub(super) base: ProposalBase,
}

/// Read-only validation work selected from one coherent authority cut.
///
/// Production currently validates this bounded owner slice while retaining an
/// upgradable authority read guard. That serial cut prevents continuous
/// admission from starving an ordered chain transition; fork traversal and
/// detached payload preparation remain outside the guard. This value owns no
/// mutable authority and performs no database or VM work.
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
    /// Chain-layer availability facts. Plan must combine these with the final
    /// Accepted-membership projection before publishing dependency levels.
    pub(super) chain_available: Vec<super::state::DependencyKey>,
    pub(super) chain_lost: Vec<super::state::DependencyKey>,
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
    RecoveryAdmission(RecoveryAdmissionError),
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
            let position = proposal_window_position(snapshot, &proposal.0);
            (proposal, position)
        }));
        self.validate_positions(positions)
    }

    fn validate_positions(
        self,
        mut positions: Vec<(ProposalId, ProposalWindowPosition)>,
    ) -> Result<ChainTransitionReceipt, ChainValidationError> {
        positions.sort_unstable_by(|left, right| left.0.0.cmp(&right.0.0));
        if positions
            .array_windows::<2>()
            .any(|[left, right]| left.0 == right.0)
        {
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
            let after = ProposalContextReceipt::from_position(position);
            if after.status() != subject.before {
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
            chain_available: self.chain_available,
            chain_lost: self.chain_lost,
        })
    }
}

#[cfg(test)]
#[path = "tests/support/chain.rs"]
pub(in crate::authority) mod test_support;

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
    pub(super) chain_available: Vec<super::state::DependencyKey>,
    pub(super) chain_lost: Vec<super::state::DependencyKey>,
}

#[derive(Clone, Debug)]
pub(super) struct ChainStatusChange {
    pub(super) hash: RawTxHash,
    pub(super) expected: EntryVersion,
    pub(super) after: ProposalContextReceipt,
}

#[derive(Clone, Debug)]
pub(super) struct ChainProposalDemotion {
    pub(super) hash: RawTxHash,
    pub(super) expected: ExpectedPreAcceptedOwner,
}
