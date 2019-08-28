//! Top-level Pool type, methods, and tests
use super::component::{DefectEntry, TxEntry};
use crate::component::orphan::OrphanPool;
use crate::component::pending::PendingQueue;
use crate::component::proposed::ProposedPool;
use crate::config::TxPoolConfig;
use crate::error::PoolError;
use ckb_dao::DaoCalculator;
use ckb_logger::{debug_target, error_target, trace_target};
use ckb_script::ScriptConfig;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{
            get_related_dep_out_points, resolve_transaction, OverlayCellProvider,
            ResolvedTransaction, UnresolvableError,
        },
        BlockView, Capacity, Cycle, TransactionView,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use faketime::unix_time_as_millis;
use lru_cache::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
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
    pub(crate) last_txs_updated_at: u64,
    // sum of all tx_pool tx's virtual sizes.
    pub(crate) total_tx_size: usize,
    // sum of all tx_pool tx's cycles.
    pub(crate) total_tx_cycles: Cycle,
    pub snapshot: Arc<Snapshot>,
    pub script_config: ScriptConfig,
}

#[derive(Clone)]
pub struct TxPoolInfo {
    pub pending_size: usize,
    pub proposed_size: usize,
    pub orphan_size: usize,
    pub total_tx_size: usize,
    pub total_tx_cycles: Cycle,
    pub last_txs_updated_at: u64,
}

impl TxPool {
    pub fn new(
        config: TxPoolConfig,
        snapshot: Arc<Snapshot>,
        script_config: ScriptConfig,
    ) -> TxPool {
        let conflict_cache_size = config.max_conflict_cache_size;
        let committed_txs_hash_cache_size = config.max_committed_txs_hash_cache_size;
        let last_txs_updated_at = 0u64;

        TxPool {
            config,
            pending: PendingQueue::new(),
            gap: PendingQueue::new(),
            proposed: ProposedPool::new(),
            orphan: OrphanPool::new(),
            conflict: LruCache::new(conflict_cache_size),
            committed_txs_hash_cache: LruCache::new(committed_txs_hash_cache_size),
            last_txs_updated_at,
            total_tx_size: 0,
            total_tx_cycles: 0,
            snapshot,
            script_config,
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn cloned_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.snapshot)
    }

