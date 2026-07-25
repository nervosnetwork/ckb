use super::*;

impl TxPoolService {
    pub(crate) fn try_submit_entry(
        &self,
        tx_pool: &mut TxPool,
        snapshot: Arc<Snapshot>,
        pre_resolve_tip: Byte32,
        entry: TxEntry,
        status: Status,
        entry_id: ProposalShortId,
    ) -> SubmitEntryOutcome {
        let mut coordinated = self.try_submit_entry_inner(
            tx_pool,
            snapshot,
            pre_resolve_tip,
            entry.clone(),
            status,
            entry_id,
        );
        if coordinated.outcome.result.is_ok() {
            self.finalize_coordinated_submit(tx_pool, &entry, &mut coordinated);
        }
        coordinated.outcome
    }
}
