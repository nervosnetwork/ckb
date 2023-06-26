//! Top-level Pool type, methods, and tests
extern crate rustc_hash;
extern crate slab;
use super::component::{commit_txs_scanner::CommitTxsScanner, TxEntry};
use crate::callback::Callbacks;
use crate::component::pool_map::{PoolEntry, PoolMap, Status};
use crate::component::recent_reject::RecentReject;
use crate::error::Reject;
use ckb_app_config::TxPoolConfig;
use ckb_logger::{debug, error, warn};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{resolve_transaction, OverlayCellChecker, OverlayCellProvider, ResolvedTransaction},
        tx_pool::{TxPoolEntryInfo, TxPoolIds},
        Cycle, TransactionView, UncleBlockView,
    },
    packed::{Byte32, ProposalShortId},
};
use lru::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;

/// Tx-pool implementation
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    pub(crate) pool_map: PoolMap,
    /// cache for committed transactions hash
    pub(crate) committed_txs_hash_cache: LruCache<ProposalShortId, Byte32>,
    // sum of all tx_pool tx's virtual sizes.
    pub(crate) total_tx_size: usize,
    // sum of all tx_pool tx's cycles.
    pub(crate) total_tx_cycles: Cycle,
    /// storage snapshot reference
    pub(crate) snapshot: Arc<Snapshot>,
    /// record recent reject
    pub recent_reject: Option<RecentReject>,
    // expiration milliseconds,
    pub(crate) expiry: u64,
}

impl TxPool {
    /// Create new TxPool
    pub fn new(config: TxPoolConfig, snapshot: Arc<Snapshot>) -> TxPool {
        let recent_reject = Self::build_recent_reject(&config);
        let expiry = config.expiry_hours as u64 * 60 * 60 * 1000;
        TxPool {
            pool_map: PoolMap::new(config.max_ancestors_count),
            committed_txs_hash_cache: LruCache::new(COMMITTED_HASH_CACHE_SIZE),
            total_tx_size: 0,
            total_tx_cycles: 0,
            config,
            snapshot,
            recent_reject,
            expiry,
        }
    }

    /// Tx-pool owned snapshot, it may not consistent with chain cause tx-pool update snapshot asynchronously
    pub(crate) fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Makes a clone of the `Arc<Snapshot>`
    pub(crate) fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    fn get_by_status(&self, status: &Status) -> Vec<&PoolEntry> {
        self.pool_map.get_by_status(status)
    }

    /// Get tx-pool size
    pub fn status_size(&self, status: &Status) -> usize {
        self.get_by_status(status).len()
    }

    /// Update size and cycles statics for add tx
    pub fn update_statics_for_add_tx(&mut self, tx_size: usize, cycles: Cycle) {
        self.total_tx_size += tx_size;
        self.total_tx_cycles += cycles;
    }

    /// Update size and cycles statics for remove tx
    /// cycles overflow is possible, currently obtaining cycles is not accurate
    pub fn update_statics_for_remove_tx(&mut self, tx_size: usize, cycles: Cycle) {
        let total_tx_size = self.total_tx_size.checked_sub(tx_size).unwrap_or_else(|| {
            error!(
                "total_tx_size {} overflow by sub {}",
                self.total_tx_size, tx_size
            );
            0
        });
        let total_tx_cycles = self.total_tx_cycles.checked_sub(cycles).unwrap_or_else(|| {
            error!(
                "total_tx_cycles {} overflow by sub {}",
                self.total_tx_cycles, cycles
            );
            0
        });
        self.total_tx_size = total_tx_size;
        self.total_tx_cycles = total_tx_cycles;
    }

    /// Add tx with pending status
    /// If did have this value present, false is returned.
    pub(crate) fn add_pending(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        self.pool_map.add_entry(entry, Status::Pending)
    }

    /// Add tx which proposed but still uncommittable to gap
    pub(crate) fn add_gap(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        self.pool_map.add_entry(entry, Status::Gap)
    }

    /// Add tx with proposed status
    pub(crate) fn add_proposed(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        self.pool_map.add_entry(entry, Status::Proposed)
    }

