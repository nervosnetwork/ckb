use super::*;

impl RetainedIngress {
    pub(in crate::authority) fn admission_for_foundation(&self) -> &ValidatedAdmission {
        &self.admission
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
