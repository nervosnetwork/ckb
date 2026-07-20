//! Top-level Pool type, methods, and tests
use super::component::{TxEntry, tx_selector::TxSelector};
use crate::component::pool_map::{PoolEntry, PoolMap, Status};
use crate::constants::{MAX_ESTIMATE_TARGET, MIN_ESTIMATE_TARGET};
use crate::error::Reject;
use crate::pool_cell::PoolCell;
use crate::tx_source::TxSource;
use ckb_app_config::TxPoolConfig;
use ckb_fee_estimator::Error as FeeEstimatorError;
use ckb_logger::debug;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::{BlockNumber, FeeRate};
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        Capacity, Cycle, TransactionView, UncleBlockView,
        cell::{OverlayCellChecker, OverlayCellProvider, ResolvedTransaction, resolve_transaction},
        tx_pool::{PoolTxDetailInfo, TxPoolEntryInfo, TxPoolIds},
    },
    packed::{Byte32, ProposalShortId},
};
use lru::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

pub(crate) mod rbf;

/// Cache size for recently committed transaction hashes.
///
/// Used to short-circuit re-submission of transactions that were already committed in a
/// recent block. 100k entries is enough to cover several hours of main-chain traffic
/// under normal load without consuming excessive memory.
const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;

/// Cache size for conflicted transactions that were evicted from the pool.
///
/// Keeps a limited history of evicted transactions so that we can return more descriptive
/// rejection reasons (e.g. which transaction replaced this one). 10k is a conservative
/// trade-off between diagnostic usefulness and memory usage.
const CONFLICTS_CACHE_SIZE: usize = 10_000;

/// Cache size for conflicted transaction inputs.
///
/// Mirrors the conflicted transaction cache but indexes by consumed out-point so that
/// a new transaction spending the same cell can be rejected with a precise message.
/// Sized at 3x `CONFLICTS_CACHE_SIZE` because a single transaction can consume many inputs.
const CONFLICTS_INPUTS_CACHE_SIZE: usize = 30_000;

fn reject_full_for_evicted(entry: &TxEntry) -> Reject {
    Reject::Full(format!(
        "the fee_rate for this transaction is: {}",
        entry.fee_rate()
    ))
}

/// Tx-pool implementation
pub struct TxPool {
    pub(crate) config: TxPoolConfig,
    pub(crate) pool_map: PoolMap,
    /// cache for committed transactions hash
    pub(crate) committed_txs_hash_cache: LruCache<ProposalShortId, Byte32>,
    /// storage snapshot reference
    pub(crate) snapshot: Arc<Snapshot>,
    // expiration milliseconds,
    pub(crate) expiry: u64,
    // conflicted transaction cache; stores the original pipeline source so that
    // recovered transactions keep their origin (peer/cycles) where known
    pub(crate) conflicts_cache: lru::LruCache<ProposalShortId, (TransactionView, TxSource)>,
    // conflicted transaction outputs cache, input -> set of conflicting tx ids.
    // A set is necessary because multiple txs can conflict on the same input;
    // a single-value cache would silently drop older conflicts and break
    // recovery (see RbfOrphanRecovery).
    pub(crate) conflicts_outputs_cache: lru::LruCache<OutPoint, HashSet<ProposalShortId>>,
}

