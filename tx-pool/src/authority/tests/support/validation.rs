use super::*;

impl FinalAdmissionValidation {
    pub(in crate::authority) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        Self::capture_with_min_fee_rate_for_foundation(
            authority,
            snapshot,
            work,
            ckb_types::core::FeeRate::zero(),
        )
    }

    pub(in crate::authority) fn capture_with_min_fee_rate_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: FinalAdmissionWork,
        min_fee_rate: ckb_types::core::FeeRate,
    ) -> Result<Self, FinalAdmissionValidationError> {
        let current = work.clone();
        Self::prepare(snapshot, work, min_fee_rate)?.complete_inner(authority, current)
    }
}

impl DirectAdmissionValidation {
    pub(in crate::authority) fn capture_for_foundation(
        authority: &TxPoolAuthority,
        snapshot: Arc<Snapshot>,
        work: DirectAdmissionWork,
    ) -> Result<Self, FinalAdmissionValidationError> {
        Self::prepare(snapshot, work, ckb_types::core::FeeRate::zero())?.complete_inner(authority)
    }
}
