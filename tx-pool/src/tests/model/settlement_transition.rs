//! Total source-classification relation for one move-only compute settlement.
//!
//! The active capability fixes the checked-out view, dependency cut and
//! payload policy. Classification may retain its result, requeue the owner or
//! emit a typed structural fault; it cannot retry an unchanged observation.

use super::{
    dependency_progress::ModelDependencyCut,
    evidence_transition::{
        ModelEvidenceFrontier, ModelEvidenceIdentity, ModelEvidenceView, ModelKnownDependencies,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelPayloadPolicy {
    RemoteDeclaredCycles,
    Trusted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSettlementEvidence {
    pub(crate) payload_identity: ModelEvidenceIdentity,
    pub(crate) sealed_witness: u8,
    pub(crate) view: ModelEvidenceView,
    pub(crate) dependency_cut: ModelDependencyCut,
    pub(crate) dependencies: ModelKnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelMissingSettlement {
    pub(crate) dependencies: ModelKnownDependencies,
    pub(crate) missing: ModelKnownDependencies,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSettlementRejection {
    ChainBound,
    ResourceBound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelSettlementNext {
    QueuedVerify(ModelSettlementEvidence),
    Waiting(ModelMissingSettlement),
    Ready(ModelSettlementEvidence),
    Rejected(ModelSettlementRejection),
    VerificationRejected(ModelSettlementEvidence),
    Retry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSettlementCut {
    pub(crate) authority_view: ModelEvidenceView,
    pub(crate) owner_identity: ModelEvidenceIdentity,
    pub(crate) baseline_dependencies: ModelKnownDependencies,
    pub(crate) current_policy: ModelPayloadPolicy,
    pub(crate) active_view: ModelEvidenceView,
    pub(crate) active_dependency_cut: ModelDependencyCut,
    pub(crate) active_policy: ModelPayloadPolicy,
    pub(crate) frontier: ModelEvidenceFrontier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSettlementFault {
    MembershipProjection,
    DependencyProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSettlementObservation {
    QueuedResolve,
    QueuedVerify,
    Waiting,
    Ready,
    Rejected,
    Fault(ModelSettlementFault),
}

impl ModelSettlementCut {
    fn chain_state_is_current(&self) -> bool {
        self.authority_view == self.active_view
    }

    fn evidence_fault(
        &self,
        evidence: &ModelSettlementEvidence,
        require_sealed_witness: bool,
    ) -> Option<ModelSettlementObservation> {
        if evidence.payload_identity != self.owner_identity
            || evidence.view != self.active_view
            || (require_sealed_witness && evidence.sealed_witness != self.owner_identity.witness)
        {
            return Some(ModelSettlementObservation::Fault(
                ModelSettlementFault::MembershipProjection,
            ));
        }
        if evidence.dependency_cut != self.active_dependency_cut {
            return Some(ModelSettlementObservation::Fault(
                ModelSettlementFault::DependencyProjection,
            ));
        }
        None
    }

    pub(crate) fn classify(&self, next: &ModelSettlementNext) -> ModelSettlementObservation {
        match next {
            ModelSettlementNext::QueuedVerify(resolved) => {
                if let Some(fault) = self.evidence_fault(resolved, false) {
                    return fault;
                }
                if !self.chain_state_is_current() {
                    return ModelSettlementObservation::QueuedResolve;
                }
                if self.frontier.resolution_is_current(
                    &self.baseline_dependencies,
                    &resolved.dependencies,
                    self.active_dependency_cut,
                ) {
                    ModelSettlementObservation::QueuedVerify
                } else {
                    ModelSettlementObservation::QueuedResolve
                }
            }
            ModelSettlementNext::Waiting(missing) => {
                if self.chain_state_is_current()
                    && self.frontier.missing_result_is_current(
                        &self.baseline_dependencies,
                        &missing.dependencies,
                        &missing.missing,
                        self.active_dependency_cut,
                    )
                {
                    ModelSettlementObservation::Waiting
                } else {
                    ModelSettlementObservation::QueuedResolve
                }
            }
            ModelSettlementNext::Ready(verified) => {
                if let Some(fault) = self.evidence_fault(verified, true) {
                    return fault;
                }
                if self.frontier.resolution_is_current(
                    &self.baseline_dependencies,
                    &verified.dependencies,
                    self.active_dependency_cut,
                ) {
                    ModelSettlementObservation::Ready
                } else {
                    ModelSettlementObservation::QueuedResolve
                }
            }
            ModelSettlementNext::Rejected(rejection) => {
                if self.chain_state_is_current()
                    || *rejection == ModelSettlementRejection::ResourceBound
                {
                    ModelSettlementObservation::Rejected
                } else {
                    ModelSettlementObservation::QueuedResolve
                }
            }
            ModelSettlementNext::VerificationRejected(resolved) => {
                if let Some(fault) = self.evidence_fault(resolved, false) {
                    return fault;
                }
                if self.current_policy == self.active_policy {
                    if self.chain_state_is_current() {
                        ModelSettlementObservation::Rejected
                    } else {
                        ModelSettlementObservation::QueuedResolve
                    }
                } else if self.active_policy == ModelPayloadPolicy::RemoteDeclaredCycles
                    && self.current_policy == ModelPayloadPolicy::Trusted
                {
                    if !self.chain_state_is_current() {
                        return ModelSettlementObservation::QueuedResolve;
                    }
                    if self.frontier.resolution_is_current(
                        &self.baseline_dependencies,
                        &resolved.dependencies,
                        self.active_dependency_cut,
                    ) {
                        ModelSettlementObservation::QueuedVerify
                    } else {
                        ModelSettlementObservation::QueuedResolve
                    }
                } else {
                    ModelSettlementObservation::Fault(ModelSettlementFault::MembershipProjection)
                }
            }
            ModelSettlementNext::Retry => ModelSettlementObservation::QueuedResolve,
        }
    }
}