    pub fn info(&self) -> TxPoolInfo {
        TxPoolInfo {
            pending_size: self.pending.size(),
            proposed_size: self.proposed.size(),
            orphan_size: self.proposed.size(),
            total_tx_size: self.total_tx_size,
            total_tx_cycles: self.total_tx_cycles,
            last_txs_updated_at: self.last_txs_updated_at,
        }
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

    // If did have this value present, false is returned.
    pub fn add_pending(&mut self, entry: TxEntry) -> bool {
        if self
            .gap
            .contains_key(&entry.transaction.proposal_short_id())
        {
            return false;
        }
        trace_target!(
            crate::LOG_TARGET_TX_POOL,
            "add_pending {}",
            entry.transaction.hash()
        );
        self.pending.add_entry(entry).is_none()
    }

    // add_gap inserts proposed but still uncommittable transaction.
    pub fn add_gap(&mut self, entry: TxEntry) -> bool {
        trace_target!(
            crate::LOG_TARGET_TX_POOL,
            "add_gap {}",
            entry.transaction.hash()
        );
        self.gap.add_entry(entry).is_none()
    }

    pub fn add_proposed(&mut self, entry: TxEntry) -> bool {
        trace_target!(
            crate::LOG_TARGET_TX_POOL,
            "add_proposed {}",
            entry.transaction.hash()
        );
        self.touch_last_txs_updated_at();
        self.proposed.add_entry(entry).is_none()
    }

    pub(crate) fn add_orphan(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
        unknowns: Vec<OutPoint>,
    ) -> Option<DefectEntry> {
        trace_target!(crate::LOG_TARGET_TX_POOL, "add_orphan {}", &tx.hash());
        self.orphan.add_tx(cycles, size, tx, unknowns.into_iter())
    }

    pub(crate) fn touch_last_txs_updated_at(&mut self) {
        self.last_txs_updated_at = unix_time_as_millis();
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.last_txs_updated_at
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.conflict.contains_key(id)
            || self.proposed.contains_key(id)
            || self.orphan.contains_key(id)
    }

    pub fn contains_tx(&self, id: &ProposalShortId) -> bool {
        self.pending.contains_key(id)
            || self.gap.contains_key(id)
            || self.proposed.contains_key(id)
            || self.orphan.contains_key(id)
            || self.conflict.contains_key(id)
    }

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
                    .map(|entry| (entry.transaction, entry.cycles))
            })
            .or_else(|| {
                self.conflict
                    .get(id)
                    .cloned()
                    .map(|entry| (entry.transaction, entry.cycles))
            })
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.get(id).map(|e| &e.transaction))
            .cloned()
    }

    pub fn get_tx_without_conflict(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.pending
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.proposed.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .cloned()
    }

    pub fn proposed(&self) -> &ProposedPool {
        &self.proposed
    }

    pub fn get_tx_from_proposed_and_others(&self, id: &ProposalShortId) -> Option<TransactionView> {
        self.proposed
            .get_tx(id)
            .or_else(|| self.gap.get_tx(id))
            .or_else(|| self.pending.get_tx(id))
            .or_else(|| self.orphan.get_tx(id))
            .or_else(|| self.conflict.get(id).map(|e| &e.transaction))
            .cloned()
    }

    pub(crate) fn remove_committed_txs_from_proposed<'a>(
        &mut self,
        txs: impl Iterator<Item = (&'a TransactionView, Vec<OutPoint>)>,
    ) {
        for (tx, related_out_points) in txs {
            let hash = tx.hash();
            trace_target!(crate::LOG_TARGET_TX_POOL, "committed {}", hash);
            for entry in self.proposed.remove_committed_tx(tx, &related_out_points) {
                self.update_statics_for_remove_tx(entry.size, entry.cycles);
            }
            self.committed_txs_hash_cache
                .insert(tx.proposal_short_id(), hash.to_owned());
        }
    }

    pub fn remove_expired<'a>(&mut self, ids: impl Iterator<Item = &'a ProposalShortId>) {
        for id in ids {
            for entry in self.gap.remove_entry_and_descendants(id) {
                self.add_pending(entry);
            }
            for entry in self.proposed.remove_entry_and_descendants(id) {
                self.add_pending(entry);
            }
        }
    }

    pub fn add_tx_to_pool(
        &mut self,
        tx: TransactionView,
        cycles: Cycle,
    ) -> Result<Cycle, PoolError> {
        let tx_size = tx.serialized_size();
        if self.reach_size_limit(tx_size) {
            return Err(PoolError::LimitReached);
        }
        let short_id = tx.proposal_short_id();
        match self.resolve_tx_from_pending_and_proposed(&tx) {
            Ok(rtx) => self.verify_rtx(&rtx, Some(cycles)).and_then(|cycles| {
                if self.reach_cycles_limit(cycles) {
                    return Err(PoolError::LimitReached);
                }
                if self.contains_proposed(&short_id) {
                    if let Err(e) = self.proposed_tx_and_descendants(Some(cycles), tx_size, tx) {
                        debug_target!(
                            crate::LOG_TARGET_TX_POOL,
                            "Failed to add proposed tx {:?}, reason: {:?}",
                            short_id,
                            e
                        );
                        return Err(e);
                    }
                    self.update_statics_for_add_tx(tx_size, cycles);
                    return Ok(cycles);
                }
                if let Err(e) = self.pending_tx(Some(cycles), tx_size, tx) {
                    return Err(e);
                } else {
                    self.update_statics_for_add_tx(tx_size, cycles);
                    return Ok(cycles);
                }
            }),
            Err(err) => Err(PoolError::UnresolvableTransaction(err)),
        }
    }

    fn contains_proposed(&self, short_id: &ProposalShortId) -> bool {
        self.snapshot().proposals().contains_proposed(short_id)
    }

    pub fn resolve_tx_from_pending_and_proposed<'a>(
        &self,
        tx: &'a TransactionView,
    ) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
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

    pub fn resolve_tx_from_proposed<'a>(
        &self,
        tx: &'a TransactionView,
    ) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
        let snapshot = self.snapshot();
        let cell_provider = OverlayCellProvider::new(&self.proposed, snapshot);
        let mut seen_inputs = HashSet::new();
        resolve_transaction(tx, &mut seen_inputs, &cell_provider, snapshot)
    }

    pub(crate) fn verify_rtx(
        &self,
        rtx: &ResolvedTransaction,
        cycles: Option<Cycle>,
    ) -> Result<Cycle, PoolError> {
        let snapshot = self.snapshot();
        let tip_header = snapshot.tip_header();
        let tip_number = tip_header.number();
        let epoch_number = tip_header.epoch();
        let consensus = snapshot.consensus();

        match cycles {
            Some(cycles) => {
                ContextualTransactionVerifier::new(
                    &rtx,
                    snapshot,
                    tip_number + 1,
                    epoch_number,
                    tip_header.hash(),
                    consensus,
                )
                .verify()
                .map_err(PoolError::InvalidTx)?;
                Ok(cycles)
            }
            None => {
                let max_cycles = consensus.max_block_cycles();
                let cycles = TransactionVerifier::new(
                    &rtx,
                    snapshot,
                    tip_number + 1,
                    epoch_number,
                    tip_header.hash(),
                    consensus,
                    &self.script_config,
                    snapshot,
                )
                .verify(max_cycles)
                .map_err(PoolError::InvalidTx)?;
                Ok(cycles)
            }
        }
    }

    // remove resolved tx from orphan pool
    pub(crate) fn try_proposed_orphan_by_ancestor(&mut self, tx: &TransactionView) {
        let entries = self.orphan.remove_by_ancestor(tx);
        for entry in entries {
            let tx_hash = entry.transaction.hash().to_owned();
            if self.contains_proposed(&tx.proposal_short_id()) {
                let ret = self.proposed_tx(entry.cycles, entry.size, entry.transaction);
                if ret.is_err() {
                    self.update_statics_for_remove_tx(entry.size, entry.cycles.unwrap_or(0));
                    trace_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "proposed tx {} failed {:?}",
                        tx_hash,
                        ret
                    );
                }
            } else {
                let ret = self.pending_tx(entry.cycles, entry.size, entry.transaction);
                if ret.is_err() {
                    self.update_statics_for_remove_tx(entry.size, entry.cycles.unwrap_or(0));
                    trace_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "pending tx {} failed {:?}",
                        tx_hash,
                        ret
                    );
                }
            }
        }
    }

    pub(crate) fn calculate_transaction_fee(
        &self,
        snapshot: &Snapshot,
        rtx: &ResolvedTransaction,
    ) -> Result<Capacity, PoolError> {
        DaoCalculator::new(snapshot.consensus(), snapshot)
            .transaction_fee(&rtx)
            .map_err(|err| PoolError::TxFee(err.to_string()))
    }

    fn handle_tx_by_resolved_result<F>(
        &mut self,
        pool_name: &str,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
        tx_resolved_result: Result<(Cycle, Capacity, Vec<OutPoint>), PoolError>,
        add_to_pool: F,
    ) -> Result<Cycle, PoolError>
    where
        F: FnOnce(
            &mut TxPool,
            Cycle,
            Capacity,
            usize,
            Vec<OutPoint>,
            TransactionView,
        ) -> Result<(), PoolError>,
    {
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        match tx_resolved_result {
            Ok((cycles, fee, related_dep_out_points)) => {
                add_to_pool(self, cycles, fee, size, related_dep_out_points, tx)?;
                Ok(cycles)
            }
            Err(PoolError::InvalidTx(e)) => {
                self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to {} {}, verify failed, reason: {:?}",
                    pool_name,
                    tx_hash,
                    e
                );
                Err(PoolError::InvalidTx(e))
            }
            Err(PoolError::UnresolvableTransaction(err)) => {
                match &err {
                    UnresolvableError::Dead(_) => {
                        if self
                            .conflict
                            .insert(short_id, DefectEntry::new(tx, 0, cycles, size))
                            .is_some()
                        {
                            self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                        }
                    }
                    UnresolvableError::Unknown(out_points) => {
                        if self
                            .add_orphan(cycles, size, tx, out_points.to_owned())
                            .is_some()
                        {
                            self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                        }
                    }
                    // The remaining errors are InvalidHeader/InvalidDepGroup.
                    // They all represent invalid transactions
                    // that should just be discarded.
                    // OutOfOrder should only appear in BlockCellProvider
                    UnresolvableError::InvalidDepGroup(_)
                    | UnresolvableError::InvalidHeader(_)
                    | UnresolvableError::OutOfOrder(_) => {
                        self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                    }
                }
                Err(PoolError::UnresolvableTransaction(err))
            }
            Err(err) => {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to {} {}, reason: {:?}",
                    pool_name,
                    tx_hash,
                    err
                );
                self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                Err(err)
            }
        }
    }

    pub(crate) fn gap_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, PoolError> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(&tx)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cycles).and_then(|cycles| {
                    let fee = self.calculate_transaction_fee(self.snapshot(), &rtx);
                    let related_dep_out_points = rtx.related_dep_out_points();
                    fee.map(|fee| (cycles, fee, related_dep_out_points))
                })
            });
        self.handle_tx_by_resolved_result(
            "gap",
            cycles,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                if tx_pool.add_gap(entry) {
                    Ok(())
                } else {
                    Err(PoolError::Duplicate)
                }
            },
        )
    }

    pub(crate) fn proposed_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, PoolError> {
        let tx_result = self
            .resolve_tx_from_proposed(&tx)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cycles).and_then(|cycles| {
                    let fee = self.calculate_transaction_fee(self.snapshot(), &rtx);
                    let related_dep_out_points = rtx.related_dep_out_points();
                    fee.map(|fee| (cycles, fee, related_dep_out_points))
                })
            });
        self.handle_tx_by_resolved_result(
            "proposed",
            cycles,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                tx_pool.add_proposed(entry);
                Ok(())
            },
        )
    }

    fn pending_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, PoolError> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(&tx)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cycles).and_then(|cycles| {
                    let fee = self.calculate_transaction_fee(self.snapshot(), &rtx);
                    let related_dep_out_points = rtx.related_dep_out_points();
                    fee.map(|fee| (cycles, fee, related_dep_out_points))
                })
            });
        self.handle_tx_by_resolved_result(
            "pending",
            cycles,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                if tx_pool.add_pending(entry) {
                    Ok(())
                } else {
                    Err(PoolError::Duplicate)
                }
            },
        )
    }

    pub(crate) fn proposed_tx_and_descendants(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, PoolError> {
        self.proposed_tx(cycles, size, tx.clone()).map(|cycles| {
            self.try_proposed_orphan_by_ancestor(&tx);
            cycles
        })
    }

    pub(crate) fn readd_dettached_tx(
        &mut self,
        snapshot: &Snapshot,
        txs_verify_cache: &mut LruCache<Byte32, Cycle>,
        tx: TransactionView,
    ) {
        let tx_hash = tx.hash().to_owned();
        let cached_cycles = txs_verify_cache.get(&tx_hash).cloned();
        let tx_short_id = tx.proposal_short_id();
        let tx_size = tx.serialized_size();
        if snapshot.proposals().contains_proposed(&tx_short_id) {
            if let Ok(cycles) = self.proposed_tx_and_descendants(cached_cycles, tx_size, tx) {
                if cached_cycles.is_none() {
                    txs_verify_cache.insert(tx_hash, cycles);
                }
                self.update_statics_for_add_tx(tx_size, cycles);
            }
        } else if snapshot.proposals().contains_gap(&tx_short_id) {
            if let Ok(cycles) = self.gap_tx(cached_cycles, tx_size, tx) {
                if cached_cycles.is_none() {
                    txs_verify_cache.insert(tx_hash, cycles);
                }
                self.update_statics_for_add_tx(tx_size, cached_cycles.unwrap_or(0));
            }
        } else if let Ok(cycles) = self.pending_tx(cached_cycles, tx_size, tx) {
            if cached_cycles.is_none() {
                txs_verify_cache.insert(tx_hash, cycles);
            }
            self.update_statics_for_add_tx(tx_size, cached_cycles.unwrap_or(0));
        }
    }

    pub fn update_tx_pool_for_reorg<'a>(
        &mut self,
        detached_blocks: impl Iterator<Item = &'a BlockView>,
        attached_blocks: impl Iterator<Item = &'a BlockView>,
        detached_proposal_id: impl Iterator<Item = &'a ProposalShortId>,
        txs_verify_cache: &mut LruCache<Byte32, Cycle>,
        snapshot: Arc<Snapshot>,
    ) {
        self.snapshot = Arc::clone(&snapshot);
        let mut detached = LinkedHashSet::default();
        let mut attached = LinkedHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().iter().skip(1).cloned())
        }

        let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let txs_iter = attached.iter().map(|tx| {
            let get_cell_data = |out_point: &OutPoint| {
                snapshot
                    .get_cell_data(&out_point.tx_hash(), out_point.index().unpack())
                    .map(|result| result.0)
            };
            let related_out_points =
                get_related_dep_out_points(tx, get_cell_data).expect("Get dep out points failed");
            (tx, related_out_points)
        });
        self.remove_expired(detached_proposal_id);
        self.remove_committed_txs_from_proposed(txs_iter);

        for tx in retain {
            self.readd_dettached_tx(&snapshot, txs_verify_cache, tx);
        }

        for tx in &attached {
            self.try_proposed_orphan_by_ancestor(tx);
        }

        let mut entries = Vec::new();
        let mut gaps = Vec::new();

        // pending ---> gap ----> proposed
        // try move gap to proposed
        let mut removed: Vec<ProposalShortId> = Vec::with_capacity(self.gap.size());
        for id in self.gap.sorted_keys() {
            if snapshot.proposals().contains_proposed(&id) {
                let entry = self.gap.get(&id).expect("exists");
                entries.push((Some(entry.cycles), entry.size, entry.transaction.to_owned()));
                removed.push(id.clone());
            }
        }
        removed.into_iter().for_each(|id| {
            self.gap.remove_entry_and_descendants(&id);
        });

        // try move pending to proposed
        let mut removed: Vec<ProposalShortId> = Vec::with_capacity(self.pending.size());
        for id in self.pending.sorted_keys() {
            let entry = self.pending.get(&id).expect("exists");
            if snapshot.proposals().contains_proposed(&id) {
                entries.push((Some(entry.cycles), entry.size, entry.transaction.to_owned()));
                removed.push(id.clone());
            } else if snapshot.proposals().contains_gap(&id) {
                gaps.push((Some(entry.cycles), entry.size, entry.transaction.to_owned()));
                removed.push(id.clone());
            }
        }
        removed.into_iter().for_each(|id| {
            self.pending.remove_entry_and_descendants(&id);
        });

        // try move conflict to proposed
        for entry in self.conflict.entries() {
            if snapshot.proposals().contains_proposed(entry.key()) {
                let entry = entry.remove();
                entries.push((entry.cycles, entry.size, entry.transaction));
            } else if snapshot.proposals().contains_gap(entry.key()) {
                let entry = entry.remove();
                gaps.push((entry.cycles, entry.size, entry.transaction));
            }
        }

        for (cycles, size, tx) in entries {
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.proposed_tx_and_descendants(cycles, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add proposed tx {}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }

        for (cycles, size, tx) in gaps {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "tx proposed, add to gap {}",
                tx.hash()
            );
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.gap_tx(cycles, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to gap {}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }
    }

    pub fn get_proposals(&self, limit: usize) -> HashSet<ProposalShortId> {
        let mut proposals = HashSet::with_capacity(limit);
        self.pending.fill_proposals(limit, &mut proposals);
        self.gap.fill_proposals(limit, &mut proposals);
        proposals
    }

    pub fn get_tx_from_pool_or_store(
        &self,
        proposal_id: &ProposalShortId,
    ) -> Option<TransactionView> {
        self.get_tx_from_proposed_and_others(proposal_id)
            .or_else(|| {
                self.committed_txs_hash_cache
                    .get(proposal_id)
                    .and_then(|tx_hash| self.snapshot().get_transaction(tx_hash).map(|(tx, _)| tx))
            })
    }
}
