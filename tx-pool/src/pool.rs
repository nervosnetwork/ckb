//! Top-level Pool type, methods, and tests
use super::component::{commit_txs_scanner::CommitTxsScanner, TxEntry};
use crate::callback::Callbacks;
use crate::component::pending::PendingQueue;
use crate::component::proposed::ProposedPool;
use crate::component::recent_reject::RecentReject;
use crate::error::Reject;
use crate::util::verify_rtx;
use ckb_app_config::TxPoolConfig;
use ckb_logger::{debug, error, trace, warn};
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
use ckb_verification::{cache::CacheEntry, TxVerifyEnv};
use lru::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;

// limit the size of the pool by sorting out tx based on EvictKey.
macro_rules! evict_for_trim_size {
    ($self:ident, $pool:expr, $callbacks:expr) => {
        if let Some(id) = $pool
            .iter()
            .min_by_key(|(_id, entry)| entry.as_evict_key())
            .map(|(id, _)| id)
            .cloned()
        {
            let removed = $pool.remove_entry_and_descendants(&id);
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
                $callbacks.call_reject($self, &entry, reject);
            }
        }
    };
}

/// Tx-pool implementation
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    /// The short id that has not been proposed
    pub(crate) pending: PendingQueue,
    /// The proposal gap
    pub(crate) gap: PendingQueue,
    /// Tx pool that finely for commit
    pub(crate) proposed: ProposedPool,
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
        let recent_reject = build_recent_reject(&config);
        let expiry = config.expiry_hours as u64 * 60 * 60 * 1000;
        TxPool {
            pending: PendingQueue::new(),
            gap: PendingQueue::new(),
            proposed: ProposedPool::new(config.max_ancestors_count),
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
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Makes a clone of the `Arc<Snapshot>`
    pub fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Whether Tx-pool reach size limit
    pub fn reach_size_limit(&self, tx_size: usize) -> bool {
        (self.total_tx_size + tx_size) > self.config.max_tx_pool_size
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

    /// Add tx to pending pool
    /// If did have this value present, false is returned.
    pub fn add_pending(&mut self, entry: TxEntry) -> bool {
        if self.gap.contains_key(&entry.proposal_short_id()) {
            return false;
        }
        trace!("add_pending {}", entry.transaction().hash());
        self.pending.add_entry(entry)
    }

    /// Add tx which proposed but still uncommittable to gap pool
    pub fn add_gap(&mut self, entry: TxEntry) -> bool {
        trace!("add_gap {}", entry.transaction().hash());
        self.gap.add_entry(entry)
    }

    /// Add tx to proposed pool
    pub fn add_proposed(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        trace!("add_proposed {}", entry.transaction().hash());
        self.proposed.add_entry(entry)
    }

    /// Returns true if the tx-pool contains a tx with specified id.
    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id) || self.gap.contains_key(id) || self.proposed.contains_key(id)
    }

    /// Returns tx with cycles corresponding to the id.
    pub fn get_tx_with_cycles(&self, id: &ProposalShortId) -> Option<(TransactionView, Cycle)> {
        self.pending
            .get(id)
            .map(|entry| (entry.transaction().clone(), entry.cycles))
            .or_else(|| {
                self.gap
                    .get(id)
                    .map(|entry| (entry.transaction().clone(), entry.cycles))
            })
            .or_else(|| {
                self.proposed
                    .get(id)
                    .map(|entry| (entry.transaction().clone(), entry.cycles))
            })
    }

    /// Returns tx corresponding to the id.
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
    }

    /// Returns tx from pending and gap corresponding to the id. RPC
    pub fn get_entry_from_pending_or_gap(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.pending.get(id).or_else(|| self.gap.get(id))
    }

    pub(crate) fn proposed(&self) -> &ProposedPool {
        &self.proposed
    }

    pub(crate) fn get_tx_from_proposed_and_others(
        &self,
        id: &ProposalShortId,
    ) -> Option<&TransactionView> {
        self.proposed
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.pending.get_tx(id))
    }

    pub(crate) fn remove_committed_txs<'a>(
        &mut self,
        txs: impl Iterator<Item = &'a TransactionView>,
        callbacks: &Callbacks,
        detached_headers: &HashSet<Byte32>,
    ) {
        for tx in txs {
            let tx_hash = tx.hash();
            debug!("try remove_committed_tx {}", tx_hash);
            self.remove_committed_tx(tx, callbacks);

            self.committed_txs_hash_cache
                .put(tx.proposal_short_id(), tx_hash);
        }

        if !detached_headers.is_empty() {
            self.resolve_conflict_header_dep(detached_headers, callbacks)
        }
    }

    pub(crate) fn resolve_conflict_header_dep(
        &mut self,
        detached_headers: &HashSet<Byte32>,
        callbacks: &Callbacks,
    ) {
        for (entry, reject) in self.proposed.resolve_conflict_header_dep(detached_headers) {
            callbacks.call_reject(self, &entry, reject);
        }
        for (entry, reject) in self.gap.resolve_conflict_header_dep(detached_headers) {
            callbacks.call_reject(self, &entry, reject);
        }
        for (entry, reject) in self.pending.resolve_conflict_header_dep(detached_headers) {
            callbacks.call_reject(self, &entry, reject);
        }
    }

    pub(crate) fn remove_committed_tx(&mut self, tx: &TransactionView, callbacks: &Callbacks) {
        let hash = tx.hash();
        let short_id = tx.proposal_short_id();
        // try remove committed tx from proposed
        // proposed tx should not contain conflict, if exists just skip resolve conflict
        if let Some(entry) = self.proposed.remove_committed_tx(tx) {
            debug!("remove_committed_tx from proposed {}", hash);
            callbacks.call_committed(self, &entry)
        } else {
            let conflicts = self.proposed.resolve_conflict(tx);

            for (entry, reject) in conflicts {
                callbacks.call_reject(self, &entry, reject);
            }
        }

        // pending and gap should resolve conflict no matter exists or not
        if let Some(entry) = self.gap.remove_entry(&short_id) {
            debug!("remove_committed_tx from gap {}", hash);
            callbacks.call_committed(self, &entry)
        }
        {
            let conflicts = self.gap.resolve_conflict(tx);

            for (entry, reject) in conflicts {
                callbacks.call_reject(self, &entry, reject);
            }
        }

        if let Some(entry) = self.pending.remove_entry(&short_id) {
            debug!("remove_committed_tx from pending {}", hash);
            callbacks.call_committed(self, &entry)
        }
        {
            let conflicts = self.pending.resolve_conflict(tx);

            for (entry, reject) in conflicts {
                callbacks.call_reject(self, &entry, reject);
            }
        }
    }

    //  Expire all transaction (and their dependencies) in the pool.
    pub(crate) fn remove_expired(&mut self, callbacks: &Callbacks) {
        let now_ms = ckb_systemtime::unix_time_as_millis();
        let expired =
            |_id: &ProposalShortId, tx_entry: &TxEntry| self.expiry + tx_entry.timestamp < now_ms;
        let mut removed = self.pending.remove_entries_by_filter(expired);
        removed.extend(self.gap.remove_entries_by_filter(expired));
        let removed_proposed_ids: Vec<_> = self
            .proposed
            .iter()
            .filter_map(|(id, tx_entry)| {
                if self.expiry + tx_entry.timestamp < now_ms {
                    Some(id)
                } else {
                    None
                }
            })
            .cloned()
            .collect();
        for id in removed_proposed_ids {
            removed.extend(self.proposed.remove_entry_and_descendants(&id))
        }

        for entry in removed {
            let tx_hash = entry.transaction().hash();
            debug!("remove_expired {} timestamp({})", tx_hash, entry.timestamp);
            let reject = Reject::Expiry(entry.timestamp);
            callbacks.call_reject(self, &entry, reject);
        }
    }

    // Remove transactions from the pool until total size < size_limit.
    pub(crate) fn limit_size(&mut self, callbacks: &Callbacks) {
        while self.total_tx_size > self.config.max_tx_pool_size {
            if !self.pending.is_empty() {
                evict_for_trim_size!(self, self.pending, callbacks)
            } else if !self.gap.is_empty() {
                evict_for_trim_size!(self, self.gap, callbacks)
            } else {
                evict_for_trim_size!(self, self.proposed, callbacks)
            }
        }
    }

    // remove transaction with detached proposal from gap and proposed
    // try re-put to pending
    pub(crate) fn remove_by_detached_proposal<'a>(
        &mut self,
        ids: impl Iterator<Item = &'a ProposalShortId>,
    ) {
        for id in ids {
            if let Some(entry) = self.gap.remove_entry(id) {
                let tx_hash = entry.transaction().hash();
                let ret = self.add_pending(entry);
                debug!(
                    "remove_by_detached_proposal from gap {} add_pending {}",
                    tx_hash, ret
                );
            }
            let mut entries = self.proposed.remove_entry_and_descendants(id);
            entries.sort_unstable_by_key(|entry| entry.ancestors_count);
            for mut entry in entries {
                let tx_hash = entry.transaction().hash();
                entry.reset_ancestors_state();
                let ret = self.add_pending(entry);
                debug!(
                    "remove_by_detached_proposal from proposed {} add_pending {}",
                    tx_hash, ret
                );
            }
        }
    }

    pub(crate) fn remove_tx(&mut self, id: &ProposalShortId) -> bool {
        let entries = self.proposed.remove_entry_and_descendants(id);
        if !entries.is_empty() {
            for entry in entries {
                self.update_statics_for_remove_tx(entry.size, entry.cycles);
            }
            return true;
        }

        if let Some(entry) = self.gap.remove_entry(id) {
            self.update_statics_for_remove_tx(entry.size, entry.cycles);
            return true;
        }

        if let Some(entry) = self.pending.remove_entry(id) {
            self.update_statics_for_remove_tx(entry.size, entry.cycles);
            return true;
        }

        false
    }

    pub(crate) fn resolve_tx_from_pending_and_proposed(
        &self,
        tx: TransactionView,
    ) -> Result<Arc<ResolvedTransaction>, Reject> {
        let snapshot = self.snapshot();
        let proposed_provider = OverlayCellProvider::new(&self.proposed, snapshot);
        let gap_and_proposed_provider = OverlayCellProvider::new(&self.gap, &proposed_provider);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&self.pending, &gap_and_proposed_provider);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(
            tx,
            &mut seen_inputs,
            &pending_and_proposed_provider,
            snapshot,
        )
        .map(Arc::new)
        .map_err(Reject::Resolve)
    }

    pub(crate) fn check_rtx_from_pending_and_proposed(
        &self,
        rtx: &ResolvedTransaction,
    ) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let proposed_checker = OverlayCellChecker::new(&self.proposed, snapshot);
        let gap_and_proposed_checker = OverlayCellChecker::new(&self.gap, &proposed_checker);
        let pending_and_proposed_checker =
            OverlayCellChecker::new(&self.pending, &gap_and_proposed_checker);
        let mut seen_inputs = HashSet::new();
        rtx.check(&mut seen_inputs, &pending_and_proposed_checker, snapshot)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn resolve_tx_from_proposed(
        &self,
        tx: TransactionView,
    ) -> Result<Arc<ResolvedTransaction>, Reject> {
        let snapshot = self.snapshot();
        let cell_provider = OverlayCellProvider::new(&self.proposed, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(tx, &mut seen_inputs, &cell_provider, snapshot)
            .map(Arc::new)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn check_rtx_from_proposed(&self, rtx: &ResolvedTransaction) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let cell_checker = OverlayCellChecker::new(&self.proposed, snapshot);
        let mut seen_inputs = HashSet::new();
        rtx.check(&mut seen_inputs, &cell_checker, snapshot)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn gap_rtx(
        &mut self,
        cache_entry: CacheEntry,
        size: usize,
        timestamp: u64,
        rtx: Arc<ResolvedTransaction>,
    ) -> Result<CacheEntry, Reject> {
        let snapshot = self.cloned_snapshot();
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(TxVerifyEnv::new_proposed(tip_header, 0));
        self.check_rtx_from_pending_and_proposed(&rtx)?;

        let max_cycles = snapshot.consensus().max_block_cycles();
        let verified = verify_rtx(
            snapshot,
            Arc::clone(&rtx),
            tx_env,
            &Some(cache_entry),
            max_cycles,
        )?;

        let entry =
            TxEntry::new_with_timestamp(rtx, verified.cycles, verified.fee, size, timestamp);
        let tx_hash = entry.transaction().hash();
        if self.add_gap(entry) {
            Ok(CacheEntry::Completed(verified))
        } else {
            Err(Reject::Duplicated(tx_hash))
        }
    }

    pub(crate) fn proposed_rtx(
        &mut self,
        cache_entry: CacheEntry,
        size: usize,
        timestamp: u64,
        rtx: Arc<ResolvedTransaction>,
    ) -> Result<CacheEntry, Reject> {
        let snapshot = self.cloned_snapshot();
        let tip_header = snapshot.tip_header();
        let tx_env = Arc::new(TxVerifyEnv::new_proposed(tip_header, 1));
        self.check_rtx_from_proposed(&rtx)?;

        let max_cycles = snapshot.consensus().max_block_cycles();
        let verified = verify_rtx(
            snapshot,
            Arc::clone(&rtx),
            tx_env,
            &Some(cache_entry),
            max_cycles,
        )?;

        let entry =
            TxEntry::new_with_timestamp(rtx, verified.cycles, verified.fee, size, timestamp);
        let tx_hash = entry.transaction().hash();
        if self.add_proposed(entry)? {
            Ok(CacheEntry::Completed(verified))
        } else {
            Err(Reject::Duplicated(tx_hash))
        }
    }

    /// Get to-be-proposal transactions that may be included in the next block.
    pub fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        let mut proposals = HashSet::with_capacity(limit);
        self.pending
            .fill_proposals(limit, exclusion, &mut proposals);
        self.gap.fill_proposals(limit, exclusion, &mut proposals);
        proposals
    }

    /// Returns tx from tx-pool or storage corresponding to the id.
    pub fn get_tx_from_pool_or_store(
        &self,
        proposal_id: &ProposalShortId,
    ) -> Option<TransactionView> {
        self.get_tx_from_proposed_and_others(proposal_id)
            .cloned()
            .or_else(|| {
                self.committed_txs_hash_cache
                    .peek(proposal_id)
                    .and_then(|tx_hash| self.snapshot().get_transaction(tx_hash).map(|(tx, _)| tx))
            })
    }

    pub(crate) fn get_ids(&self) -> TxPoolIds {
        let pending = self
            .pending
            .iter()
            .map(|(_, entry)| entry.transaction().hash())
            .chain(self.gap.iter().map(|(_, entry)| entry.transaction().hash()))
            .collect();

        let proposed = self
            .proposed
            .iter()
            .map(|(_, entry)| entry.transaction().hash())
            .collect();

        TxPoolIds { pending, proposed }
    }

    pub(crate) fn get_all_entry_info(&self) -> TxPoolEntryInfo {
        let pending = self
            .pending
            .iter()
            .map(|(_, entry)| (entry.transaction().hash(), entry.to_info()))
            .chain(
                self.gap
                    .iter()
                    .map(|(_, entry)| (entry.transaction().hash(), entry.to_info())),
            )
            .collect();

        let proposed = self
            .proposed
            .iter()
            .map(|(_, entry)| (entry.transaction().hash(), entry.to_info()))
            .collect();

        TxPoolEntryInfo { pending, proposed }
    }

    pub(crate) fn drain_all_transactions(&mut self) -> Vec<TransactionView> {
        let mut txs = CommitTxsScanner::new(&self.proposed)
            .txs_to_commit(self.total_tx_size, self.total_tx_cycles)
            .0
            .into_iter()
            .map(|tx_entry| tx_entry.into_transaction())
            .collect::<Vec<_>>();
        self.proposed.clear();
        txs.append(&mut self.gap.drain());
        txs.append(&mut self.pending.drain());
        self.total_tx_size = 0;
        self.total_tx_cycles = 0;
        // self.touch_last_txs_updated_at();
        txs
    }

    pub(crate) fn clear(&mut self, snapshot: Arc<Snapshot>) {
        self.pending = PendingQueue::new();
        self.gap = PendingQueue::new();
        self.proposed = ProposedPool::new(self.config.max_ancestors_count);
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
            CommitTxsScanner::new(self.proposed()).txs_to_commit(txs_size_limit, max_block_cycles);

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
}

fn build_recent_reject(config: &TxPoolConfig) -> Option<RecentReject> {
    if !config.recent_reject.as_os_str().is_empty() {
        let recent_reject_ttl = config.keep_rejected_tx_hashes_days as i32 * 24 * 60 * 60;
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
