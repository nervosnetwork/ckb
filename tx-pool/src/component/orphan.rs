use ckb_logger::trace;
use ckb_network::PeerIndex;
use ckb_types::{
    core::TransactionView,
    packed::{OutPoint, ProposalShortId},
};
use ckb_util::shrink_to_fit;
use std::collections::HashMap;

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

#[derive(Default, Debug, Clone)]
pub(crate) struct OrphanPool {
    pub(crate) entries: HashMap<ProposalShortId, Entry>,
    pub(crate) by_out_point: HashMap<OutPoint, ProposalShortId>,
}

impl OrphanPool {
    pub fn new() -> Self {
        OrphanPool::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(dead_code)]
    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.entries, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_out_point, SHRINK_THRESHOLD);
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn remove_orphan_tx(&mut self, id: &ProposalShortId) -> Option<Entry> {
        self.entries.remove(id).map(|entry| {
            for out_point in entry.tx.input_pts_iter() {
                self.by_out_point.remove(&out_point);
            }
            entry
        })
    }

    pub fn remove_orphan_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.remove_orphan_tx(&id);
        }
        self.shrink_to_fit();
    }

    pub fn limit_size(&mut self) -> usize {
        let now = faketime::unix_time().as_secs();
        let expires: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(id, entry)| {
                if entry.expires_at <= now {
                    Some(id)
                } else {
                    None
                }
            })
            .cloned()
            .collect();

        let mut evicted = expires.len();

        for id in expires {
            self.remove_orphan_tx(&id);
        }

        while self.len() > DEFAULT_MAX_ORPHAN_TRANSACTIONS {
            evicted += 1;
            // Evict a random orphan:
            let id = self.entries.keys().next().cloned().expect("bound checked");
            self.remove_orphan_tx(&id);
        }

        if evicted > 0 {
            trace!("OrphanTxPool full, evicted {} tx", evicted);
            self.shrink_to_fit();
        }
        evicted
    }

    pub fn add_orphan_tx(&mut self, tx: TransactionView, peer: PeerIndex) {
        if self.entries.contains_key(&tx.proposal_short_id()) {
            return;
        }

        self.entries
            .insert(tx.proposal_short_id(), Entry::new(tx.clone(), peer));

        for out_point in tx.input_pts_iter() {
            self.by_out_point.insert(out_point, tx.proposal_short_id());
        }
        self.limit_size();
    }

    pub fn find_by_previous(&self, tx: &TransactionView) -> Option<ProposalShortId> {
        tx.output_pts()
            .iter()
            .find_map(|out_point| self.by_out_point.get(out_point).cloned())
    }
}
