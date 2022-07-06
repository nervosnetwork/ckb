use ckb_network::PeerIndex;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed::ProposalShortId,
};
use ckb_util::{shrink_to_fit, LinkedHashMap};

const SHRINK_THRESHOLD: usize = 100;
pub(crate) const DEFAULT_MAX_CHUNK_TRANSACTIONS: usize = 100;

#[derive(Debug, Clone, Eq)]
pub(crate) struct Entry {
    pub(crate) tx: TransactionView,
    pub(crate) remote: Option<(Cycle, PeerIndex)>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Entry) -> bool {
        self.tx == other.tx
    }
}

#[derive(Default)]
pub(crate) struct ChunkQueue {
    inner: LinkedHashMap<ProposalShortId, Entry>,
    // memory last pop value for atomic reset
    front: Option<Entry>,
}

impl ChunkQueue {
    pub(crate) fn new() -> Self {
        ChunkQueue {
            inner: LinkedHashMap::default(),
            front: None,
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.len() > DEFAULT_MAX_CHUNK_TRANSACTIONS
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.front
            .as_ref()
            .map(|e| e.tx.proposal_short_id())
            .as_ref()
            == Some(id)
            || self.inner.contains_key(id)
    }

    pub fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }

    pub fn clean_front(&mut self) {
        self.front = None;
    }

    pub fn pop_front(&mut self) -> Option<Entry> {
        if let Some(entry) = &self.front {
            Some(entry.clone())
        } else {
            match self.inner.pop_front() {
                Some((_id, entry)) => {
                    self.front = Some(entry.clone());
                    Some(entry)
                }
                None => None,
            }
        }
    }

    pub fn remove_chunk_tx(&mut self, id: &ProposalShortId) -> Option<Entry> {
        let ret = self.inner.remove(id);
        self.shrink_to_fit();
        ret
    }

    pub fn remove_chunk_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.inner.remove(&id);
        }
        self.shrink_to_fit();
    }

    /// If the queue did not have this tx present, true is returned.
    ///
    /// If the queue did have this tx present, false is returned.
    pub fn add_tx(&mut self, tx: TransactionView, remote: Option<(Cycle, PeerIndex)>) -> bool {
        if self.contains_key(&tx.proposal_short_id()) {
            return false;
        }

        self.inner
            .insert(tx.proposal_short_id(), Entry { tx, remote })
            .is_none()
    }
}
