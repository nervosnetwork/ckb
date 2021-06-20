use ckb_network::PeerIndex;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed::ProposalShortId,
};
use ckb_util::{shrink_to_fit, LinkedHashMap};

const SHRINK_THRESHOLD: usize = 100;
pub(crate) const DEFAULT_MAX_CHUNK_TRANSACTIONS: usize = 100;

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub(crate) tx: TransactionView,
    pub(crate) remote: Option<(Cycle, PeerIndex)>,
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

    #[allow(dead_code)]
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
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
        self.inner.remove(id)
    }

    pub fn remove_chunk_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.remove_chunk_tx(&id);
        }
        self.shrink_to_fit();
    }

    pub fn add_remote_tx(&mut self, tx: TransactionView, remote: (Cycle, PeerIndex)) {
        if self.len() > DEFAULT_MAX_CHUNK_TRANSACTIONS {
            return;
        }

        if self.inner.contains_key(&tx.proposal_short_id()) {
            return;
        }

        self.inner.insert(
            tx.proposal_short_id(),
            Entry {
                tx,
                remote: Some(remote),
            },
        );
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.clean_front();
        self.shrink_to_fit()
    }
}
