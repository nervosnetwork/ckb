//! Top-level Pool type, methods, and tests
use super::types::{DefectEntry, ProposedEntry, TxPoolConfig};
use crate::tx_pool::orphan::OrphanPool;
use crate::tx_pool::pending::PendingQueue;
use crate::tx_pool::proposed::ProposedPool;
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::{Capacity, Cycle};
use ckb_logger::{error_target, trace_target};
use faketime::unix_time_as_millis;
use lru_cache::LruCache;

#[derive(Debug, Clone)]
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    /// The short id that has not been proposed
    pub(crate) pending: PendingQueue,
    /// The proposal gap
    pub(crate) gap: PendingQueue,
    /// Tx pool that finely for commit
    pub(crate) proposed: ProposedPool,
    /// Orphans in the pool
    pub(crate) orphan: OrphanPool,
    /// cache for conflict transaction
    pub(crate) conflict: LruCache<ProposalShortId, DefectEntry>,
    /// last txs updated timestamp, used by getblocktemplate
    pub(crate) last_txs_updated_at: u64,
    // sum of all tx_pool tx's virtual sizes.
    pub(crate) total_tx_size: usize,
    // sum of all tx_pool tx's cycles.
    pub(crate) total_tx_cycles: Cycle,
}

impl TxPool {
    pub fn new(config: TxPoolConfig) -> TxPool {
        let cache_size = config.max_verfify_cache_size;
        let last_txs_updated_at = 0u64;

        TxPool {
            config,
            pending: PendingQueue::new(),
            gap: PendingQueue::new(),
            proposed: ProposedPool::new(),
            orphan: OrphanPool::new(),
            conflict: LruCache::new(cache_size),
            last_txs_updated_at,
            total_tx_size: 0,
            total_tx_cycles: 0,
        }
    }

    pub fn pending_size(&self) -> u32 {
        self.pending.size() as u32
    }
    pub fn gap_size(&self) -> u32 {
        self.gap.size() as u32
    }
    pub fn proposed_size(&self) -> u32 {
        self.proposed.vertices.len() as u32
    }
    pub fn orphan_size(&self) -> u32 {
        self.orphan.vertices.len() as u32
    }

    pub fn total_tx_size(&self) -> usize {
        self.total_tx_size
    }

    pub fn total_tx_cycles(&self) -> Cycle {
        self.total_tx_cycles
    }

    pub fn reach_size_limit(&self, tx_size: usize) -> bool {
        (self.total_tx_size + tx_size) > self.config.max_mem_size
    }

    pub fn reach_cycles_limit(&self, cycles: Cycle) -> bool {
        (self.total_tx_cycles + cycles) > self.config.max_cycles
    }

    pub fn update_statics_for_add_tx(&mut self, tx_size: usize, cycles: Cycle) {
        self.total_tx_size += tx_size;
        self.total_tx_cycles += cycles;
    }

    // cycles overflow is possible, currently obtaining cycles is not accurate
    pub fn update_statics_for_remove_tx(&mut self, tx_size: usize, cycles: Cycle) {
        let total_tx_size = self.total_tx_size.checked_sub(tx_size).unwrap_or_else(|| {
            error_target!(
                crate::LOG_TARGET_TX_POOL,
                "total_tx_size {} overflow by sub {}",
                self.total_tx_size,
                tx_size
            );
            0
        });
        let total_tx_cycles = self.total_tx_cycles.checked_sub(cycles).unwrap_or_else(|| {
            error_target!(
                crate::LOG_TARGET_TX_POOL,
                "total_tx_cycles {} overflow by sub {}",
                self.total_tx_cycles,
                cycles
            );
            0
        });
        self.total_tx_size = total_tx_size;
        self.total_tx_cycles = total_tx_cycles;
    }

    // enqueue_tx inserts a new transaction into pending queue.
    // If did have this value present, false is returned.
    pub fn enqueue_tx(&mut self, cycles: Option<Cycle>, size: usize, tx: Transaction) -> bool {
        if self.gap.contains_key(&tx.proposal_short_id()) {
            return false;
        }
        self.pending.add_tx(cycles, size, tx).is_none()
    }

    // add_gap inserts proposed but still uncommittable transaction.
    pub fn add_gap(&mut self, cycles: Option<Cycle>, size: usize, tx: Transaction) -> bool {
        self.gap.add_tx(cycles, size, tx).is_none()
    }

    pub(crate) fn add_orphan(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: Transaction,
        unknowns: Vec<OutPoint>,
    ) -> Option<DefectEntry> {
        trace_target!(crate::LOG_TARGET_TX_POOL, "add_orphan {:#x}", &tx.hash());
        self.orphan.add_tx(cycles, size, tx, unknowns.into_iter())
    }

    pub(crate) fn add_proposed(
        &mut self,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        tx: Transaction,
    ) {
        trace_target!(crate::LOG_TARGET_TX_POOL, "add_proposed {:#x}", tx.hash());
        self.touch_last_txs_updated_at();
        self.proposed.add_tx(cycles, fee, size, tx);
    }

    pub(crate) fn touch_last_txs_updated_at(&mut self) {
        self.last_txs_updated_at = unix_time_as_millis();
    }

    pub fn proposed_txs_iter(&self) -> impl Iterator<Item = &ProposedEntry> {
        self.proposed.txs_iter()
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.conflict.contains_key(id)
            || self.proposed.contains_key(id)
            || self.orphan.contains_key(id)
    }

    pub fn get_tx_with_cycles(&self, id: &ProposalShortId) -> Option<(Transaction, Option<Cycle>)> {
        self.pending
            .get(id)
            .cloned()
            .map(|entry| (entry.transaction, entry.cycles))
            .or_else(|| {
                self.gap
                    .get(id)
                    .cloned()
                    .map(|entry| (entry.transaction, entry.cycles))
            })
            .or_else(|| {
                self.proposed
                    .get(id)
                    .cloned()
                    .map(|entry| (entry.transaction, Some(entry.cycles)))
            })
            .or_else(|| {
                self.orphan
                    .get(id)
                    .cloned()
                    .map(|entry| (entry.transaction, entry.cycles))
            })
            .or_else(|| {
                self.conflict
                    .get(id)
                    .cloned()
                    .map(|entry| (entry.transaction, entry.cycles))
            })
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.get(id).map(|e| &e.transaction))
            .cloned()
    }

    pub fn get_tx_without_conflict(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .cloned()
    }

    pub fn get_tx_from_proposed(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.proposed.get_tx(id).cloned()
    }

    pub(crate) fn remove_committed_txs_from_proposed<'a>(
        &mut self,
        txs: impl Iterator<Item = &'a Transaction>,
    ) {
        for tx in txs {
            let hash = tx.hash();
            trace_target!(crate::LOG_TARGET_TX_POOL, "committed {:#x}", hash);
            for entry in self.proposed.remove_committed_tx(tx) {
                self.update_statics_for_remove_tx(entry.size, entry.cycles);
            }
        }
    }

    pub fn remove_expired<'a>(&mut self, ids: impl Iterator<Item = &'a ProposalShortId>) {
        for id in ids {
            if let Some(entry) = self.gap.remove(id) {
                self.enqueue_tx(entry.cycles, entry.size, entry.transaction);
            }
            for entry in self.proposed.remove(id) {
                self.enqueue_tx(Some(entry.cycles), entry.size, entry.transaction);
            }
        }
    }
}
