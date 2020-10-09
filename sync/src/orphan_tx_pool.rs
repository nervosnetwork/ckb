use ckb_logger::trace;
use ckb_network::PeerIndex;
use ckb_types::{
    core::TransactionView,
    packed::{self, OutPoint},
};
use ckb_util::shrink_to_fit;
use std::collections::HashMap;
use tokio::sync::RwLock;

const SHRINK_THRESHOLD: usize = 100;
pub(crate) const ORPHAN_TX_EXPIRE_TIME: u64 = 2 * 48; // double block interval
pub(crate) const DEFAULT_MAX_ORPHAN_TRANSACTIONS: usize = 100;

#[derive(Debug, Clone)]
pub struct Entry {
    /// Transaction
    pub tx: TransactionView,
    // peer id
    pub peer: PeerIndex,
    // Expire timestamp
    pub expires_at: u64,
}

impl Entry {
    pub fn new(tx: TransactionView, peer: PeerIndex) -> Entry {
        Entry {
            tx,
            peer,
            expires_at: faketime::unix_time().as_secs() + ORPHAN_TX_EXPIRE_TIME,
        }
    }
}

#[derive(Debug)]
pub struct OrphanTxPool {
    pub(crate) inner: RwLock<Inner>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Inner {
    pub(crate) entries: HashMap<packed::Byte32, Entry>,
    pub(crate) by_out_point: HashMap<OutPoint, packed::Byte32>,
}

impl Inner {
    pub fn remove_orphan_tx(&mut self, hash: &packed::Byte32) -> Option<Entry> {
        self.entries.remove(hash).map(|entry| {
            for out_point in entry.tx.input_pts_iter() {
                self.by_out_point.remove(&out_point);
            }
            entry
        })
    }

    pub fn limit_size(&mut self) -> u64 {
        let mut evicted = 0u64;
        let now = faketime::unix_time().as_secs();
        let expires: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(hash, entry)| {
                if entry.expires_at <= now {
                    Some(hash)
                } else {
                    None
                }
            })
            .cloned()
            .collect();
        for hash in expires {
            evicted += 1;
            self.remove_orphan_tx(&hash);
        }

        while self.len() > DEFAULT_MAX_ORPHAN_TRANSACTIONS {
            evicted += 1;
            // Evict a random orphan:
            let hash = self.entries.keys().cloned().next().expect("bound checked");
            self.remove_orphan_tx(&hash);
        }

        if evicted > 0 {
            trace!("OrphanTxPool overflow, removed {} tx", evicted);
            self.shrink_to_fit();
        }
        evicted
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.entries, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_out_point, SHRINK_THRESHOLD);
    }
}

impl Default for OrphanTxPool {
    fn default() -> Self {
        OrphanTxPool::new()
    }
}

impl OrphanTxPool {
    pub fn new() -> Self {
        OrphanTxPool {
            inner: RwLock::new(Inner::default()),
        }
    }

    pub async fn add_orphan_tx(&self, tx: TransactionView, peer: PeerIndex) {
        let mut guard = self.inner.write().await;

        if guard.entries.contains_key(&tx.hash()) {
            return;
        }

        guard
            .entries
            .insert(tx.hash(), Entry::new(tx.clone(), peer));

        for out_point in tx.input_pts_iter() {
            guard.by_out_point.insert(out_point, tx.hash());
        }
        guard.limit_size();
    }

    pub async fn get(&self, hash: &packed::Byte32) -> Option<Entry> {
        let guard = self.inner.read().await;
        guard.entries.get(hash).cloned()
    }

    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    pub async fn find_by_previous(&self, tx: &TransactionView) -> Option<packed::Byte32> {
        let guard = self.inner.read().await;

        tx.output_pts()
            .iter()
            .find_map(|out_point| guard.by_out_point.get(out_point).cloned())
    }

    pub async fn remove_orphan_txs(&self, hashes: &[packed::Byte32]) {
        let mut guard = self.inner.write().await;
        for hash in hashes {
            guard.remove_orphan_tx(&hash);
        }
        guard.shrink_to_fit();
    }

    pub async fn remove_orphan_tx(&self, hash: &packed::Byte32) {
        let mut guard = self.inner.write().await;
        guard.remove_orphan_tx(hash);
        guard.shrink_to_fit();
    }
}
