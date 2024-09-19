use ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL;
use ckb_logger::{debug, trace};
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use ckb_types::{
    core::{Cycle, TransactionView},
    packed::{OutPoint, ProposalShortId},
};
use ckb_util::shrink_to_fit;
use std::collections::{HashMap, HashSet};

const SHRINK_THRESHOLD: usize = 100;

/// 100 max block interval
pub(crate) const ORPHAN_TX_EXPIRE_TIME: u64 = 100 * MAX_BLOCK_INTERVAL;
pub(crate) const DEFAULT_MAX_ORPHAN_TRANSACTIONS: usize = 100;

#[derive(Debug, Clone)]
pub struct Entry {
    /// Transaction
    pub tx: TransactionView,
    /// peer id
    pub peer: PeerIndex,
    /// Declared cycles
    pub cycle: Cycle,
    /// Expire timestamp
    pub expires_at: u64,
}

impl Entry {
    pub fn new(tx: TransactionView, peer: PeerIndex, cycle: Cycle) -> Entry {
        Entry {
            tx,
            peer,
            cycle,
            expires_at: ckb_systemtime::unix_time().as_secs() + ORPHAN_TX_EXPIRE_TIME,
        }
    }
}

#[derive(Default, Debug, Clone)]
pub(crate) struct OrphanPool {
    pub(crate) entries: HashMap<ProposalShortId, Entry>,
    pub(crate) by_out_point: HashMap<OutPoint, HashSet<ProposalShortId>>,
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

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    fn shrink_to_fit(&mut self) {
        shrink_to_fit!(self.entries, SHRINK_THRESHOLD);
        shrink_to_fit!(self.by_out_point, SHRINK_THRESHOLD);
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn remove_orphan_tx(&mut self, id: &ProposalShortId) -> Option<Entry> {
        self.entries.remove(id).inspect(|entry| {
            debug!("remove orphan tx {}", entry.tx.hash());
            for out_point in entry.tx.input_pts_iter() {
                if let Some(ids_set) = self.by_out_point.get_mut(&out_point) {
                    ids_set.remove(id);

                    if ids_set.is_empty() {
                        self.by_out_point.remove(&out_point);
                    }
                }
            }
        })
    }

    pub fn remove_orphan_txs(&mut self, ids: impl Iterator<Item = ProposalShortId>) {
        for id in ids {
            self.remove_orphan_tx(&id);
        }
        self.shrink_to_fit();
    }

    fn limit_size(&mut self) -> Vec<Byte32> {
        let now = ckb_systemtime::unix_time().as_secs();
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

        let mut evicted_txs = vec![];

        for id in expires {
            if let Some(entry) = self.remove_orphan_tx(&id) {
                evicted_txs.push(entry.tx.hash());
            }
        }

        while self.len() > DEFAULT_MAX_ORPHAN_TRANSACTIONS {
            // Evict a random orphan:
            let id = self.entries.keys().next().cloned().expect("bound checked");
            if let Some(entry) = self.remove_orphan_tx(&id) {
                evicted_txs.push(entry.tx.hash());
            }
        }

        if !evicted_txs.is_empty() {
            trace!("OrphanTxPool full, evicted {} tx", evicted_txs.len());
            self.shrink_to_fit();
        }
        evicted_txs
    }

    pub fn add_orphan_tx(
        &mut self,
        tx: TransactionView,
        peer: PeerIndex,
        declared_cycle: Cycle,
    ) -> Vec<Byte32> {
        if self.entries.contains_key(&tx.proposal_short_id()) {
            return vec![];
        }

        debug!("add_orphan_tx {}", tx.hash());
        self.entries.insert(
            tx.proposal_short_id(),
            Entry::new(tx.clone(), peer, declared_cycle),
        );

        for out_point in tx.input_pts_iter() {
            self.by_out_point
                .entry(out_point)
                .or_default()
                .insert(tx.proposal_short_id());
        }

        // DoS prevention: do not allow OrphanPool to grow unbounded
        self.limit_size()
    }

    pub fn find_by_previous(&self, tx: &TransactionView) -> Vec<&ProposalShortId> {
        tx.output_pts()
            .iter()
            .filter_map(|out_point| self.by_out_point.get(out_point))
            .flatten()
            .collect::<Vec<_>>()
    }
}
