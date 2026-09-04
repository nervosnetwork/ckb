use super::*;

impl ParentFirstPersistenceReceipt {
    pub(in crate::authority) fn accepted(&self) -> &[Arc<TransactionView>] {
        self.0.accepted()
    }
}

impl FullQueryPermit {
    pub(in crate::authority) fn prepared_rows_for_foundation(&self) -> usize {
        self.state.rows.capacity()
    }
}