impl TxPool {
    /// Create new TxPool
    pub fn new(config: TxPoolConfig, snapshot: Arc<Snapshot>) -> TxPool {
        let expiry = config.expiry_hours as u64 * 60 * 60 * 1000;
        TxPool {
            pool_map: PoolMap::new(config.max_ancestors_count),
            committed_txs_hash_cache: LruCache::new(COMMITTED_HASH_CACHE_SIZE),
            config,
            snapshot,
            expiry,
            conflicts_cache: LruCache::new(CONFLICTS_CACHE_SIZE),
            conflicts_outputs_cache: lru::LruCache::new(CONFLICTS_INPUTS_CACHE_SIZE),
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

    /// Check whether tx-pool enable RBF
    pub fn enable_rbf(&self) -> bool {
        self.config.min_rbf_rate > self.config.min_fee_rate
    }

    /// The least required fee for a replacement transaction to be accepted.
    ///
    /// The incremental RBF fee is computed against the replacement transaction's
    /// serialized `size`, not its weight. This matches how `min_fee_rate` is
    /// enforced at submission time (see [`check_tx_fee_with_min_fee_rate`]),
    /// because cycles are not reliably available when a tx is first submitted.
    /// Using size here keeps the replacement threshold consistent with the pool's
    /// normal fee policy.
    pub fn min_replace_fee(&self, tx: &TxEntry) -> Option<Capacity> {
        if !self.enable_rbf() {
            return None;
        }

        let entry = self.get_pool_entry(&tx.proposal_short_id())?;
        let mut conflicts = vec![entry];
        let descendants = self.pool_map.calc_descendants(&tx.proposal_short_id());
        let descendants = descendants
            .iter()
            .filter_map(|id| self.get_pool_entry(id))
            .collect::<Vec<_>>();
        conflicts.extend(descendants);
        self.calculate_min_replace_fee(&conflicts, tx.size as u64)
    }

    /// Add tx with pending status
    /// If did have this value present, false is returned.
    pub(crate) fn add_pending(
        &mut self,
        entry: TxEntry,
    ) -> Result<(bool, HashSet<TxEntry>), Reject> {
        self.pool_map.add_entry(entry, Status::Pending)
    }

    /// Add tx which proposed but still uncommittable to gap
    pub(crate) fn add_gap(&mut self, entry: TxEntry) -> Result<(bool, HashSet<TxEntry>), Reject> {
        self.pool_map.add_entry(entry, Status::Gap)
    }

    /// Add tx with proposed status
    pub(crate) fn add_proposed(
        &mut self,
        entry: TxEntry,
    ) -> Result<(bool, HashSet<TxEntry>), Reject> {
        self.pool_map.add_entry(entry, Status::Proposed)
    }

    /// Returns true if the tx-pool contains a tx with specified id.
    pub(crate) fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pool_map.get_by_id(id).is_some()
    }

    pub(crate) fn record_conflict(&mut self, tx: TransactionView, source: TxSource) {
        let short_id = tx.proposal_short_id();
        for input in tx.input_pts_iter() {
            if let Some(set) = self.conflicts_outputs_cache.get_mut(&input) {
                set.insert(short_id.clone());
            } else {
                let mut set = HashSet::with_capacity(1);
                set.insert(short_id.clone());
                self.conflicts_outputs_cache.put(input, set);
            }
        }
        self.conflicts_cache.put(short_id.clone(), (tx, source));
        debug!(
            "record_conflict {:?} now cache size: {}",
            short_id,
            self.conflicts_cache.len()
        );
    }

    pub(crate) fn remove_conflict(&mut self, short_id: &ProposalShortId) {
        if let Some((tx, _)) = self.conflicts_cache.pop(short_id) {
            for input in tx.input_pts_iter() {
                if let Some(set) = self.conflicts_outputs_cache.get_mut(&input) {
                    set.remove(short_id);
                    if set.is_empty() {
                        self.conflicts_outputs_cache.pop(&input);
                    }
                }
            }
        }
        debug!(
            "remove_conflict {:?} now cache size: {}",
            short_id,
            self.conflicts_cache.len()
        );
    }

    pub(crate) fn get_conflicted_txs_from_inputs(
        &self,
        inputs: impl Iterator<Item = OutPoint>,
    ) -> Vec<(TransactionView, TxSource)> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(set) = self.conflicts_outputs_cache.peek(&input) {
                for short_id in set {
                    if seen.insert(short_id.clone())
                        && let Some((tx, source)) = self.conflicts_cache.peek(short_id)
                        // Only recover a tx if *all* of its inputs are currently
                        // available.  A tx that still conflicts with the in-pool
                        // state would be rejected again and, if both conflicting
                        // txs are in the conflicts cache, can trigger an
                        // infinite recover/reject loop (RBF cycling).
                        && self.pool_map.find_conflict_outpoint(tx).is_none()
                    {
                        result.push((tx.clone(), *source));
                    }
                }
            }
        }
        result
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
        detached_headers: &HashSet<Byte32>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        for tx in txs {
            let tx_hash = tx.hash();
            debug!("try remove_committed_tx {}", tx_hash);
            self.remove_committed_tx(tx, reject_events);

            self.committed_txs_hash_cache
                .put(tx.proposal_short_id(), tx_hash);
        }

