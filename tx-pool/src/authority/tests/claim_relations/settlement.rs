//! Total source-classification relation for one move-only compute settlement.
//!
//! The active capability fixes the checked-out view, dependency cut and
//! payload policy. Classification may retain its result, requeue the owner or
//! emit a typed structural fault; it cannot retry an unchanged observation.

use super::{
    dependency::ClaimDependencyCut,
    evidence::{
        ClaimEvidenceFrontier, ClaimEvidenceIdentity, ClaimEvidenceView, ClaimKnownDependencies,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimSettlementRejection {
    ChainBound,
    ResourceBound,
}

type SettlementRejection = ClaimSettlementRejection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimVerifyCycleClass {
    Small,
    Large,
}

type VerifyCycleClass = ClaimVerifyCycleClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimPayloadPolicy {
    RemoteDeclaredCycles(u8),
    Trusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimPayloadPolicyEvolution {
    Unchanged,
    RemoteToTrusted,
    Invalid,
}

impl ClaimPayloadPolicy {
    pub(crate) fn evolution_to(self, current: Self) -> ClaimPayloadPolicyEvolution {
        match (self, current) {
            (Self::RemoteDeclaredCycles(active), Self::RemoteDeclaredCycles(current))
                if active == current =>
            {
                ClaimPayloadPolicyEvolution::Unchanged
            }
            (Self::Trusted, Self::Trusted) => ClaimPayloadPolicyEvolution::Unchanged,
            (Self::RemoteDeclaredCycles(_), Self::Trusted) => {
                ClaimPayloadPolicyEvolution::RemoteToTrusted
            }
            (Self::RemoteDeclaredCycles(_), Self::RemoteDeclaredCycles(_))
            | (Self::Trusted, Self::RemoteDeclaredCycles(_)) => {
                ClaimPayloadPolicyEvolution::Invalid
            }
        }
    }

    pub(crate) const fn verify_cycle_class(self, large_cycle_threshold: u8) -> VerifyCycleClass {
        match self {
            Self::RemoteDeclaredCycles(cycles) if cycles > large_cycle_threshold => {
                VerifyCycleClass::Large
            }
            Self::RemoteDeclaredCycles(_) | Self::Trusted => VerifyCycleClass::Small,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimSettlementEvidence {
    pub(crate) payload_identity: ClaimEvidenceIdentity,
    pub(crate) view: ClaimEvidenceView,
    pub(crate) dependency_cut: ClaimDependencyCut,
    pub(crate) dependencies: ClaimKnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimSettlementOrigin {
    pub(crate) payload_identity: ClaimEvidenceIdentity,
    pub(crate) view: ClaimEvidenceView,
    pub(crate) dependency_cut: ClaimDependencyCut,
    pub(crate) payload_policy: ClaimPayloadPolicy,
}

impl ClaimSettlementOrigin {
    pub(crate) fn evidence(&self, dependencies: ClaimKnownDependencies) -> ClaimSettlementEvidence {
        ClaimSettlementEvidence {
            payload_identity: self.payload_identity,
            view: self.view,
            dependency_cut: self.dependency_cut,
            dependencies,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimMissingSettlement {
    pub(crate) dependencies: ClaimKnownDependencies,
    pub(crate) missing: ClaimKnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClaimSettlementNext {
    QueuedVerify(ClaimSettlementEvidence),
    Waiting(ClaimMissingSettlement),
    Ready(ClaimSettlementEvidence),
    Rejected(SettlementRejection),
    VerificationRejected(ClaimSettlementEvidence),
    Retry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimSettlementCut {
    pub(crate) authority_view: ClaimEvidenceView,
    pub(crate) owner_identity: ClaimEvidenceIdentity,
    pub(crate) baseline_dependencies: ClaimKnownDependencies,
    pub(crate) current_policy: ClaimPayloadPolicy,
    pub(crate) active: ClaimSettlementOrigin,
    pub(crate) frontier: ClaimEvidenceFrontier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimSettlementFault {
    MembershipProjection,
    DependencyProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimSettlementObservation {
    QueuedResolve,
    QueuedVerify,
    Waiting,
    Ready,
    Rejected,
    Fault(ClaimSettlementFault),
}

impl ClaimSettlementCut {
    fn chain_state_is_current(&self) -> bool {
        self.authority_view == self.active.view
    }

    fn evidence_fault(
        &self,
        evidence: &ClaimSettlementEvidence,
    ) -> Option<ClaimSettlementObservation> {
        if evidence.payload_identity != self.owner_identity || evidence.view != self.active.view {
            return Some(ClaimSettlementObservation::Fault(
                ClaimSettlementFault::MembershipProjection,
            ));
        }
        if evidence.dependency_cut != self.active.dependency_cut {
            return Some(ClaimSettlementObservation::Fault(
                ClaimSettlementFault::DependencyProjection,
            ));
        }
        None
    }

    pub(crate) fn classify(&self, next: &ClaimSettlementNext) -> ClaimSettlementObservation {
        if !self
            .frontier
            .proof_is_current(&self.baseline_dependencies, self.active.dependency_cut)
        {
            return ClaimSettlementObservation::QueuedResolve;
        }
        match next {
            ClaimSettlementNext::QueuedVerify(resolved) => {
                if let Some(fault) = self.evidence_fault(resolved) {
                    return fault;
                }
                if !self.chain_state_is_current() {
                    return ClaimSettlementObservation::QueuedResolve;
                }
                if self.frontier.resolution_is_current(
                    &self.baseline_dependencies,
                    &resolved.dependencies,
                    self.active.dependency_cut,
                ) {
                    ClaimSettlementObservation::QueuedVerify
                } else {
                    ClaimSettlementObservation::QueuedResolve
                }
            }
            ClaimSettlementNext::Waiting(missing) => {
                if self.chain_state_is_current()
                    && self.frontier.missing_result_is_current(
                        &self.baseline_dependencies,
                        &missing.dependencies,
                        &missing.missing,
                        self.active.dependency_cut,
                    )
                {
                    ClaimSettlementObservation::Waiting
                } else {
                    ClaimSettlementObservation::QueuedResolve
                }
            }
            ClaimSettlementNext::Ready(verified) => {
                if let Some(fault) = self.evidence_fault(verified) {
                    return fault;
                }
                if self.frontier.resolution_is_current(
                    &self.baseline_dependencies,
                    &verified.dependencies,
                    self.active.dependency_cut,
                ) {
                    ClaimSettlementObservation::Ready
                } else {
                    ClaimSettlementObservation::QueuedResolve
                }
            }
            ClaimSettlementNext::Rejected(rejection) => {
                if self.chain_state_is_current() || *rejection == SettlementRejection::ResourceBound
                {
                    ClaimSettlementObservation::Rejected
                } else {
                    ClaimSettlementObservation::QueuedResolve
                }
            }
            ClaimSettlementNext::VerificationRejected(resolved) => {
                if let Some(fault) = self.evidence_fault(resolved) {
                    return fault;
                }
                match self.active.payload_policy.evolution_to(self.current_policy) {
                    ClaimPayloadPolicyEvolution::Unchanged => {
                        if self.chain_state_is_current() {
                            ClaimSettlementObservation::Rejected
                        } else {
                            ClaimSettlementObservation::QueuedResolve
                        }
                    }
                    ClaimPayloadPolicyEvolution::RemoteToTrusted => {
                        if !self.chain_state_is_current() {
                            return ClaimSettlementObservation::QueuedResolve;
                        }
                        if self.frontier.resolution_is_current(
                            &self.baseline_dependencies,
                            &resolved.dependencies,
                            self.active.dependency_cut,
                        ) {
                            ClaimSettlementObservation::QueuedVerify
                        } else {
                            ClaimSettlementObservation::QueuedResolve
                        }
                    }
                    ClaimPayloadPolicyEvolution::Invalid => ClaimSettlementObservation::Fault(
                        ClaimSettlementFault::MembershipProjection,
                    ),
                }
            }
            ClaimSettlementNext::Retry => ClaimSettlementObservation::QueuedResolve,
        }
    }
}
