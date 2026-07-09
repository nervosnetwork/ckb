use ckb_chain_spec::consensus::MAX_BLOCK_INTERVAL;
use ckb_logger::{debug, trace};
use ckb_network::PeerIndex;
use ckb_types::packed::Byte32;
use ckb_types::{
    core::TransactionView,
    packed::{OutPoint, ProposalShortId},
};
use ckb_util::shrink_to_fit;
use std::collections::{HashMap, HashSet};

use crate::constants::SHRINK_THRESHOLD;
use crate::tx_source::TxSource;

/// Expiration time for orphan transactions, expressed as a multiple of the
/// maximum block interval.
///
/// Orphans are transactions whose inputs are not yet available. They are kept
/// for a long window so that out-of-order block/transaction propagation does
/// not cause them to be dropped prematurely. 100 block intervals provides
/// roughly one day of buffer time on main-net parameters.
pub(crate) const ORPHAN_TX_EXPIRE_TIME: u64 = 100 * MAX_BLOCK_INTERVAL;

/// Default maximum number of transactions stored in the orphan pool.
///
/// Limits memory consumption for transactions that cannot be resolved yet.
/// 100 is a conservative default that tolerates moderate network disorder
/// without allowing the orphan pool to grow unbounded.
pub(crate) const DEFAULT_MAX_ORPHAN_TRANSACTIONS: usize = 100;

#[derive(Debug, Clone)]
pub struct Entry {
    /// Transaction
    pub tx: TransactionView,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// Expire timestamp
    pub expires_at: u64,
}

impl Entry {
    pub fn new(tx: TransactionView, source: TxSource) -> Entry {
        Entry {
            tx,
            source,
            expires_at: ckb_systemtime::unix_time().as_secs() + ORPHAN_TX_EXPIRE_TIME,
        }
    }

    /// Returns the peer index if this orphan came from a remote source.
    pub fn peer(&self) -> Option<PeerIndex> {
        self.source.peer()
    }
}

#[derive(Default, Debug, Clone)]
pub(crate) struct OrphanPool {
    entries: HashMap<ProposalShortId, Entry>,
    by_out_point: HashMap<OutPoint, HashSet<ProposalShortId>>,
}

impl OrphanPool {
    pub fn new() -> Self {
        OrphanPool::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
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
            for out_point in entry
                .tx
                .input_pts_iter()
                .chain(entry.tx.cell_deps_iter().map(|c| c.out_point()))
            {
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

    /// Remove all orphan transactions submitted by the given peer.
    ///
    /// Returns the short ids of the removed transactions so callers can clean up
    /// any related in-flight state (e.g. RBF candidates).
    pub fn remove_by_peer(&mut self, peer: PeerIndex) -> Vec<ProposalShortId> {
        let ids: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.peer() == Some(peer))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            self.remove_orphan_tx(id);
        }
        ids
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.by_out_point.clear();
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

    /// Add a transaction to the orphan pool.
    ///
    /// Returns `(true, evicted_txs)` if the transaction was newly inserted and
    /// is still present after eviction, or `(false, evicted_txs)` if it was
    /// already present or was evicted by the size limit.
    pub fn add_orphan_tx(&mut self, tx: TransactionView, source: TxSource) -> (bool, Vec<Byte32>) {
        let id = tx.proposal_short_id();
        if self.entries.contains_key(&id) {
            return (false, vec![]);
        }

        debug!("add_orphan_tx {}", tx.hash());
        self.entries
            .insert(id.clone(), Entry::new(tx.clone(), source));

        for out_point in tx
            .input_pts_iter()
            .chain(tx.cell_deps_iter().map(|c| c.out_point()))
        {
            self.by_out_point
                .entry(out_point)
                .or_default()
                .insert(id.clone());
        }

        // DoS prevention: do not allow OrphanPool to grow unbounded
        let evicted_txs = self.limit_size();
        let retained = self.entries.contains_key(&id);
        (retained, evicted_txs)
    }

    pub fn find_by_previous(&self, tx: &TransactionView) -> Vec<&ProposalShortId> {
        tx.output_pts()
            .iter()
            .filter_map(|out_point| self.by_out_point.get(out_point))
            .flatten()
            .collect::<Vec<_>>()
    }
}
