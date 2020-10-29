//! Top-level Pool type, methods, and tests
use super::component::{DefectEntry, TxEntry};
use crate::component::orphan::OrphanPool;
use crate::component::pending::PendingQueue;
use crate::component::proposed::ProposedPool;
use crate::error::Reject;
use ckb_app_config::TxPoolConfig;
use ckb_dao::DaoCalculator;
use ckb_error::{Error, ErrorKind};
use ckb_logger::{debug, error, trace};
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::core::BlockNumber;
use ckb_types::{
    core::{
        cell::{resolve_transaction, OverlayCellProvider, ResolvedTransaction},
        error::OutPointError,
        Capacity, Cycle, TransactionView,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use ckb_verification::cache::CacheEntry;
use ckb_verification::{TimeRelativeTransactionVerifier, TransactionVerifier};
use faketime::unix_time_as_millis;
use lru::LruCache;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// TODO(doc): @zhangsoledad
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
    /// cache for committed transactions hash
    pub(crate) committed_txs_hash_cache: LruCache<ProposalShortId, Byte32>,
    /// last txs updated timestamp, used by getblocktemplate
    pub(crate) last_txs_updated_at: Arc<AtomicU64>,
    // sum of all tx_pool tx's virtual sizes.
    pub(crate) total_tx_size: usize,
    // sum of all tx_pool tx's cycles.
    pub(crate) total_tx_cycles: Cycle,
    /// TODO(doc): @zhangsoledad
    pub snapshot: Arc<Snapshot>,
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
    /// TODO(doc): @zhangsoledad
    pub fn new(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        last_txs_updated_at: Arc<AtomicU64>,
    ) -> TxPool {
        let conflict_cache_size = config.max_conflict_cache_size;
        let committed_txs_hash_cache_size = config.max_committed_txs_hash_cache_size;

        TxPool {
            config,
            pending: PendingQueue::new(config.max_ancestors_count),
            gap: PendingQueue::new(config.max_ancestors_count),
            proposed: ProposedPool::new(config.max_ancestors_count),
            orphan: OrphanPool::new(),
            conflict: LruCache::new(conflict_cache_size),
            committed_txs_hash_cache: LruCache::new(committed_txs_hash_cache_size),
            last_txs_updated_at,
            total_tx_size: 0,
            total_tx_cycles: 0,
            snapshot,
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// TODO(doc): @zhangsoledad
    pub fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    /// TODO(doc): @zhangsoledad
    pub fn info(&self) -> TxPoolInfo {
        let tip_header = self.snapshot.tip_header();
        TxPoolInfo {
            tip_hash: tip_header.hash(),
            tip_number: tip_header.number(),
            pending_size: self.pending.size() + self.gap.size(),
            proposed_size: self.proposed.size(),
            orphan_size: self.orphan.size(),
            total_tx_size: self.total_tx_size,
            total_tx_cycles: self.total_tx_cycles,
            last_txs_updated_at: self.get_last_txs_updated_at(),
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn reach_size_limit(&self, tx_size: usize) -> bool {
        (self.total_tx_size + tx_size) > self.config.max_mem_size
    }

    /// TODO(doc): @zhangsoledad
    pub fn reach_cycles_limit(&self, cycles: Cycle) -> bool {
        (self.total_tx_cycles + cycles) > self.config.max_cycles
    }

    /// TODO(doc): @zhangsoledad
    pub fn update_statics_for_add_tx(&mut self, tx_size: usize, cycles: Cycle) {
        self.total_tx_size += tx_size;
        self.total_tx_cycles += cycles;
    }

    /// TODO(doc): @zhangsoledad
    // cycles overflow is possible, currently obtaining cycles is not accurate
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

    /// TODO(doc): @zhangsoledad
    // If did have this value present, false is returned.
    pub fn add_pending(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        if self
            .gap
            .contains_key(&entry.transaction.proposal_short_id())
        {
            return Ok(false);
        }
        trace!("add_pending {}", entry.transaction.hash());
        self.pending.add_entry(entry).map(|entry| entry.is_none())
    }

    /// TODO(doc): @zhangsoledad
    // add_gap inserts proposed but still uncommittable transaction.
    pub fn add_gap(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        trace!("add_gap {}", entry.transaction.hash());
        self.gap.add_entry(entry).map(|entry| entry.is_none())
    }

    /// TODO(doc): @zhangsoledad
    pub fn add_proposed(&mut self, entry: TxEntry) -> Result<bool, Reject> {
        trace!("add_proposed {}", entry.transaction.hash());
        self.touch_last_txs_updated_at();
        self.proposed.add_entry(entry).map(|entry| entry.is_none())
    }

    pub(crate) fn add_orphan(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
        unknowns: Vec<OutPoint>,
    ) -> Option<DefectEntry> {
        trace!("add_orphan {}", &tx.hash());
        self.orphan
            .add_tx(cache_entry, size, tx, unknowns.into_iter())
    }

    pub(crate) fn touch_last_txs_updated_at(&self) {
        self.last_txs_updated_at
            .store(unix_time_as_millis(), Ordering::SeqCst);
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.last_txs_updated_at.load(Ordering::SeqCst)
    }

    /// TODO(doc): @zhangsoledad
    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.conflict.contains(id)
            || self.proposed.contains_key(id)
            || self.orphan.contains_key(id)
    }

    /// TODO(doc): @zhangsoledad
    pub fn contains_tx(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.gap.contains_key(id)
            || self.proposed.contains_key(id)
            || self.orphan.contains_key(id)
            || self.conflict.contains(id)
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_tx_with_cycles(
        &self,
        id: &ProposalShortId,
    ) -> Option<(TransactionView, Option<Cycle>)> {
        self.pending
            .get(id)
            .cloned()
            .map(|entry| (entry.transaction, Some(entry.cycles)))
            .or_else(|| {
                self.gap
                    .get(id)
                    .cloned()
                    .map(|entry| (entry.transaction, Some(entry.cycles)))
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
                    .map(|entry| (entry.transaction, entry.cache_entry.map(|c| c.cycles)))
            })
            .or_else(|| {
                self.conflict
                    .peek(id)
                    .cloned()
                    .map(|entry| (entry.transaction, entry.cache_entry.map(|c| c.cycles)))
            })
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.peek(id).map(|e| &e.transaction))
            .cloned()
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_tx_without_conflict(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .cloned()
    }

    /// TODO(doc): @zhangsoledad
    pub fn proposed(&self) -> &ProposedPool {
        &self.proposed
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_tx_from_proposed_and_others(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.proposed
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.pending.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.peek(id).map(|e| &e.transaction))
            .cloned()
    }

    pub(crate) fn remove_committed_txs_from_proposed<'a>(
        &mut self,
        txs: impl Iterator<Item = (&'a TransactionView, Vec<OutPoint>)>,
    ) {
        for (tx, related_out_points) in txs {
            let hash = tx.hash();
            trace!("committed {}", hash);
            for entry in self.proposed.remove_committed_tx(tx, &related_out_points) {
                self.update_statics_for_remove_tx(entry.size, entry.cycles);
            }
            self.committed_txs_hash_cache
                .put(tx.proposal_short_id(), hash.to_owned());
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn remove_expired<'a>(&mut self, ids: impl Iterator<Item = &'a ProposalShortId>) {
        for id in ids {
            for entry in self.gap.remove_entry_and_descendants(id) {
                if let Err(err) = self.add_pending(entry) {
                    debug!("move expired gap to pending error {}", err);
                }
            }
            for entry in self.proposed.remove_entry_and_descendants(id) {
                if let Err(err) = self.add_pending(entry) {
                    debug!("move expired proposed to pending error {}", err);
                }
            }
        }
    }

    fn contains_proposed(&self, short_id: &ProposalShortId) -> bool {
        self.snapshot().proposals().contains_proposed(short_id)
    }

    /// TODO(doc): @zhangsoledad
    pub fn resolve_tx_from_pending_and_proposed(
        &self,
        tx: TransactionView,
    ) -> Result<ResolvedTransaction, Error> {
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
    }

    /// TODO(doc): @zhangsoledad
    pub fn resolve_tx_from_proposed(
        &self,
        tx: TransactionView,
    ) -> Result<ResolvedTransaction, Error> {
        let snapshot = self.snapshot();
        let cell_provider = OverlayCellProvider::new(&self.proposed, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(tx, &mut seen_inputs, &cell_provider, snapshot)
    }

    pub(crate) fn verify_rtx(
        &self,
        rtx: &ResolvedTransaction,
        cache_entry: Option<CacheEntry>,
    ) -> Result<CacheEntry, Error> {
        let snapshot = self.snapshot();
        let tip_header = snapshot.tip_header();
        let tip_number = tip_header.number();
        let epoch_number = tip_header.epoch();
        let consensus = snapshot.consensus();

        match cache_entry {
            Some(cache_entry) => {
                TimeRelativeTransactionVerifier::new(
                    &rtx,
                    snapshot,
                    tip_number + 1,
                    epoch_number,
                    tip_header.hash(),
                    consensus,
                )
                .verify()?;
                Ok(cache_entry)
            }
            None => {
                let max_cycles = consensus.max_block_cycles();
                let cache_entry = TransactionVerifier::new(
                    &rtx,
                    snapshot,
                    tip_number + 1,
                    epoch_number,
                    tip_header.hash(),
                    consensus,
                    snapshot,
                )
                .verify(max_cycles)?;
                Ok(cache_entry)
            }
        }
    }

    // remove resolved tx from orphan pool
    pub(crate) fn try_proposed_orphan_by_ancestor(&mut self, tx: &TransactionView) {
        let entries = self.orphan.remove_by_ancestor(tx);
        for entry in entries {
            let tx_hash = entry.transaction.hash();
            if self.contains_proposed(&entry.transaction.proposal_short_id()) {
                let ret = self.proposed_tx(entry.cache_entry, entry.size, entry.transaction);
                if ret.is_err() {
                    self.update_statics_for_remove_tx(
                        entry.size,
                        entry.cache_entry.map(|c| c.cycles).unwrap_or(0),
                    );
                    trace!("proposed tx {} failed {:?}", tx_hash, ret);
                }
            } else {
                let ret = self.pending_tx(entry.cache_entry, entry.size, entry.transaction);
                if ret.is_err() {
                    self.update_statics_for_remove_tx(
                        entry.size,
                        entry.cache_entry.map(|c| c.cycles).unwrap_or(0),
                    );
                    trace!("pending tx {} failed {:?}", tx_hash, ret);
                }
            }
        }
    }

    pub(crate) fn calculate_transaction_fee(
        &self,
        snapshot: &Snapshot,
        rtx: &ResolvedTransaction,
    ) -> Result<Capacity, Error> {
        DaoCalculator::new(snapshot.consensus(), snapshot)
            .transaction_fee(&rtx)
            .map_err(|err| {
                error!(
                    "Failed to generate tx fee for {}, reason: {:?}",
                    rtx.transaction.hash(),
                    err
                );
                err
            })
    }

    fn handle_tx_by_resolved_result<F>(
        &mut self,
        pool_name: &str,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
        tx_resolved_result: Result<(CacheEntry, Vec<OutPoint>), Error>,
        add_to_pool: F,
    ) -> Result<CacheEntry, Error>
    where
        F: FnOnce(
            &mut TxPool,
            Cycle,
            Capacity,
            usize,
            Vec<OutPoint>,
            TransactionView,
        ) -> Result<(), Error>,
    {
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        match tx_resolved_result {
            Ok((cache_entry, related_dep_out_points)) => {
                add_to_pool(
                    self,
                    cache_entry.cycles,
                    cache_entry.fee,
                    size,
                    related_dep_out_points,
                    tx,
                )?;
                Ok(cache_entry)
            }
            Err(err) => {
                match err.kind() {
                    ErrorKind::Transaction => {
                        self.update_statics_for_remove_tx(
                            size,
                            cache_entry.map(|c| c.cycles).unwrap_or(0),
                        );
                        debug!(
                            "Failed to add tx to {} {}, verify failed, reason: {:?}",
                            pool_name, tx_hash, err,
                        );
                    }
                    ErrorKind::OutPoint => {
                        match err
                            .downcast_ref::<OutPointError>()
                            .expect("error kind checked")
                        {
                            OutPointError::Dead(_) => {
                                if self
                                    .conflict
                                    .put(short_id, DefectEntry::new(tx, 0, cache_entry, size))
                                    .is_some()
                                {
                                    self.update_statics_for_remove_tx(
                                        size,
                                        cache_entry.map(|c| c.cycles).unwrap_or(0),
                                    );
                                }
                            }

                            OutPointError::Unknown(out_points) => {
                                if self
                                    .add_orphan(cache_entry, size, tx, out_points.to_owned())
                                    .is_some()
                                {
                                    self.update_statics_for_remove_tx(
                                        size,
                                        cache_entry.map(|c| c.cycles).unwrap_or(0),
                                    );
                                }
                            }

                            // The remaining errors represent invalid transactions that should
                            // just be discarded.
                            //
                            // To avoid mis-discarding error types added in the future, please don't
                            // use placeholder `_` as the match arm.
                            //
                            // OutOfOrder should only appear in BlockCellProvider
                            OutPointError::ImmatureHeader(_)
                            | OutPointError::InvalidHeader(_)
                            | OutPointError::InvalidDepGroup(_)
                            | OutPointError::OutOfOrder(_) => {
                                self.update_statics_for_remove_tx(
                                    size,
                                    cache_entry.map(|c| c.cycles).unwrap_or(0),
                                );
                            }
                        }
                    }
                    _ => {
                        debug!(
                            "Failed to add tx to {} {}, unknown reason: {:?}",
                            pool_name, tx_hash, err
                        );
                        self.update_statics_for_remove_tx(
                            size,
                            cache_entry.map(|c| c.cycles).unwrap_or(0),
                        );
                    }
                }
                Err(err)
            }
        }
    }

    pub(crate) fn gap_tx(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
    ) -> Result<CacheEntry, Error> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(tx.clone())
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cache_entry).map(|cache_entry| {
                    let related_dep_out_points = rtx.related_dep_out_points();
                    (cache_entry, related_dep_out_points)
                })
            });
        self.handle_tx_by_resolved_result(
            "gap",
            cache_entry,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                let tx_hash = entry.transaction.hash();
                if tx_pool.add_gap(entry)? {
                    Ok(())
                } else {
                    Err(Reject::Duplicated(tx_hash).into())
                }
            },
        )
    }

    pub(crate) fn proposed_tx(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
    ) -> Result<CacheEntry, Error> {
        let tx_result = self.resolve_tx_from_proposed(tx.clone()).and_then(|rtx| {
            self.verify_rtx(&rtx, cache_entry).map(|cache_entry| {
                let related_dep_out_points = rtx.related_dep_out_points();
                (cache_entry, related_dep_out_points)
            })
        });
        self.handle_tx_by_resolved_result(
            "proposed",
            cache_entry,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                tx_pool.add_proposed(entry)?;
                Ok(())
            },
        )
    }

    fn pending_tx(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
    ) -> Result<CacheEntry, Error> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(tx.clone())
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cache_entry).map(|cache_entry| {
                    let related_dep_out_points = rtx.related_dep_out_points();
                    (cache_entry, related_dep_out_points)
                })
            });
        self.handle_tx_by_resolved_result(
            "pending",
            cache_entry,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                let tx_hash = entry.transaction.hash();
                if tx_pool.add_pending(entry)? {
                    Ok(())
                } else {
                    Err(Reject::Duplicated(tx_hash).into())
                }
            },
        )
    }

    pub(crate) fn proposed_tx_and_descendants(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
    ) -> Result<CacheEntry, Error> {
        self.proposed_tx(cache_entry, size, tx.clone())
            .map(|cache_entry| {
                self.try_proposed_orphan_by_ancestor(&tx);
                cache_entry
            })
    }

    pub(crate) fn readd_dettached_tx(
        &mut self,
        snapshot: &Snapshot,
        txs_verify_cache: &HashMap<Byte32, CacheEntry>,
        tx: TransactionView,
    ) -> Option<(Byte32, CacheEntry)> {
        let mut ret = None;
        let tx_hash = tx.hash();
        let cache_entry = txs_verify_cache.get(&tx_hash).cloned();
        let tx_short_id = tx.proposal_short_id();
        let tx_size = tx.data().serialized_size_in_block();
        if snapshot.proposals().contains_proposed(&tx_short_id) {
            if let Ok(new_cache_entry) = self.proposed_tx_and_descendants(cache_entry, tx_size, tx)
            {
                if cache_entry.is_none() {
                    ret = Some((tx_hash, new_cache_entry));
                }
                self.update_statics_for_add_tx(tx_size, new_cache_entry.cycles);
            }
        } else if snapshot.proposals().contains_gap(&tx_short_id) {
            if let Ok(new_cache_entry) = self.gap_tx(cache_entry, tx_size, tx) {
                if cache_entry.is_none() {
                    ret = Some((tx_hash, new_cache_entry));
                }
                self.update_statics_for_add_tx(tx_size, cache_entry.map(|c| c.cycles).unwrap_or(0));
            }
        } else if let Ok(new_cache_entry) = self.pending_tx(cache_entry, tx_size, tx) {
            if cache_entry.is_none() {
                ret = Some((tx_hash, new_cache_entry));
            }
            self.update_statics_for_add_tx(tx_size, cache_entry.map(|c| c.cycles).unwrap_or(0));
        }

        ret
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
    ) -> HashSet<ProposalShortId> {
        let min_fee_rate = self.config.min_fee_rate;
        let mut proposals = HashSet::with_capacity(limit);
        self.pending
            .fill_proposals(limit, min_fee_rate, exclusion, &mut proposals);
        self.gap
            .fill_proposals(limit, min_fee_rate, exclusion, &mut proposals);
        proposals
    }

    /// TODO(doc): @zhangsoledad
    pub fn get_tx_from_pool_or_store(
        &self,
        proposal_id: &ProposalShortId,
    ) -> Option<TransactionView> {
        self.get_tx_from_proposed_and_others(proposal_id)
            .or_else(|| {
                self.committed_txs_hash_cache
                    .peek(proposal_id)
                    .and_then(|tx_hash| self.snapshot().get_transaction(tx_hash).map(|(tx, _)| tx))
            })
    }
}
