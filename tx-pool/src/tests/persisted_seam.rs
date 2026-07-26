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

#[tokio::test]
async fn persistence_writer_admits_only_one_snapshot_owner() {
    let writer = Arc::new(PersistenceWriter::default());
    let first = writer.acquire().await;
    let waiting_writer = Arc::clone(&writer);
    let second = tokio::spawn(async move { waiting_writer.acquire().await });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());

    drop(first);
    tokio::time::timeout(std::time::Duration::from_secs(1), second)
        .await
        .expect("released persistence ownership wakes one waiter")
        .expect("persistence waiter does not panic");
}
