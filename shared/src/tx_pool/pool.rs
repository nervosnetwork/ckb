//! Top-level Pool type, methods, and tests
use super::trace::{TxTrace, TxTraceMap};
use super::types::{OrphanPool, PendingQueue, PoolEntry, StagingPool, TxPoolConfig};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use faketime::unix_time_as_millis;
use log::trace;
use lru_cache::LruCache;
use numext_fixed_hash::H256;

#[derive(Debug, Clone)]
pub(crate) struct TxFilter {
    map: LruCache<H256, ()>,
}

impl TxFilter {
    pub fn new(size: usize) -> TxFilter {
        TxFilter {
            map: LruCache::new(size),
        }
    }

    pub fn insert(&mut self, hash: H256) -> bool {
        self.map.insert(hash, ()).is_none()
    }
}

#[derive(Debug, Clone)]
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    /// Already known transaction filter
    pub(crate) filter: TxFilter,
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
            filter: TxFilter::new(1000),
            pending: PendingQueue::new(),
            staging: StagingPool::new(),
            orphan: OrphanPool::new(),
            conflict: LruCache::new(cache_size),
            last_txs_updated_at,
            trace: TxTraceMap::new(trace_size),
        }
    }

    // enqueue_tx inserts a new transaction into the non-verifiable transaction queue.
    pub fn enqueue_tx(&mut self, tx: Transaction) -> bool {
        let tx_hash = tx.hash();
        if !self.filter.insert(tx_hash.clone()) {
            trace!(target: "tx_pool", "discarding already known transaction {:#x}", tx_hash);
            return false;
        }

        let short_id = tx.proposal_short_id();
        let entry = PoolEntry::new(tx, 0, None);
        self.pending.insert(short_id, entry).is_none()
    }

    // trace_tx basically same as enqueue_tx, but additional register a trace.
    pub fn trace_tx(&mut self, tx: Transaction) -> bool {
        let tx_hash = tx.hash();
        if !self.filter.insert(tx_hash.clone()) {
            trace!(target: "tx_pool", "discarding already known transaction {:#x}", tx_hash);
            return false;
        }
        let short_id = tx.proposal_short_id();
        let entry = PoolEntry::new(tx, 0, None);

        if self.config.trace_enable() {
            self.trace
                .add_pending(&entry.transaction.hash(), "unknown tx, add to pending");
        }
        self.pending.insert(short_id, entry).is_none()
    }

    pub fn get_tx_traces(&self, hash: &H256) -> Option<&Vec<TxTrace>> {
        self.trace.get(hash)
    }

    pub(crate) fn add_orphan(&mut self, entry: PoolEntry, unknowns: Vec<OutPoint>) {
        if self.config.trace_enable() {
            self.trace.add_orphan(
                &entry.transaction.hash(),
                format!("unknowns {:?}", unknowns),
            );
        }
        self.orphan.add_tx(entry, unknowns.into_iter());
    }

    pub(crate) fn add_staging(&mut self, entry: PoolEntry) {
        if self.config.trace_enable() {
            self.trace
                .staged(&entry.transaction.hash(), "staged".to_string());
        }
        self.touch_last_txs_updated_at();
        self.staging.add_tx(entry);
    }

    pub(crate) fn remove_pending_from_proposal(
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

    pub fn remove_staged(&mut self, ids: &[ProposalShortId]) {
        for id in ids {
            if let Some(txs) = self.staging.remove(id) {
                self.pending.insert(*id, txs[0].clone());

                for tx in txs.iter().skip(1) {
                    self.conflict
                        .insert(tx.transaction.proposal_short_id(), tx.clone());
                }
            } else if let Some(tx) = self.conflict.remove(id) {
                self.pending.insert(*id, tx);
            } else if let Some(tx) = self.orphan.remove(id) {
                self.pending.insert(*id, tx);
            }
        }
    }
}