    /// Returns true if the tx-pool contains a tx with specified id.
    pub(crate) fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pool_map.get_by_id(id).is_some()
    }

    pub(crate) fn set_entry_proposed(&mut self, short_id: &ProposalShortId) {
        self.pool_map.set_entry(short_id, Status::Proposed)
    }

    pub(crate) fn set_entry_gap(&mut self, short_id: &ProposalShortId) {
        self.pool_map.set_entry(short_id, Status::Gap)
    }

    /// Returns tx with cycles corresponding to the id.
    pub(crate) fn get_tx_with_cycles(
        &self,
        id: &ProposalShortId,
    ) -> Option<(TransactionView, Cycle)> {
        self.pool_map
            .get_by_id(id)
            .map(|entry| (entry.inner.transaction().clone(), entry.inner.cycles))
    }

    pub(crate) fn get_pool_entry(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.pool_map.get_by_id(id)
    }

    pub(crate) fn get_tx_from_pool(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.pool_map
            .get_by_id(id)
            .map(|entry| entry.inner.transaction())
    }

    pub(crate) fn remove_committed_txs<'a>(
        &mut self,
        txs: impl Iterator<Item = &'a TransactionView>,
        callbacks: &Callbacks,
        detached_headers: &HashSet<Byte32>,
    ) {
        for tx in txs {
            let tx_hash = tx.hash();
            self.remove_committed_tx(tx, callbacks);

            self.committed_txs_hash_cache
                .put(tx.proposal_short_id(), tx_hash);
        }

        if !detached_headers.is_empty() {
            self.resolve_conflict_header_dep(detached_headers, callbacks)
        }
    }

    fn resolve_conflict_header_dep(
        &mut self,
        detached_headers: &HashSet<Byte32>,
        callbacks: &Callbacks,
    ) {
        for (entry, reject) in self.pool_map.resolve_conflict_header_dep(detached_headers) {
            callbacks.call_reject(self, &entry, reject);
        }
    }

    fn remove_committed_tx(&mut self, tx: &TransactionView, callbacks: &Callbacks) {
        let short_id = tx.proposal_short_id();
        if let Some(entry) = self.pool_map.remove_entry(&short_id) {
            callbacks.call_committed(self, &entry)
        }
        {
            let conflicts = self.pool_map.resolve_conflict(tx);
            for (entry, reject) in conflicts {
                callbacks.call_reject(self, &entry, reject);
            }
        }
    }

    // Expire all transaction (and their dependencies) in the pool.
    pub(crate) fn remove_expired(&mut self, callbacks: &Callbacks) {
        let now_ms = ckb_systemtime::unix_time_as_millis();
        let removed: Vec<_> = self
            .pool_map
            .iter()
            .filter(|&entry| self.expiry + entry.inner.timestamp < now_ms)
            .map(|entry| entry.inner.clone())
            .collect();

        for entry in removed {
            self.pool_map.remove_entry(&entry.proposal_short_id());
            let reject = Reject::Expiry(entry.timestamp);
            callbacks.call_reject(self, &entry, reject);
        }
    }

    // Remove transactions from the pool until total size <= size_limit.
    pub(crate) fn limit_size(&mut self, callbacks: &Callbacks) {
        while self.total_tx_size > self.config.max_tx_pool_size {
            let next_evict_entry = || {
                self.pool_map
                    .next_evict_entry(Status::Pending)
                    .or_else(|| self.pool_map.next_evict_entry(Status::Gap))
                    .or_else(|| self.pool_map.next_evict_entry(Status::Proposed))
            };

            if let Some(id) = next_evict_entry() {
                let removed = self.pool_map.remove_entry_and_descendants(&id);
                for entry in removed {
                    let tx_hash = entry.transaction().hash();
                    debug!(
                        "removed by size limit {} timestamp({})",
                        tx_hash, entry.timestamp
                    );
                    let reject = Reject::Full(format!(
                        "the fee_rate for this transaction is: {}",
                        entry.fee_rate()
                    ));
                    callbacks.call_reject(self, &entry, reject);
                }
            }
        }
        self.pool_map.entries.shrink_to_fit();
    }

    // remove transaction with detached proposal from gap and proposed
    // try re-put to pending
    pub(crate) fn remove_by_detached_proposal<'a>(
        &mut self,
        ids: impl Iterator<Item = &'a ProposalShortId>,
    ) {
        for id in ids {
            if let Some(e) = self.pool_map.get_by_id(id) {
                let status = e.status;
                if status == Status::Pending {
                    continue;
                }
                let mut entries = self.pool_map.remove_entry_and_descendants(id);
                entries.sort_unstable_by_key(|entry| entry.ancestors_count);
                for mut entry in entries {
                    let tx_hash = entry.transaction().hash();
                    entry.reset_statistic_state();
                    let ret = self.add_pending(entry);
                    debug!(
                        "remove_by_detached_proposal from {:?} {} add_pending {:?}",
                        status, tx_hash, ret
                    );
                }
            }
        }
    }

    pub(crate) fn remove_tx(&mut self, id: &ProposalShortId) -> bool {
        let entries = self.pool_map.remove_entry_and_descendants(id);
        if !entries.is_empty() {
            for entry in entries {
                self.update_statics_for_remove_tx(entry.size, entry.cycles);
            }
            return true;
        }

        if let Some(entry) = self.pool_map.remove_entry(id) {
            self.update_statics_for_remove_tx(entry.size, entry.cycles);
            return true;
        }
        false
    }

    pub(crate) fn check_rtx_from_pool(&self, rtx: &ResolvedTransaction) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let checker = OverlayCellChecker::new(&self.pool_map, snapshot);
        let mut seen_inputs = HashSet::new();
        rtx.check(&mut seen_inputs, &checker, snapshot)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn resolve_tx_from_pool(
        &self,
        tx: TransactionView,
    ) -> Result<Arc<ResolvedTransaction>, Reject> {
        let snapshot = self.snapshot();
        let provider = OverlayCellProvider::new(&self.pool_map, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(tx, &mut seen_inputs, &provider, snapshot)
            .map(Arc::new)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn gap_rtx(&mut self, short_id: &ProposalShortId) -> Result<(), Reject> {
        match self.get_pool_entry(short_id) {
            Some(entry) => {
                let tx_hash = entry.inner.transaction().hash();
                if entry.status == Status::Gap {
                    Err(Reject::Duplicated(tx_hash))
                } else {
                    debug!("gap_rtx: {:?} => {:?}", tx_hash, short_id);
                    self.set_entry_gap(short_id);
                    Ok(())
                }
            }
            None => Err(Reject::Malformed(String::from("invalid short_id"))),
        }
    }

    pub(crate) fn proposed_rtx(&mut self, short_id: &ProposalShortId) -> Result<(), Reject> {
        match self.get_pool_entry(short_id) {
            Some(entry) => {
                let tx_hash = entry.inner.transaction().hash();
                if entry.status == Status::Proposed {
                    Err(Reject::Duplicated(tx_hash))
                } else {
                    debug!("proposed_rtx: {:?} => {:?}", tx_hash, short_id);
                    self.set_entry_proposed(short_id);
                    Ok(())
                }
            }
            None => Err(Reject::Malformed(String::from("invalid short_id"))),
        }
    }

    /// Get to-be-proposal transactions that may be included in the next block.
    pub(crate) fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        let mut proposals = HashSet::with_capacity(limit);
        self.pool_map
            .fill_proposals(limit, exclusion, &mut proposals, &Status::Pending);
        self.pool_map
            .fill_proposals(limit, exclusion, &mut proposals, &Status::Gap);
        proposals
    }

    /// Returns tx from tx-pool or storage corresponding to the id.
    pub(crate) fn get_tx_from_pool_or_store(
        &self,
        proposal_id: &ProposalShortId,
    ) -> Option<TransactionView> {
        self.get_tx_from_pool(proposal_id).cloned().or_else(|| {
            self.committed_txs_hash_cache
                .peek(proposal_id)
                .and_then(|tx_hash| self.snapshot().get_transaction(tx_hash).map(|(tx, _)| tx))
        })
    }

    pub(crate) fn get_ids(&self) -> TxPoolIds {
        let pending: Vec<Byte32> = self
            .get_by_status(&Status::Pending)
            .iter()
            .chain(self.get_by_status(&Status::Gap).iter())
            .map(|entry| entry.inner.transaction().hash())
            .collect();

        let proposed: Vec<Byte32> = self
            .get_by_status(&Status::Proposed)
            .iter()
            .map(|entry| entry.inner.transaction().hash())
            .collect();

        TxPoolIds { pending, proposed }
    }

    pub(crate) fn get_all_entry_info(&self) -> TxPoolEntryInfo {
        let pending = self
            .get_by_status(&Status::Pending)
            .iter()
            .chain(self.get_by_status(&Status::Gap).iter())
            .map(|entry| (entry.inner.transaction().hash(), entry.inner.to_info()))
            .collect();

        let proposed = self
            .get_by_status(&Status::Proposed)
            .iter()
            .map(|entry| (entry.inner.transaction().hash(), entry.inner.to_info()))
            .collect();

        TxPoolEntryInfo { pending, proposed }
    }

    pub(crate) fn drain_all_transactions(&mut self) -> Vec<TransactionView> {
        let mut txs = CommitTxsScanner::new(&self.pool_map)
            .txs_to_commit(self.total_tx_size, self.total_tx_cycles)
            .0
            .into_iter()
            .map(|tx_entry| tx_entry.into_transaction())
            .collect::<Vec<_>>();
        let mut pending = self
            .pool_map
            .entries
            .remove_by_status(&Status::Pending)
            .into_iter()
            .map(|e| e.inner.into_transaction())
            .collect::<Vec<_>>();
        txs.append(&mut pending);
        let mut gap = self
            .pool_map
            .entries
            .remove_by_status(&Status::Gap)
            .into_iter()
            .map(|e| e.inner.into_transaction())
            .collect::<Vec<_>>();
        txs.append(&mut gap);
        self.total_tx_size = 0;
        self.total_tx_cycles = 0;
        self.pool_map.clear();
        txs
    }

    pub(crate) fn clear(&mut self, snapshot: Arc<Snapshot>) {
        self.pool_map.clear();
        self.snapshot = snapshot;
        self.committed_txs_hash_cache = LruCache::new(COMMITTED_HASH_CACHE_SIZE);
        self.total_tx_size = 0;
        self.total_tx_cycles = 0;
    }

    pub(crate) fn package_proposals(
        &self,
        proposals_limit: u64,
        uncles: &[UncleBlockView],
    ) -> HashSet<ProposalShortId> {
        let uncle_proposals = uncles
            .iter()
            .flat_map(|u| u.data().proposals().into_iter())
            .collect();
        self.get_proposals(proposals_limit as usize, &uncle_proposals)
    }

    pub(crate) fn package_txs(
        &self,
        max_block_cycles: Cycle,
        txs_size_limit: usize,
    ) -> (Vec<TxEntry>, usize, Cycle) {
        let (entries, size, cycles) =
            CommitTxsScanner::new(&self.pool_map).txs_to_commit(txs_size_limit, max_block_cycles);

        if !entries.is_empty() {
            ckb_logger::info!(
                "[get_block_template] candidate txs count: {}, size: {}/{}, cycles:{}/{}",
                entries.len(),
                size,
                txs_size_limit,
                cycles,
                max_block_cycles
            );
        }
        (entries, size, cycles)
    }

    fn build_recent_reject(config: &TxPoolConfig) -> Option<RecentReject> {
        if !config.recent_reject.as_os_str().is_empty() {
            let recent_reject_ttl =
                u8::max(1, config.keep_rejected_tx_hashes_days) as i32 * 24 * 60 * 60;
            match RecentReject::new(
                &config.recent_reject,
                config.keep_rejected_tx_hashes_count,
                recent_reject_ttl,
            ) {
                Ok(recent_reject) => Some(recent_reject),
                Err(err) => {
                    error!(
                        "Failed to open recent reject database {:?} {}",
                        config.recent_reject, err
                    );
                    None
                }
            }
        } else {
            warn!("Recent reject database is disabled!");
            None
        }
    }
}
