use super::super::state::RawTxHash;
use super::*;
use ckb_types::packed::ProposalShortId;

impl PackedTemplateTransaction {
    pub(in crate::authority) fn hash(&self) -> RawTxHash {
        RawTxHash(self.resolved.transaction.hash())
    }

    pub(in crate::authority) fn proposal_short_id(&self) -> ProposalShortId {
        self.resolved.transaction.proposal_short_id()
    }

    pub(in crate::authority) fn accepted_at(&self) -> AcceptedAtMillis {
        self.accepted_at
    }

    pub(in crate::authority) fn metrics(&self) -> &CandidateMetrics {
        &self.metrics
    }

    pub(in crate::authority) fn resolved(&self) -> &Arc<ResolvedTransaction> {
        &self.resolved
    }
}

impl PackedTemplateTransactions {
    pub(in crate::authority) fn entries(&self) -> &[PackedTemplateTransaction] {
        &self.entries
    }

    pub(in crate::authority) fn serialized_bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.metrics.cost.serialized_bytes)
            .sum()
    }

    pub(in crate::authority) fn cycles(&self) -> Cycle {
        self.entries
            .iter()
            .map(|entry| entry.metrics.cost.cycles)
            .sum()
    }
}

impl TemplateSelectionReceipt {
    pub(in crate::authority) fn pack_transactions_for_foundation(
        &self,
        limits: TemplatePackingLimits,
        max_consecutive_failures: usize,
    ) -> Result<PackedTemplateTransactions, TemplateReadError> {
        self.pack_transactions_with_failure_bound(limits, max_consecutive_failures)
    }
}
