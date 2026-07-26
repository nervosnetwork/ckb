use super::*;

impl TxPool {
    pub(crate) fn load_from_file(&self) -> Result<Vec<TransactionView>, AnyError> {
        self.load_persistence_snapshot()
            .map(PersistenceSnapshot::into_transactions)
    }

    pub(crate) fn save_into_file(&mut self) -> Result<(), AnyError> {
        let snapshot = PersistenceSnapshot {
            accepted: self.get_all_txs(),
            recovery: Vec::new(),
        };
        write_snapshot(&self.config.persisted_data, snapshot)?;
        let chain = self.cloned_snapshot();
        drop(self.reset_generation(chain));
        Ok(())
    }
}
