use super::*;

impl FinalAdmissionValidation {
    pub(in crate::authority) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        let current = work.clone();
        Self::prepare(snapshot, work)?.complete_inner(authority, current)
    }
}

impl DirectAdmissionValidation {
    pub(in crate::authority) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: DirectAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        Self::prepare(snapshot, work)?.complete_inner(authority)
    }
}
