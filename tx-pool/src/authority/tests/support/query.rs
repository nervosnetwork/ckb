use super::*;

impl ParentFirstPersistenceReceipt {
    pub(in crate::authority) fn accepted(&self) -> &[Arc<TransactionView>] {
        self.0.accepted()
    }

    pub(in crate::authority) fn recovery(&self) -> &[Arc<TransactionView>] {
        self.0.recovery()
    }
}
