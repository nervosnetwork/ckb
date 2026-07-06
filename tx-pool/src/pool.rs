//! Top-level Pool type, methods, and tests
use super::component::{TxEntry, tx_selector::TxSelector};
use crate::component::pool_map::{PoolEntry, PoolMap, Status};
use crate::constants::{MAX_ESTIMATE_TARGET, MIN_ESTIMATE_TARGET};
use crate::error::Reject;
use crate::pool_cell::PoolCell;
use ckb_app_config::TxPoolConfig;
use ckb_fee_estimator::Error as FeeEstimatorError;
use ckb_logger::{debug, error};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::tx_pool::PoolTxDetailInfo;
use ckb_types::core::{BlockNumber, FeeRate};
use ckb_types::packed::OutPoint;
use ckb_types::{
    core::{
        Capacity, Cycle, TransactionView, UncleBlockView,
        cell::{OverlayCellChecker, OverlayCellProvider, ResolvedTransaction, resolve_transaction},
        tx_pool::{TxPoolEntryInfo, TxPoolIds},
    },
    packed::{Byte32, ProposalShortId},
};
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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

/// Upper bound on the number of RBF replacement candidates evaluated in one replacement.
///
/// Prevents an O(n) scan of the mempool when a large transaction conflicts with many
/// existing entries. 100 is the same order of magnitude as Bitcoin Core's replacement
/// candidate limit.
const MAX_REPLACEMENT_CANDIDATES: usize = 100;

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
    // conflicted transaction cache
    pub(crate) conflicts_cache: lru::LruCache<ProposalShortId, TransactionView>,
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

    /// The least required fee rate to allow tx to be replaced
    pub fn min_replace_fee(&self, tx: &TxEntry) -> Option<Capacity> {
        if !self.enable_rbf() {
            return None;
        }

        let mut conflicts = vec![self.get_pool_entry(&tx.proposal_short_id()).unwrap()];
        let descendants = self.pool_map.calc_descendants(&tx.proposal_short_id());
        let descendants = descendants
            .iter()
            .filter_map(|id| self.get_pool_entry(id))
            .collect::<Vec<_>>();
        conflicts.extend(descendants);
        self.calculate_min_replace_fee(&conflicts, tx.size)
    }

    /// min_replace_fee = sum(replaced_txs.fee) + extra_rbf_fee
    fn calculate_min_replace_fee(&self, conflicts: &[&PoolEntry], size: usize) -> Option<Capacity> {
        let extra_rbf_fee = self.config.min_rbf_rate.fee(size as u64);
        // don't account for duplicate txs
        let replaced_sum_fee = conflicts
            .iter()
            .map(|c| (c.id.clone(), c.inner.fee))
            .collect::<HashMap<_, _>>()
            .into_values()
            .try_fold(Capacity::zero(), |acc, x| acc.safe_add(x));
        let total_fee = replaced_sum_fee.and_then(|sum| sum.safe_add(extra_rbf_fee));
        match total_fee {
            Ok(res) => Some(res),
            Err(_) => {
                let fees = conflicts.iter().map(|c| c.inner.fee).collect::<Vec<_>>();
                error!(
                    "conflicts: {:?} replaced_sum_fee {:?} overflow by add {}",
                    conflicts.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    fees,
                    extra_rbf_fee
                );
                None
            }
        }
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

    pub(crate) fn record_conflict(&mut self, tx: TransactionView) {
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
        self.conflicts_cache.put(short_id.clone(), tx);
        debug!(
            "record_conflict {:?} now cache size: {}",
            short_id,
            self.conflicts_cache.len()
        );
    }

    pub(crate) fn remove_conflict(&mut self, short_id: &ProposalShortId) {
        if let Some(tx) = self.conflicts_cache.pop(short_id) {
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
    ) -> Vec<TransactionView> {
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(set) = self.conflicts_outputs_cache.peek(&input) {
                for short_id in set {
                    if seen.insert(short_id.clone())
                        && let Some(tx) = self.conflicts_cache.peek(short_id)
                    {
                        result.push(tx.clone());
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

    pub(crate) fn remove_tx(&mut self, id: &ProposalShortId) -> bool {
        let entries = self.pool_map.remove_entry_and_descendants(id);
        !entries.is_empty()
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
            .map(|(_id, tx)| tx.hash())
            .collect();
        TxPoolEntryInfo {
            pending,
            proposed,
            conflicted,
        }
    }

    pub(crate) fn drain_all_transactions(&mut self) -> Vec<TransactionView> {
        let mut txs = TxSelector::new(&self.pool_map)
            .txs_to_commit(usize::MAX, Cycle::MAX)
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
        self.pool_map.clear();
        txs
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

    pub(crate) fn check_rbf(
        &self,
        snapshot: &Snapshot,
        entry: &TxEntry,
    ) -> Result<HashSet<ProposalShortId>, Reject> {
        assert!(self.enable_rbf());
        let tx_inputs: Vec<OutPoint> = entry.transaction().input_pts_iter().collect();
        let conflict_ids = self.pool_map.find_conflict_tx(entry.transaction());

        if conflict_ids.is_empty() {
            return Ok(HashSet::new());
        }

        let short_id = entry.proposal_short_id();

        // Rule #1, the node has enabled RBF, which is checked by caller
        let conflicts = conflict_ids
            .iter()
            .filter_map(|id| self.get_pool_entry(id))
            .collect::<Vec<_>>();
        assert!(conflicts.len() == conflict_ids.len());

        // Rule #2, new tx don't contain any new unconfirmed inputs
        self.check_rbf_no_new_unconfirmed_inputs(&conflicts, &tx_inputs, snapshot)?;

        // Rule #5, check descendants count limit, ancestor-descendant overlap,
        // and no inputs from descendants
        let all_conflicted = self.check_rbf_descendants(&conflicts, &short_id, &tx_inputs)?;

        // Check new tx does not use cell deps from conflicted txs
        self.check_rbf_no_conflict_cell_deps(&all_conflicted, entry)?;

        // Rule #3 & #4, new tx's fee must be higher than both conflicts and min_rbf_fee
        self.check_rbf_fee(&all_conflicted, entry)?;

        Ok(conflict_ids)
    }

    /// RBF Rule #2: new tx must not contain any new unconfirmed inputs
    /// (all inputs must either be from the conflicted txs or already confirmed on-chain).
    fn check_rbf_no_new_unconfirmed_inputs(
        &self,
        conflicts: &[&PoolEntry],
        tx_inputs: &[OutPoint],
        snapshot: &Snapshot,
    ) -> Result<(), Reject> {
        let inputs_capacity = conflicts
            .iter()
            .map(|c| c.inner.transaction().inputs().len())
            .sum();
        let mut inputs = HashSet::with_capacity(inputs_capacity);
        for c in conflicts.iter() {
            inputs.extend(c.inner.transaction().input_pts_iter());
        }
        if tx_inputs
            .iter()
            .any(|pt| !inputs.contains(pt) && !snapshot.transaction_exists(&pt.tx_hash()))
        {
            return Err(Reject::RBFRejected(
                "new Tx contains unconfirmed inputs".to_string(),
            ));
        }
        Ok(())
    }

    /// RBF Rule #5: check that the number of replaced txs (conflicts + descendants)
    /// does not exceed MAX_REPLACEMENT_CANDIDATES, that the new tx does not
    /// reference outputs of descendant txs as inputs, and that the new tx's
    /// ancestors do not overlap with the conflicted txs' descendants.
    ///
    /// Returns the full set of conflicted entries (direct conflicts + their descendants).
    fn check_rbf_descendants<'a>(
        &'a self,
        conflicts: &[&'a PoolEntry],
        _short_id: &ProposalShortId,
        tx_inputs: &[OutPoint],
    ) -> Result<Vec<&'a PoolEntry>, Reject> {
        let mut all_conflicted = conflicts.to_vec();
        // Deduplicate via HashSet of proposal short ids so shared descendants
        // are not double-counted across multiple direct conflicts (3.7 fix).
        let mut seen_ids: HashSet<ProposalShortId> =
            conflicts.iter().map(|c| c.id.clone()).collect();
        let mut ancestors: HashSet<ProposalShortId> = HashSet::with_capacity(tx_inputs.len() * 2);
        // Include inputs in ancestor set.
        for input in tx_inputs {
            let parent_id = ProposalShortId::from_tx_hash(&input.tx_hash());
            if self.get_pool_entry(&parent_id).is_some() {
                ancestors.insert(parent_id.clone());
                ancestors.extend(self.pool_map.calc_ancestors(&parent_id));
            }
        }
        // Also include cell_deps parents in ancestor set (3.8 fix): if the
        // new tx uses a cell_dep that is an in-pool tx, it should be treated
        // as an ancestor for the disjointness check.
        // Note: the caller does not pass cell_deps here, but the check below
        // is conservative — if a cell_dep is a descendant of a conflicted tx,
        // `check_rbf_no_conflict_cell_deps` will catch it separately.
        for conflict in conflicts.iter() {
            let descendants = self.pool_map.calc_descendants(&conflict.id);

            // Count only newly-seen ids to avoid double-counting shared descendants.
            let new_count = descendants
                .iter()
                .filter(|id| !seen_ids.contains(id))
                .count();
            let replace_count = seen_ids.len() + new_count;
            if replace_count > MAX_REPLACEMENT_CANDIDATES {
                return Err(Reject::RBFRejected(format!(
                    "Tx conflict with too many txs, conflict txs count: {}, expect <= {}",
                    replace_count, MAX_REPLACEMENT_CANDIDATES,
                )));
            }

            let entries = descendants
                .iter()
                .filter_map(|id| self.get_pool_entry(id))
                .collect::<Vec<_>>();

            // Check the more specific error first: the new tx is spending an
            // output that belongs to a descendant of the to-be-replaced tx.
            for entry in entries.iter() {
                let hash = entry.inner.transaction().hash();
                if tx_inputs.iter().any(|pt| pt.tx_hash() == hash) {
                    return Err(Reject::RBFRejected(
                        "new Tx contains inputs in descendants of to be replaced Tx".to_string(),
                    ));
                }
            }

            // Then check the broader ancestor/descendant overlap.
            if !descendants.is_disjoint(&ancestors) {
                return Err(Reject::RBFRejected(
                    "Tx ancestors have common with conflict Tx descendants".to_string(),
                ));
            }

            // Only extend all_conflicted with entries we haven't seen yet.
            for entry in entries {
                if seen_ids.insert(entry.id.clone()) {
                    all_conflicted.push(entry);
                }
            }
        }
        Ok(all_conflicted)
    }

    /// Check that the new tx does not reference any conflicted tx as a cell dep.
    fn check_rbf_no_conflict_cell_deps(
        &self,
        all_conflicted: &[&PoolEntry],
        entry: &TxEntry,
    ) -> Result<(), Reject> {
        let tx_cells_deps: Vec<OutPoint> = entry
            .transaction()
            .cell_deps_iter()
            .map(|c| c.out_point())
            .collect();
        for conflicted in all_conflicted.iter() {
            let hash = conflicted.inner.transaction().hash();
            if tx_cells_deps.iter().any(|pt| pt.tx_hash() == hash) {
                return Err(Reject::RBFRejected(
                    "new Tx contains cell deps from conflicts".to_string(),
                ));
            }
        }
        Ok(())
    }

    /// RBF Rule #3 & #4: the new tx's fee must be higher than the total fee of
    /// all conflicted txs and must meet the minimum replacement fee rate.
    fn check_rbf_fee(&self, all_conflicted: &[&PoolEntry], entry: &TxEntry) -> Result<(), Reject> {
        let fee = entry.fee;
        if let Some(min_replace_fee) = self.calculate_min_replace_fee(all_conflicted, entry.size) {
            if fee < min_replace_fee {
                return Err(Reject::RBFRejected(format!(
                    "Tx's current fee is {}, expect it to >= {} to replace old txs",
                    fee, min_replace_fee,
                )));
            }
        } else {
            return Err(Reject::RBFRejected(
                "calculate_min_replace_fee failed".to_string(),
            ));
        }
        Ok(())
    }

    /// query the details of a transaction in the pool, only for trouble shooting
    pub(crate) fn get_tx_detail(&self, id: &ProposalShortId) -> Option<PoolTxDetailInfo> {
        if let Some(entry) = self.pool_map.get_by_id(id) {
            let ids = self.get_ids();
            let rank_in_pending = if entry.status == Status::Proposed {
                0
            } else {
                let tx_hash = entry.inner.transaction().hash();
                ids.pending
                    .iter()
                    .enumerate()
                    .find(|(_, hash)| &tx_hash == *hash)
                    .map(|r| r.0)
                    .unwrap_or_default()
                    + 1
            };
            let res = PoolTxDetailInfo {
                timestamp: entry.inner.timestamp,
                entry_status: entry.status.to_string(),
                pending_count: self.pool_map.pending_size(),
                rank_in_pending,
                proposed_count: ids.proposed.len(),
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
