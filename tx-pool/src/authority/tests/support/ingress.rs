use super::*;

impl RemoteCycleLimit {
    pub(in crate::authority) const fn for_foundation(declared: Cycle) -> Self {
        Self(declared)
    }
}

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

impl RetainedIngressAttempt {
    #[expect(
        clippy::result_large_err,
        reason = "the test projection preserves the exact production attempt without a fixture-only boxed representation"
    )]
    pub(in crate::authority) fn into_validated_for_foundation(
        self,
    ) -> Result<RetainedIngress, Self> {
        match self {
            Self::Validated(ingress) => Ok(ingress),
            other => Err(other),
        }
    }
}

impl RetainedIngressRejection {
    pub(in crate::authority) fn reason_for_foundation(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

#[expect(
    clippy::result_large_err,
    reason = "the test boundary preserves the exact production attempt without a fixture-only boxed representation"
)]
pub(in crate::authority) fn remote_at_for_foundation(
    tx: TransactionView,
    declared_cycles: Cycle,
    peer: PeerIndex,
    admitted_at_secs: u64,
    consensus: &Consensus,
) -> Result<RetainedIngress, RetainedIngressAttempt> {
    let tx = BoundedTransaction::try_new(tx).expect("foundation transaction is bounded");
    remote_at(tx, declared_cycles, peer, admitted_at_secs, consensus)
        .into_validated_for_foundation()
}

pub(in crate::authority) fn proposal_for_foundation(tx: TransactionView) -> RetainedIngress {
    RetainedIngress {
        kind: RetainedIngressKind::Proposal,
        admission: ValidatedAdmission::proposal(tx)
            .expect("the foundation Proposal fixture has valid ingress evidence"),
    }
}