        if !detached_headers.is_empty() {
            self.resolve_conflict_header_dep(detached_headers, reject_events)
        }
    }

    fn resolve_conflict_header_dep(
        &mut self,
        detached_headers: &HashSet<Byte32>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        for (entry, reject) in self.pool_map.resolve_conflict_header_dep(detached_headers) {
            reject_events.push((entry, reject));
        }
    }

    fn remove_committed_tx(
        &mut self,
        tx: &TransactionView,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) {
        let short_id = tx.proposal_short_id();
        if let Some(_entry) = self.pool_map.remove_entry(&short_id) {
            debug!("remove_committed_tx for {}", tx.hash());
        }
        {
            for (entry, reject) in self.pool_map.resolve_conflict(tx) {
                debug!(
                    "removed {} for committed: {}",
                    entry.transaction().hash(),
                    tx.hash()
                );
                reject_events.push((entry, reject));
            }
        }
    }

    // Expire all transaction (and their dependencies) in the pool.
    pub(crate) fn remove_expired(&mut self, reject_events: &mut Vec<(TxEntry, Reject)>) {
        let now_ms = ckb_systemtime::unix_time_as_millis();

        let removed: Vec<_> = self
            .pool_map
            .iter()
            .filter(|&entry| self.expiry + entry.inner.timestamp < now_ms)
            .map(|entry| entry.inner.clone())
            .collect();

        for entry in removed {
            let tx_hash = entry.transaction().hash();
            debug!("remove_expired {} timestamp({})", tx_hash, entry.timestamp);
            self.pool_map.remove_entry(&entry.proposal_short_id());
            let reject = Reject::Expiry(entry.timestamp);
            reject_events.push((entry, reject));
        }
    }

    // Remove transactions from the pool until total size <= size_limit.
    // Return a `Reject` for current inserting entry if it's removed
    pub(crate) fn limit_size(
        &mut self,
        current_entry_id: Option<&ProposalShortId>,
        reject_events: &mut Vec<(TxEntry, Reject)>,
    ) -> Option<Reject> {
        let mut ret = None;
        while self.pool_map.stats.total_tx_size.get() > self.config.max_tx_pool_size {
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
                        "Removed by size limit {} timestamp({})",
                        tx_hash, entry.timestamp
                    );
                    let reject = reject_full_for_evicted(&entry);
                    if let Some(short_id) = current_entry_id
                        && entry.proposal_short_id() == *short_id
                    {
                        ret = Some(reject.clone());
                    }
                    reject_events.push((entry, reject));
                }
            }
        }
        ret
    }

    // remove transaction with detached proposal from gap and proposed
    // try re-put to pending
    pub(crate) fn remove_by_detached_proposal<'a>(
        &mut self,
        ids: impl Iterator<Item = &'a ProposalShortId>,
        callbacks: &crate::callback::Callbacks,
        reject_events: &mut Vec<(TxEntry, Reject)>,
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
                    let ret = self.add_pending(entry.clone());
                    match ret {
                        Ok((true, ref evicted)) => {
                            callbacks.call_pending(&entry);
                            for evict in evicted {
                                let reject = reject_full_for_evicted(evict);
                                reject_events.push((evict.clone(), reject));
                            }
                        }
                        Ok((false, _)) => {} // duplicate
                        Err(ref reject) => {
                            reject_events.push((entry.clone(), reject.clone()));
                        }
                    }
                    debug!(
                        "remove_by_detached_proposal from {:?} {} add_pending {:?}",
                        status, tx_hash, ret
                    );
                }
            }
        }
    }

    pub(crate) fn remove_tx(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        self.pool_map.remove_entry_and_descendants(id)
    }

    pub(crate) fn check_rtx_from_pool(&self, rtx: &ResolvedTransaction) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let pool_cell = PoolCell::new(&self.pool_map, false);
        let checker = OverlayCellChecker::new(&pool_cell, snapshot);
        let mut seen_inputs = HashSet::new();
        rtx.check(&mut seen_inputs, &checker, snapshot)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn resolve_tx_from_pool(
        &self,
        tx: TransactionView,
        rbf: bool,
    ) -> Result<Arc<ResolvedTransaction>, Reject> {
        let snapshot = self.snapshot();
        let pool_cell = PoolCell::new(&self.pool_map, rbf);
        let provider = OverlayCellProvider::new(&pool_cell, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(tx, &mut seen_inputs, &provider, snapshot)
            .map(Arc::new)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn transition_to_status(
        &mut self,
        short_id: &ProposalShortId,
        target: Status,
    ) -> Result<(), Reject> {
        match self.get_pool_entry(short_id) {
            Some(entry) => {
                let tx_hash = entry.inner.transaction().hash();
                if entry.status == target {
                    Err(Reject::Duplicated(tx_hash))
                } else {
                    debug!("transition_to_status: {:?} => {:?}", tx_hash, short_id);
                    self.pool_map.set_entry(short_id, target);
                    Ok(())
                }
            }
            None => Err(Reject::Malformed(
                String::from("invalid short_id"),
                Default::default(),
            )),
        }
    }

    pub(crate) fn gap_rtx(&mut self, short_id: &ProposalShortId) -> Result<(), Reject> {
        self.transition_to_status(short_id, Status::Gap)
    }

    pub(crate) fn proposed_rtx(&mut self, short_id: &ProposalShortId) -> Result<(), Reject> {
        self.transition_to_status(short_id, Status::Proposed)
    }

    /// Get to-be-proposal transactions that may be included in the next block.
    pub(crate) fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        self.pool_map.get_proposals(limit, exclusion)
    }

    /// Returns tx from tx-pool or storage corresponding to the id.
    ///
    /// In addition to the in-pool transactions, this also consults a small cache
    /// of recently committed transaction hashes so that compact blocks can
    /// retrieve transactions that were just removed from the pool.  Replaced /
    /// conflicted transactions are intentionally *not* returned: they are no
    /// longer valid and must not be used to reconstruct a block.
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
        let pending = self
            .pool_map
            .pending_gap_entries()
            .map(|entry| entry.transaction().hash())
            .collect();

        let proposed = self
            .pool_map
            .proposed_entries()
            .map(|entry| entry.transaction().hash())
            .collect();

        TxPoolIds { pending, proposed }
    }

    pub(crate) fn get_all_entry_info(&self) -> TxPoolEntryInfo {
        let pending = self
            .pool_map
            .pending_gap_entries()
            .map(|entry| (entry.transaction().hash(), entry.to_info()))
            .collect();

        let proposed = self
            .pool_map
            .proposed_entries()
            .map(|entry| (entry.transaction().hash(), entry.to_info()))
            .collect();

        let conflicted = self
            .conflicts_cache
            .iter()
            .map(|(_id, (tx, _))| tx.hash())
            .collect();
        TxPoolEntryInfo {
            pending,
            proposed,
            conflicted,
        }
    }

    /// Collect all transactions in the pool without removing them.
    pub(crate) fn get_all_txs(&self) -> Vec<TransactionView> {
        self.pool_map
            .iter()
            .map(|entry| entry.inner.transaction().clone())
            .collect()
    }

    pub(crate) fn clear(&mut self, snapshot: Arc<Snapshot>) {
        self.pool_map.clear();
        self.snapshot = snapshot;
        self.committed_txs_hash_cache = LruCache::new(COMMITTED_HASH_CACHE_SIZE);
        self.conflicts_cache = LruCache::new(CONFLICTS_CACHE_SIZE);
        self.conflicts_outputs_cache = lru::LruCache::new(CONFLICTS_INPUTS_CACHE_SIZE);
    }

    pub(crate) fn package_proposals(
        &self,
        proposals_limit: u64,
        uncles: &[UncleBlockView],
    ) -> HashSet<ProposalShortId> {
        let uncle_proposals: HashSet<ProposalShortId> = uncles
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
            TxSelector::new(&self.pool_map).txs_to_commit(txs_size_limit, max_block_cycles);

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

    pub(crate) fn estimate_fee_rate(
        &self,
        target_to_be_committed: BlockNumber,
    ) -> Result<FeeRate, FeeEstimatorError> {
        if !(MIN_ESTIMATE_TARGET..=MAX_ESTIMATE_TARGET).contains(&target_to_be_committed) {
            return Err(FeeEstimatorError::NoProperFeeRate);
        }
        let closest = self.snapshot.consensus().tx_proposal_window().closest();
        let target_blocks = target_to_be_committed.saturating_sub(closest).max(1) as usize;
        let fee_rate = self.pool_map.estimate_fee_rate(
            target_blocks,
            self.snapshot.consensus().max_block_bytes() as usize,
            self.snapshot.consensus().max_block_cycles(),
            self.config.min_fee_rate,
        );
        Ok(fee_rate)
    }

    /// query the details of a transaction in the pool, only for trouble shooting
    pub(crate) fn get_tx_detail(&self, id: &ProposalShortId) -> Option<PoolTxDetailInfo> {
        if let Some(entry) = self.pool_map.get_by_id(id) {
            // Rank and counts come from the sorted index and pool stats
            // directly; building full id vectors via `get_ids()` would
            // allocate ~2x pool size for a single query.
            let rank_in_pending = if entry.status == Status::Proposed {
                0
            } else {
                self.pool_map
                    .pending_gap_entries()
                    .position(|pending| pending.proposal_short_id() == *id)
                    .unwrap_or_default()
                    + 1
            };
            let res = PoolTxDetailInfo {
                timestamp: entry.inner.timestamp,
                entry_status: entry.status.to_string(),
                pending_count: self.pool_map.pending_size(),
                rank_in_pending,
                proposed_count: self.pool_map.proposed_size(),
                descendants_count: self.pool_map.calc_descendants(id).len(),
                ancestors_count: self.pool_map.calc_ancestors(id).len(),
                score_sortkey: entry.inner.as_score_key().into(),
            };
            Some(res)
        } else {
            None
        }
    }
}
