//! Top-level Pool type, methods, and tests
use super::trace::TxTraceMap;
use super::types::{OrphanPool, PendingQueue, PoolEntry, StagingPool, TxPoolConfig};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use faketime::unix_time_as_millis;
use jsonrpc_types::TxTrace;
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
    pub fn enqueue_tx(&mut self, entry: PoolEntry) -> bool {
        let tx_hash = entry.transaction.hash();
        if !self.filter.insert(tx_hash.clone()) {
            trace!(target: "tx_pool", "discarding already known transaction {:#x}", tx_hash);
            return false;
        }

        let short_id = entry.transaction.proposal_short_id();
        self.pending.insert(short_id, entry).is_none()
    }

    // trace_tx basically same as enqueue_tx, but additional register a trace.
    pub fn trace_tx(&mut self, entry: PoolEntry) -> bool {
        let tx_hash = entry.transaction.hash();
        if !self.filter.insert(tx_hash.clone()) {
            trace!(target: "tx_pool", "discarding already known transaction {:#x}", tx_hash);
            return false;
        }
        let short_id = entry.transaction.proposal_short_id();

        if self.config.trace_enable() {
            self.trace.add_pending(
                &entry.transaction.hash(),
                "unknown tx, insert to pending queue",
            );
        }
        self.pending.insert(short_id, entry).is_none()
    }

    pub fn get_tx_traces(&self, hash: &H256) -> Option<&Vec<TxTrace>> {
        self.trace.get(hash)
    }

    pub(crate) fn add_orphan(&mut self, entry: PoolEntry, unknowns: Vec<OutPoint>) {
        trace!(target: "tx_pool", "add_orphan {:#x}", &entry.transaction.hash());
        if self.config.trace_enable() {
            self.trace.add_orphan(
                &entry.transaction.hash(),
                format!("orphan tx, unknown inputs/deps {:?}", unknowns),
            );
        }
        self.orphan.add_tx(entry, unknowns.into_iter());
    }

    pub(crate) fn add_staging(&mut self, entry: PoolEntry) {
        trace!(target: "tx_pool", "add_staging {:#x}", &entry.transaction.hash());
        if self.config.trace_enable() {
            self.trace
                .staged(&entry.transaction.hash(), "tx staged".to_string());
        }
        self.touch_last_txs_updated_at();
        self.staging.add_tx(entry);
    }

    pub(crate) fn committed(&mut self, tx: &Transaction) {
        let hash = tx.hash();
        trace!(target: "tx_pool", "committed {:#x}", hash);
        if self.config.trace_enable() {
            self.trace.committed(&hash, "tx committed".to_string());
        }
        self.staging.commit_tx(tx);
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

    pub fn remove_expired(&mut self, ids: &[ProposalShortId]) {
        for id in ids {
            if let Some(entries) = self.staging.remove(id) {
                let first = entries[0].clone();
                if self.config.trace_enable() {
                    self.trace
                        .expired(&first.transaction.hash(), "tx proposal expired".to_string());
                }
                self.pending.insert(*id, first);

                for entry in entries.into_iter().skip(1) {
                    if self.config.trace_enable() {
                        self.trace
                            .expired(&entry.transaction.hash(), "tx proposal expired".to_string());
                    }
                    self.conflict
                        .insert(entry.transaction.proposal_short_id(), entry);
                }
            } else if let Some(entry) = self.conflict.remove(id) {
                if self.config.trace_enable() {
                    self.trace
                        .expired(&entry.transaction.hash(), "tx proposal expired".to_string());
                }
                self.pending.insert(*id, entry);
            } else if let Some(entry) = self.orphan.remove(id) {
                if self.config.trace_enable() {
                    self.trace
                        .expired(&entry.transaction.hash(), "tx proposal expired".to_string());
                }
                self.pending.insert(*id, entry);
            }
        }
    }
}
