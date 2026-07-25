use super::*;

impl TxLinksMap {
    pub fn contains_key(&self, short_id: &ProposalShortId) -> bool {
        self.inner.contains_key(short_id)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxLinks)> {
        self.inner.iter()
    }
}
