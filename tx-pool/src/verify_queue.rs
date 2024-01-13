extern crate rustc_hash;
extern crate slab;
use ckb_network::PeerIndex;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed::ProposalShortId,
};
use ckb_util::shrink_to_fit;
use multi_index_map::MultiIndexMap;
use tokio::sync::watch;

const DEFAULT_MAX_VERIFY_TRANSACTIONS: usize = 100;
const SHRINK_THRESHOLD: usize = 120;

#[derive(Debug, Clone, Eq)]
pub struct Entry {
    pub(crate) tx: TransactionView,
    pub(crate) remote: Option<(Cycle, PeerIndex)>,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Entry) -> bool {
        self.tx == other.tx
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VerifyStatus {
    Fresh,
    Verifying,
    Completed,
}

#[derive(MultiIndexMap, Clone)]
pub struct VerifyEntry {
    #[multi_index(hashed_unique)]
    pub id: ProposalShortId,
    #[multi_index(hashed_non_unique)]
    pub status: VerifyStatus,
    #[multi_index(ordered_non_unique)]
    pub added_time: u64,
    // other sort key
    pub inner: Entry,
}

pub struct VerifyQueue {
    inner: MultiIndexVerifyEntryMap,
    queue_tx: watch::Sender<usize>,
}

impl VerifyQueue {
    pub(crate) fn new(queue_tx: watch::Sender<usize>) -> Self {
        VerifyQueue {
            inner: MultiIndexVerifyEntryMap::default(),
            queue_tx,
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
        self.len() > DEFAULT_MAX_VERIFY_TRANSACTIONS
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.get_by_id(id).is_some()
    }

    pub fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.inner, SHRINK_THRESHOLD);
    }

    pub fn remove_tx(&mut self, id: &ProposalShortId) -> Option<Entry> {
        self.inner.remove_by_id(id).map(|e| {
            self.shrink_to_fit();
            e.inner
        })
    }

    pub fn remove_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.inner.remove_by_id(&id);
        }
        self.shrink_to_fit();
    }

    pub fn pop_first(&mut self) -> Option<Entry> {
        if let Some(entry) = self.get_first() {
            self.remove_tx(&entry.tx.proposal_short_id());
            Some(entry)
        } else {
            None
        }
    }

    pub fn get_first(&self) -> Option<Entry> {
        self.inner
            .iter_by_added_time()
            .filter(|e| e.status == VerifyStatus::Fresh)
            .next()
            .map(|entry| entry.inner.clone())
    }

    /// If the queue did not have this tx present, true is returned.
    /// If the queue did have this tx present, false is returned.
    pub fn add_tx(&mut self, tx: TransactionView, remote: Option<(Cycle, PeerIndex)>) -> bool {
        if self.contains_key(&tx.proposal_short_id()) {
            return false;
        }
        self.inner.insert(VerifyEntry {
            id: tx.proposal_short_id(),
            status: VerifyStatus::Fresh,
            added_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("timestamp")
                .as_millis() as u64,
            inner: Entry { tx, remote },
        });
        eprintln!("added to queue len: {:?}", self.len());
        self.queue_tx.send(self.len()).unwrap();
        true
    }

    /// Clears the map, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
        self.shrink_to_fit()
    }
}
