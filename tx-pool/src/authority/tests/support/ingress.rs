use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum RetainedIngressCommit {
    Retained,
    AcceptedDuplicate,
    RemoteReleased,
    ProposalUnchanged,
    ProposalPayloadVariant,
    Rejected,
}

/// Test-reference proof that a retained no-owner rejection and its public
/// effect committed in one Apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) struct IngressRejectionCommit;

impl RetainedIngress {
    pub(in crate::authority) fn into_parts(self) -> (RetainedIngressKind, ValidatedAdmission) {
        (self.kind, self.admission)
    }

    pub(in crate::authority) fn admission_for_foundation(&self) -> &ValidatedAdmission {
        &self.admission
    }
}

impl RetainedIngressBoundaryError {
    pub(in crate::authority) fn from_admission_for_foundation(
        error: AdmissionValidationError,
    ) -> Self {
        match error {
            AdmissionValidationError::ResourceAllocation => Self::ResourceUnavailable,
            AdmissionValidationError::EmptyTransaction
            | AdmissionValidationError::ResourceArithmetic => Self::InvalidEvidence,
        }
    }
}

impl RetainedIngressRejection {
    pub(in crate::authority) fn reason_for_foundation(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

pub(in crate::authority) fn remote_at_for_foundation(
    tx: TransactionView,
    declared_cycles: Cycle,
    peer: PeerIndex,
    admitted_at_secs: u64,
    consensus: &Consensus,
) -> Result<RetainedIngress, RetainedIngressError> {
    remote_at(tx, declared_cycles, peer, admitted_at_secs, consensus)
}
