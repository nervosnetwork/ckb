//! Top-level Pool type, methods, and tests
use super::trace::TxTraceMap;
use super::types::{PoolEntry, TxPoolConfig};
use crate::tx_pool::orphan::OrphanPool;
use crate::tx_pool::pending::PendingQueue;
use crate::tx_pool::staging::StagingPool;
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use faketime::unix_time_as_millis;
use jsonrpc_types::TxTrace;
use log::trace;
use lru_cache::LruCache;
use numext_fixed_hash::H256;

#[derive(Debug, Clone)]
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    /// The short id that has not been proposed
    pub(crate) pending: PendingQueue,
    /// Tx pool that finely for commit
    pub(crate) staging: StagingPool,
    /// Orphans in the pool
    pub(crate) orphan: OrphanPool,
    /// cache for conflict transaction
    pub(crate) conflict: LruCache<ProposalShortId, PoolEntry>,
    /// trace record map
    pub(crate) trace: TxTraceMap,
    /// last txs updated timestamp
    pub(crate) last_txs_updated_at: u64,
}

impl TxPool {
    pub fn new(config: TxPoolConfig) -> TxPool {
        let cache_size = config.max_cache_size;
        let trace_size = config.trace.unwrap_or(0);
        let last_txs_updated_at = 0u64;

        TxPool {
            config,
            pending: PendingQueue::new(),
            staging: StagingPool::new(),
            orphan: OrphanPool::new(),
            conflict: LruCache::new(cache_size),
            last_txs_updated_at,
            trace: TxTraceMap::new(trace_size),
        }
    }

    // enqueue_tx inserts a new transaction into the non-verifiable transaction queue.
    pub fn enqueue_tx(&mut self, cycles: Option<Cycle>, tx: Transaction) -> bool {
        self.pending.add_tx(cycles, tx).is_none()
    }

    // trace_tx basically same as enqueue_tx, but additional register a trace.
    pub fn trace_tx(&mut self, tx: Transaction) -> bool {
        if self.config.trace_enable() {
            self.trace
                .add_pending(&tx.hash(), "unknown tx, insert to pending queue");
        }
        self.pending.add_tx(None, tx).is_none()
    }

    pub fn get_tx_traces(&self, hash: &H256) -> Option<&Vec<TxTrace>> {
        self.trace.get(hash)
    }

    pub(crate) fn add_orphan(
        &mut self,
        cycles: Option<Cycle>,
        tx: Transaction,
        unknowns: Vec<OutPoint>,
    ) {
        trace!(target: "tx_pool", "add_orphan {:#x}", &tx.hash());
        if self.config.trace_enable() {
            self.trace.add_orphan(
                &tx.hash(),
                format!("orphan tx, unknown inputs/deps {:?}", unknowns),
            );
        }
        self.orphan.add_tx(cycles, tx, unknowns.into_iter());
    }

    pub(crate) fn add_staging(&mut self, cycles: Cycle, tx: Transaction) {
        trace!(target: "tx_pool", "add_staging {:#x}", tx.hash());
        if self.config.trace_enable() {
            self.trace.staged(&tx.hash(), "tx staged".to_string());
        }
        self.touch_last_txs_updated_at();
        self.staging.add_tx(cycles, tx);
    }

    pub(crate) fn remove_pending_and_conflict(
        &mut self,
        id: &ProposalShortId,
    ) -> Option<PoolEntry> {
        self.pending
            .remove(id)
            .or_else(|| self.conflict.remove(id))
            .map(|entry| {
                if self.config.trace_enable() {
                    self.trace
                        .proposed(&entry.transaction.hash(), format!("{:?} proposed", id));
                }
                entry
            })
    }

    pub(crate) fn capacity(&self) -> usize {
        self.staging.capacity() + self.orphan.capacity()
    }

    pub(crate) fn touch_last_txs_updated_at(&mut self) {
        self.last_txs_updated_at = unix_time_as_millis();
    }

    pub fn staging_txs_iter(&self) -> impl Iterator<Item = &PoolEntry> {
        self.staging.txs_iter()
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.conflict.contains_key(id)
            || self.staging.contains_key(id)
            || self.orphan.contains_key(id)
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<Transaction> {
        self.pending
            .get_tx(id)
            .or_else(|| self.staging.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.get(id).map(|e| &e.transaction))
            .cloned()
    }

    //FIXME: use memsize
    pub fn is_full(&self) -> bool {
        self.capacity() > self.config.max_pool_size
    }

    pub(crate) fn remove_committed_txs_from_staging<'a>(
        &mut self,
        txs: impl Iterator<Item = &'a Transaction>,
    ) {
        for tx in txs {
            let hash = tx.hash();
            trace!(target: "tx_pool", "committed {:#x}", hash);
            if self.config.trace_enable() {
                self.trace.committed(&hash, "tx committed".to_string());
            }
            self.staging.remove_committed_tx(tx);
        }
    }

    pub fn remove_expired<'a>(&mut self, ids: impl Iterator<Item = &'a ProposalShortId>) {
        for id in ids {
            if let Some(entries) = self.staging.remove(id) {
                if self.config.trace_enable() {
                    for entry in entries {
                        self.trace
                            .expired(&entry.transaction.hash(), "tx proposal expired".to_string());
                        self.enqueue_tx(entry.cycles, entry.transaction);
                    }
                }
            }
        }
    }
}
