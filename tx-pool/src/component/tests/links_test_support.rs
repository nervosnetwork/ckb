use super::*;

impl TxLinksMap {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxLinks)> {
        self.inner.iter()
    }

    pub(crate) fn get_mut(&mut self, id: &ProposalShortId) -> Option<&mut TxLinks> {
        self.inner.get_mut(id)
    }

    pub(crate) fn add_parent(
        &mut self,
        id: &ProposalShortId,
        parent: ProposalShortId,
    ) -> Option<bool> {
        self.inner
            .get_mut(id)
            .map(|links| links.parents.insert(parent))
    }
}
