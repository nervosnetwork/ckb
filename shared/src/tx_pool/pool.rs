//! Top-level Pool type, methods, and tests
use super::types::{PoolEntry, TxPoolConfig};
use crate::tx_pool::orphan::OrphanPool;
use crate::tx_pool::pending::PendingQueue;
use crate::tx_pool::proposed::ProposedPool;
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use faketime::unix_time_as_millis;
use log::trace;
use lru_cache::LruCache;

#[derive(Debug, Clone)]
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    /// The short id that has not been proposed
    pub(crate) pending: PendingQueue,
    /// Tx pool that finely for commit
    pub(crate) proposed: ProposedPool,
    /// Orphans in the pool
    pub(crate) orphan: OrphanPool,
    /// cache for conflict transaction
    pub(crate) conflict: LruCache<ProposalShortId, PoolEntry>,
    /// last txs updated timestamp
    pub(crate) last_txs_updated_at: u64,
}

impl TxPool {
    pub fn new(config: TxPoolConfig) -> TxPool {
        let cache_size = config.max_cache_size;
        let last_txs_updated_at = 0u64;

        TxPool {
            config,
            pending: PendingQueue::new(),
            proposed: ProposedPool::new(),
            orphan: OrphanPool::new(),
            conflict: LruCache::new(cache_size),
            last_txs_updated_at,
        }
    }

    pub fn pending_size(&self) -> u32 {
        self.pending.size() as u32
    }
    pub fn proposed_size(&self) -> u32 {
        self.proposed.vertices.len() as u32
    }
    pub fn orphan_size(&self) -> u32 {
        self.orphan.vertices.len() as u32
    }

    // enqueue_tx inserts a new transaction into the non-verifiable transaction queue.
    pub fn enqueue_tx(&mut self, cycles: Option<Cycle>, tx: Transaction) -> bool {
        self.pending.add_tx(cycles, tx).is_none()
    }

    pub(crate) fn add_orphan(
        &mut self,
        cycles: Option<Cycle>,
        tx: Transaction,
        unknowns: Vec<OutPoint>,
    ) {
        trace!(target: "tx_pool", "add_orphan {:#x}", &tx.hash());
        self.orphan.add_tx(cycles, tx, unknowns.into_iter());
    }

    pub(crate) fn add_proposed(&mut self, cycles: Cycle, tx: Transaction) {
        trace!(target: "tx_pool", "add_proposed {:#x}", tx.hash());
        self.touch_last_txs_updated_at();
        self.proposed.add_tx(cycles, tx);
    }

    pub(crate) fn capacity(&self) -> usize {
        self.proposed.capacity() + self.orphan.capacity()
    }

    pub(crate) fn touch_last_txs_updated_at(&mut self) {
        self.last_txs_updated_at = unix_time_as_millis();
    }

    pub fn proposed_txs_iter(&self) -> impl Iterator<Item = &PoolEntry> {
        self.proposed.txs_iter()
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.conflict.contains_key(id)
            || self.proposed.contains_key(id)
            || self.orphan.contains_key(id)
    }

    pub fn get_entry(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.pending
            .get(id)
            .or_else(|| self.proposed.get(id))
            .or_else(|| self.orphan.get(id))
            .or_else(|| self.conflict.get(id))
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .get_tx(id)
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.get(id).map(|e| &e.transaction))
            .cloned()
    }

    pub fn get_tx_without_conflict(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .get_tx(id)
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .cloned()
    }

    pub fn get_tx_from_proposed(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.proposed.get_tx(id).cloned()
    }

    //FIXME: use memsize
    pub fn is_full(&self) -> bool {
        self.capacity() > self.config.max_pool_size
    }

    pub(crate) fn remove_committed_txs_from_proposed<'a>(
        &mut self,
        txs: impl Iterator<Item = &'a Transaction>,
    ) {
        for tx in txs {
            let hash = tx.hash();
            trace!(target: "tx_pool", "committed {:#x}", hash);
            self.proposed.remove_committed_tx(tx);
        }
    }

    pub fn remove_expired<'a>(&mut self, ids: impl Iterator<Item = &'a ProposalShortId>) {
        for id in ids {
            if let Some(entries) = self.proposed.remove(id) {
                for entry in entries {
                    self.enqueue_tx(entry.cycles, entry.transaction);
                }
            }
        }
    }
}
