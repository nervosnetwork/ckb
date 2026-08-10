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
    RemoteDeclaredCycles(u8),
    Trusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelPayloadPolicyEvolution {
    Unchanged,
    RemoteToTrusted,
    Invalid,
}

impl ModelPayloadPolicy {
    pub(crate) fn evolution_to(self, current: Self) -> ModelPayloadPolicyEvolution {
        match (self, current) {
            (Self::RemoteDeclaredCycles(active), Self::RemoteDeclaredCycles(current))
                if active == current =>
            {
                ModelPayloadPolicyEvolution::Unchanged
            }
            (Self::Trusted, Self::Trusted) => ModelPayloadPolicyEvolution::Unchanged,
            (Self::RemoteDeclaredCycles(_), Self::Trusted) => {
                ModelPayloadPolicyEvolution::RemoteToTrusted
            }
            (Self::RemoteDeclaredCycles(_), Self::RemoteDeclaredCycles(_))
            | (Self::Trusted, Self::RemoteDeclaredCycles(_)) => {
                ModelPayloadPolicyEvolution::Invalid
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSettlementEvidence {
    pub(crate) payload_identity: ModelEvidenceIdentity,
    pub(crate) view: ModelEvidenceView,
    pub(crate) dependency_cut: ModelDependencyCut,
    pub(crate) dependencies: ModelKnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelSettlementOrigin {
    pub(crate) payload_identity: ModelEvidenceIdentity,
    pub(crate) view: ModelEvidenceView,
    pub(crate) dependency_cut: ModelDependencyCut,
    pub(crate) payload_policy: ModelPayloadPolicy,
}

impl ModelSettlementOrigin {
    pub(crate) fn evidence(&self, dependencies: ModelKnownDependencies) -> ModelSettlementEvidence {
        ModelSettlementEvidence {
            payload_identity: self.payload_identity,
            view: self.view,
            dependency_cut: self.dependency_cut,
            dependencies,
        }
    }
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
    pub(crate) active: ModelSettlementOrigin,
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
        self.authority_view == self.active.view
    }

    fn evidence_fault(
        &self,
        evidence: &ModelSettlementEvidence,
    ) -> Option<ModelSettlementObservation> {
        if evidence.payload_identity != self.owner_identity || evidence.view != self.active.view {
            return Some(ModelSettlementObservation::Fault(
                ModelSettlementFault::MembershipProjection,
            ));
        }
        if evidence.dependency_cut != self.active.dependency_cut {
            return Some(ModelSettlementObservation::Fault(
                ModelSettlementFault::DependencyProjection,
            ));
        }
        None
    }

    pub(crate) fn classify(&self, next: &ModelSettlementNext) -> ModelSettlementObservation {
        if !self
            .frontier
            .proof_is_current(&self.baseline_dependencies, self.active.dependency_cut)
        {
            return ModelSettlementObservation::QueuedResolve;
        }
        match next {
            ModelSettlementNext::QueuedVerify(resolved) => {
                if let Some(fault) = self.evidence_fault(resolved) {
                    return fault;
                }
                if !self.chain_state_is_current() {
                    return ModelSettlementObservation::QueuedResolve;
                }
                if self.frontier.resolution_is_current(
                    &self.baseline_dependencies,
                    &resolved.dependencies,
                    self.active.dependency_cut,
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
                        self.active.dependency_cut,
                    )
                {
                    ModelSettlementObservation::Waiting
                } else {
                    ModelSettlementObservation::QueuedResolve
                }
            }
            ModelSettlementNext::Ready(verified) => {
                if let Some(fault) = self.evidence_fault(verified) {
                    return fault;
                }
                if self.frontier.resolution_is_current(
                    &self.baseline_dependencies,
                    &verified.dependencies,
                    self.active.dependency_cut,
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
                if let Some(fault) = self.evidence_fault(resolved) {
                    return fault;
                }
                match self.active.payload_policy.evolution_to(self.current_policy) {
                    ModelPayloadPolicyEvolution::Unchanged => {
                        if self.chain_state_is_current() {
                            ModelSettlementObservation::Rejected
                        } else {
                            ModelSettlementObservation::QueuedResolve
                        }
                    }
                    ModelPayloadPolicyEvolution::RemoteToTrusted => {
                        if !self.chain_state_is_current() {
                            return ModelSettlementObservation::QueuedResolve;
                        }
                        if self.frontier.resolution_is_current(
                            &self.baseline_dependencies,
                            &resolved.dependencies,
                            self.active.dependency_cut,
                        ) {
                            ModelSettlementObservation::QueuedVerify
                        } else {
                            ModelSettlementObservation::QueuedResolve
                        }
                    }
                    ModelPayloadPolicyEvolution::Invalid => ModelSettlementObservation::Fault(
                        ModelSettlementFault::MembershipProjection,
                    ),
                }
            }
            ModelSettlementNext::Retry => ModelSettlementObservation::QueuedResolve,
        }
    }
}
