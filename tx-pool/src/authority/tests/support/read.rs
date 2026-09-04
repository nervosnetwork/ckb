use super::super::state::{EntryVersion, TxIdentity};
use super::*;

impl AuthorityReadEntry<'_> {
    pub(in crate::authority) fn identity(&self) -> &TxIdentity {
        &self.owner.record().identity
    }

    pub(in crate::authority) fn version(&self) -> EntryVersion {
        self.owner.record().version
    }

    pub(in crate::authority) fn arrival(&self) -> Arrival {
        self.owner.record().arrival
    }
}

impl AuthorityReadView<'_> {
    pub(in crate::authority) fn pool_ids(&self) -> Result<AuthorityPoolIds, AuthorityReadError> {
        let cut = self.full_read_cut()?;
        let (pending_capacity, proposed_capacity) = cut.accepted_status_counts()?;
        let mut pending = Vec::new();
        let mut proposed = Vec::new();
        pending
            .try_reserve(pending_capacity)
            .map_err(|_| AuthorityReadError::Allocation)?;
        proposed
            .try_reserve(proposed_capacity)
            .map_err(|_| AuthorityReadError::Allocation)?;
        for order in cut.accepted_order() {
            let accepted = cut.accepted_entry_for_order(&order)?;
            match accepted.entry().status() {
                AcceptedStatus::Pending | AcceptedStatus::Gap => pending.push(order.hash().clone()),
                AcceptedStatus::Proposed => proposed.push(order.hash().clone()),
            }
        }
        if pending.len() != pending_capacity || proposed.len() != proposed_capacity {
            return Err(AuthorityReadError::Projection);
        }
        pending.sort_unstable();
        proposed.sort_unstable();
        Ok(AuthorityPoolIds { pending, proposed })
    }

    pub(in crate::authority) fn replacement_history_hashes(
        &self,
    ) -> Result<Vec<RawTxHash>, AuthorityReadError> {
        let mut history = Vec::new();
        history
            .try_reserve(self.owner_count())
            .map_err(|_| AuthorityReadError::Allocation)?;
        history.extend(self.replacement_history());
        history.sort_unstable();
        Ok(history)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::authority) struct AuthorityPoolIds {
    pub(in crate::authority) pending: Vec<RawTxHash>,
    pub(in crate::authority) proposed: Vec<RawTxHash>,
}

impl PersistenceReadReceipt {
    pub(in crate::authority) fn selected_len(&self) -> usize {
        self.selected.len()
    }
}

impl ParentFirstPersistence {
    pub(in crate::authority) fn accepted(&self) -> &[Arc<TransactionView>] {
        &self.accepted
    }

    pub(in crate::authority) fn recovery(&self) -> &[Arc<TransactionView>] {
        &self.recovery
    }
}
