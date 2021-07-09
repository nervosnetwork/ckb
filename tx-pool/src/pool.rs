//! Top-level Pool type, methods, and tests
use super::component::{commit_txs_scanner::CommitTxsScanner, TxEntry};
use crate::callback::Callbacks;
use crate::component::pending::PendingQueue;
use crate::component::proposed::ProposedPool;
use crate::error::Reject;
use crate::util::verify_rtx;
use ckb_app_config::TxPoolConfig;
use ckb_logger::{error, trace};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::BlockNumber;
use ckb_types::{
    core::{
        cell::{
            resolve_transaction_with_options, OverlayCellChecker, OverlayCellProvider,
            ResolveOptions, ResolvedTransaction,
        },
        tx_pool::{TxPoolEntryInfo, TxPoolIds},
        Cycle, TransactionView,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_verification::{cache::CacheEntry, TxVerifyEnv};
use faketime::unix_time_as_millis;
use lru::LruCache;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

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
    /// last txs updated timestamp, used by getblocktemplate
    pub(crate) last_txs_updated_at: Arc<AtomicU64>,
    // sum of all tx_pool tx's virtual sizes.
    pub(crate) total_tx_size: usize,
    // sum of all tx_pool tx's cycles.
    pub(crate) total_tx_cycles: Cycle,
    /// storage snapshot reference
    pub(crate) snapshot: Arc<Snapshot>,
}

/// Transaction pool information.
#[derive(Clone, Debug)]
pub struct TxPoolInfo {
    /// The associated chain tip block hash.
    ///
    /// Transaction pool is stateful. It manages the transactions which are valid to be commit
    /// after this block.
    pub tip_hash: Byte32,
    /// The block number of the block `tip_hash`.
    pub tip_number: BlockNumber,
    /// Count of transactions in the pending state.
    ///
    /// The pending transactions must be proposed in a new block first.
    pub pending_size: usize,
    /// Count of transactions in the proposed state.
    ///
    /// The proposed transactions are ready to be commit in the new block after the block
    /// `tip_hash`.
    pub proposed_size: usize,
    /// Count of orphan transactions.
    ///
    /// An orphan transaction has an input cell from the transaction which is neither in the chain
    /// nor in the transaction pool.
    pub orphan_size: usize,
    /// Total count of transactions in the pool of all the different kinds of states.
    pub total_tx_size: usize,
    /// Total consumed VM cycles of all the transactions in the pool.
    pub total_tx_cycles: Cycle,
    /// Last updated time. This is the Unix timestamp in milliseconds.
    pub last_txs_updated_at: u64,
}

impl TxPool {
    /// Create new TxPool
    pub fn new(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        last_txs_updated_at: Arc<AtomicU64>,
    ) -> TxPool {
        const COMMITTED_HASH_CACHE_SIZE: usize = 100_000;

        TxPool {
            config,
            pending: PendingQueue::new(),
            gap: PendingQueue::new(),
            proposed: ProposedPool::new(config.max_ancestors_count),
            committed_txs_hash_cache: LruCache::new(COMMITTED_HASH_CACHE_SIZE),
            last_txs_updated_at,
            total_tx_size: 0,
            total_tx_cycles: 0,
            snapshot,
        }
    }

    /// Tx-pool owned snapshot, it may not consistent with chain cause tx-pool update snapshot asynchronously
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Makes a clone of the Arc<Snapshot>
    pub fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    /// Tx-pool information
    pub fn info(&self) -> TxPoolInfo {
        let tip_header = self.snapshot.tip_header();
        TxPoolInfo {
            tip_hash: tip_header.hash(),
            tip_number: tip_header.number(),
            pending_size: self.pending.size() + self.gap.size(),
            proposed_size: self.proposed.size(),
            orphan_size: 0,
            total_tx_size: self.total_tx_size,
            total_tx_cycles: self.total_tx_cycles,
            last_txs_updated_at: self.get_last_txs_updated_at(),
        }
    }

    /// Whether Tx-pool reach size limit
    pub fn reach_size_limit(&self, tx_size: usize) -> bool {
        (self.total_tx_size + tx_size) > self.config.max_mem_size
    }

    /// Whether Tx-pool reach cycles limit
    pub fn reach_cycles_limit(&self, cycles: Cycle) -> bool {
        (self.total_tx_cycles + cycles) > self.config.max_cycles
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
        self.pending.add_entry(entry).is_none()
    }

    /// Add tx which proposed but still uncommittable to gap pool
    pub fn add_gap(&mut self, entry: TxEntry) -> bool {
        trace!("add_gap {}", entry.transaction().hash());
        self.gap.add_entry(entry).is_none()
    }

    /// Add tx to proposed pool
    pub fn add_proposed(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        trace!("add_proposed {}", entry.transaction().hash());
        self.touch_last_txs_updated_at();
        self.proposed.add_entry(entry)
    }

    pub(crate) fn touch_last_txs_updated_at(&self) {
        self.last_txs_updated_at
            .store(unix_time_as_millis(), Ordering::SeqCst);
    }

    /// Get last txs in tx-pool update timestamp
    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.last_txs_updated_at.load(Ordering::SeqCst)
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

    /// Returns tx exclude conflict corresponding to the id. RPC
    pub fn get_tx_without_conflict(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
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
        txs: impl Iterator<Item = (&'a TransactionView, Vec<OutPoint>)>,
        callbacks: &Callbacks,
    ) {
        for (tx, related_out_points) in txs {
            let hash = tx.hash();
            trace!("committed {}", hash);
            // try remove committed tx from proposed
            if let Some(entry) = self.proposed.remove_committed_tx(tx, &related_out_points) {
                callbacks.call_committed(self, &entry)
            } else {
                // if committed tx is not in proposed, it may conflict
                let (input_conflict, deps_consumed) = self.proposed.resolve_conflict(tx);

                for (entry, reject) in input_conflict {
                    callbacks.call_reject(self, &entry, reject);
                }

                for (entry, reject) in deps_consumed {
                    callbacks.call_reject(self, &entry, reject);
                }
            }

            self.committed_txs_hash_cache
                .put(tx.proposal_short_id(), hash.to_owned());
        }
    }

    pub(crate) fn remove_expired<'a>(&mut self, ids: impl Iterator<Item = &'a ProposalShortId>) {
        for id in ids {
            if let Some(entry) = self.gap.remove_entry(id) {
                self.add_pending(entry);
            }
            let mut entries = self.proposed.remove_entry_and_descendants(id);
            entries.sort_unstable_by_key(|entry| entry.ancestors_count);
            for mut entry in entries {
                entry.reset_ancestors_state();
                self.add_pending(entry);
            }
        }
    }

    pub(crate) fn resolve_tx_from_pending_and_proposed(
        &self,
        tx: TransactionView,
        resolve_opts: ResolveOptions,
    ) -> Result<ResolvedTransaction, Reject> {
        let snapshot = self.snapshot();
        let proposed_provider = OverlayCellProvider::new(&self.proposed, snapshot);
        let gap_and_proposed_provider = OverlayCellProvider::new(&self.gap, &proposed_provider);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&self.pending, &gap_and_proposed_provider);
        let mut seen_inputs = HashSet::new();
        resolve_transaction_with_options(
            tx,
            &mut seen_inputs,
            &pending_and_proposed_provider,
            snapshot,
            resolve_opts,
        )
        .map_err(Reject::Resolve)
    }

    pub(crate) fn check_rtx_from_pending_and_proposed(
        &self,
        rtx: &ResolvedTransaction,
        resolve_opts: ResolveOptions,
    ) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let proposed_checker = OverlayCellChecker::new(&self.proposed, snapshot);
        let gap_and_proposed_checker = OverlayCellChecker::new(&self.gap, &proposed_checker);
        let pending_and_proposed_checker =
            OverlayCellChecker::new(&self.pending, &gap_and_proposed_checker);
        let mut seen_inputs = HashSet::new();
        rtx.check(
            &mut seen_inputs,
            &pending_and_proposed_checker,
            snapshot,
            resolve_opts,
        )
        .map_err(Reject::Resolve)
    }

    pub(crate) fn resolve_tx_from_proposed(
        &self,
        tx: TransactionView,
        resolve_opts: ResolveOptions,
    ) -> Result<ResolvedTransaction, Reject> {
        let snapshot = self.snapshot();
        let cell_provider = OverlayCellProvider::new(&self.proposed, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction_with_options(
            tx,
            &mut seen_inputs,
            &cell_provider,
            snapshot,
            resolve_opts,
        )
        .map_err(Reject::Resolve)
    }

    pub(crate) fn check_rtx_from_proposed(
        &self,
        rtx: &ResolvedTransaction,
        resolve_opts: ResolveOptions,
    ) -> Result<(), Reject> {
        let snapshot = self.snapshot();
        let cell_checker = OverlayCellChecker::new(&self.proposed, snapshot);
        let mut seen_inputs = HashSet::new();
        rtx.check(&mut seen_inputs, &cell_checker, snapshot, resolve_opts)
            .map_err(Reject::Resolve)
    }

    pub(crate) fn gap_rtx(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        rtx: ResolvedTransaction,
    ) -> Result<CacheEntry, Reject> {
        let snapshot = self.snapshot();
        let tip_header = snapshot.tip_header();
        let tx_env = TxVerifyEnv::new_proposed(tip_header, 0);

        let resolve_opts = {
            let proposal_window = snapshot.consensus().tx_proposal_window();
            let epoch_number = tx_env.epoch_number(proposal_window);
            let hardfork_switch = snapshot.consensus().hardfork_switch();
            let flag = hardfork_switch.is_remove_header_deps_immature_rule_enabled(epoch_number);
            ResolveOptions::empty().set_skip_immature_header_deps_check(flag)
        };
        self.check_rtx_from_pending_and_proposed(&rtx, resolve_opts)?;

        let max_cycles = snapshot.consensus().max_block_cycles();
        let verified = verify_rtx(snapshot, &rtx, &tx_env, &cache_entry, max_cycles)?;

        let entry = TxEntry::new(rtx, verified.cycles, verified.fee, size);
        let tx_hash = entry.transaction().hash();
        if self.add_gap(entry) {
            Ok(CacheEntry::Completed(verified))
        } else {
            Err(Reject::Duplicated(tx_hash))
        }
    }

    pub(crate) fn proposed_rtx(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        rtx: ResolvedTransaction,
    ) -> Result<CacheEntry, Reject> {
        let snapshot = self.snapshot();
        let tip_header = snapshot.tip_header();
        let tx_env = TxVerifyEnv::new_proposed(tip_header, 1);

        let resolve_opts = {
            let proposal_window = snapshot.consensus().tx_proposal_window();
            let epoch_number = tx_env.epoch_number(proposal_window);
            let hardfork_switch = snapshot.consensus().hardfork_switch();
            let flag = hardfork_switch.is_remove_header_deps_immature_rule_enabled(epoch_number);
            ResolveOptions::empty().set_skip_immature_header_deps_check(flag)
        };
        self.check_rtx_from_proposed(&rtx, resolve_opts)?;

        let max_cycles = snapshot.consensus().max_block_cycles();
        let verified = verify_rtx(snapshot, &rtx, &tx_env, &cache_entry, max_cycles)?;

        let entry = TxEntry::new(rtx, verified.cycles, verified.fee, size);
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
            .map(|tx_entry| tx_entry.rtx.transaction)
            .collect::<Vec<_>>();
        self.proposed.clear();
        txs.append(&mut self.gap.drain());
        txs.append(&mut self.pending.drain());
        txs
    }
}
