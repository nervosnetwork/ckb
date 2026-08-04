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

impl<'authority> AuthorityReadView<'authority> {
    pub(in crate::authority) fn entries(
        &self,
    ) -> impl Iterator<Item = AuthorityReadEntry<'authority>> + '_ {
        self.entries.values().map(AuthorityReadEntry::new)
    }
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
