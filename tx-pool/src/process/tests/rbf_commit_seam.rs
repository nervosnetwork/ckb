use super::*;

impl TxPoolService {
    pub(crate) fn try_submit_entry(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        _status: Status,
        _entry_id: ProposalShortId,
    ) -> SubmitEntryOutcome {
        self.pipeline
            .kernel
            .mutate(|kernel| {
                self.try_submit_entry_with_handoff(
                    tx_pool,
                    snapshot,
                    pre_resolve_tip,
                    entry.clone(),
                    |tx_pool, plan| self.settle_kernel_for_pool_plan(kernel, tx_pool, &entry, plan),
                )
            })
            .0
    }
}
