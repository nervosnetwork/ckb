use super::*;

impl PipelineRuntime {
    pub(crate) fn admit_transaction(
        &self,
        tx: TransactionView,
        source: TxSource,
        epoch: u64,
        stage: RawStage,
    ) -> Result<(bool, Vec<TerminalRecord<PipelineRawTx>>), CoordinatorError> {
        self.admit_transaction_journaled(tx, source, epoch, stage, |_| {})
    }
}
