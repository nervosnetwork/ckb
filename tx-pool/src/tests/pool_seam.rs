use super::*;

impl TxPool {
    pub(crate) fn remove_conflict(&mut self, hash: &Byte32) -> bool {
        let removed = self.conflict_cache.remove(hash).is_some();
        debug!(
            "remove_conflict {:?} now room size: {}",
            hash,
            self.conflict_cache.len()
        );
        removed
    }

    /// Recover conflict-cached transactions whose inputs are all currently
    /// available. A candidate that still conflicts with the in-pool state
    /// must not come back: it would be rejected again and, with both
    /// conflicting txs cached, can trigger an infinite recover/reject loop
    /// (RBF cycling).
    pub(crate) fn get_conflicted_txs_from_inputs(
        &self,
        inputs: impl Iterator<Item = OutPoint>,
    ) -> Vec<(TransactionView, TxSource)> {
        let pool_map = &self.pool_map;
        let snapshot = self.snapshot();
        self.conflict_cache
            .recoverable_by_inputs(inputs, |tx, recovery_outpoints| {
                conflict_recovery_ready(pool_map, snapshot, tx, recovery_outpoints)
            })
    }

    pub(crate) fn schedule_conflict_candidates(
        &mut self,
        hashes: impl Iterator<Item = Byte32>,
    ) -> usize {
        let pool_map = &self.pool_map;
        let snapshot = &self.snapshot;
        self.conflict_cache
            .schedule_hashes(hashes, |tx, recovery_outpoints| {
                conflict_recovery_ready(pool_map, snapshot, tx, recovery_outpoints)
            })
    }

    pub(crate) fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        self.pool_map.get_proposals(limit, exclusion)
    }
}
