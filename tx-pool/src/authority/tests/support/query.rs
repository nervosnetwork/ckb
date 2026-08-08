use super::*;

impl ParentFirstPersistenceReceipt {
    pub(in crate::authority) fn accepted(&self) -> &[Arc<TransactionView>] {
        self.0.accepted()
    }

    pub(in crate::authority) fn recovery(&self) -> &[Arc<TransactionView>] {
        self.0.recovery()
    }
}

impl FullQueryPermit {
    pub(in crate::authority) fn prepared_rows_for_foundation(&self) -> usize {
        self.state.rows.capacity()
    }
}
