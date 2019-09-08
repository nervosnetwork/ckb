use crate::snapshot::Snapshot;
use crate::tx_pool::types::{DefectEntry, TxEntry};
use crate::tx_pool::TxPool;
use ckb_dao::DaoCalculator;
use ckb_error::{Error, ErrorKind, InternalErrorKind};
use ckb_logger::{debug_target, error_target, trace_target};
use ckb_store::ChainStore;
use ckb_types::{
    core::{
        cell::{
            get_related_dep_out_points, resolve_transaction, OverlayCellProvider,
            ResolvedTransaction,
        },
        error::OutPointError,
        BlockView, Capacity, Cycle, TransactionView,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::LinkedHashSet;
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use lru_cache::LruCache;
use std::collections::HashSet;
use std::sync::Arc;

impl TxPool {
    // Add a verified tx into pool
    // this method will handle fork related verifications to make sure we are safe during a fork
    pub fn add_tx_to_pool(&mut self, tx: TransactionView, cycles: Cycle) -> Result<Cycle, Error> {
        let tx_size = tx.serialized_size();
        if self.reach_size_limit(tx_size) {
            Err(InternalErrorKind::TransactionPoolFull)?;
        }
        let short_id = tx.proposal_short_id();
        match self.resolve_tx_from_pending_and_proposed(tx.clone()) {
            Ok(rtx) => self.verify_rtx(&rtx, Some(cycles)).and_then(|cycles| {
                if self.reach_cycles_limit(cycles) {
                    Err(InternalErrorKind::TransactionPoolFull)?;
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
            Err(err) => Err(err),
        }
    }

    fn contains_proposed(&self, short_id: &ProposalShortId) -> bool {
        self.snapshot().proposals().contains_proposed(short_id)
    }

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
        cycles: Option<Cycle>,
    ) -> Result<Cycle, Error> {
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
                .verify()?;
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
                    snapshot,
                )
                .verify(max_cycles)?;
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

    fn calculate_transaction_fee(&self, rtx: &ResolvedTransaction) -> Result<Capacity, Error> {
        let snapshot = self.snapshot();
        DaoCalculator::new(snapshot.consensus(), snapshot)
            .transaction_fee(&rtx)
            .map_err(|err| {
                error_target!(
                    crate::LOG_TARGET_TX_POOL,
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
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
        tx_resolved_result: Result<(Cycle, Capacity, Vec<OutPoint>), Error>,
        add_to_pool: F,
    ) -> Result<Cycle, Error>
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
            Ok((cycles, fee, related_dep_out_points)) => {
                add_to_pool(self, cycles, fee, size, related_dep_out_points, tx)?;
                Ok(cycles)
            }
            Err(err) => {
                match err.kind() {
                    ErrorKind::Transaction => {
                        self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                        debug_target!(
                            crate::LOG_TARGET_TX_POOL,
                            "Failed to add tx to {} {}, verify failed, reason: {:?}",
                            pool_name,
                            tx_hash,
                            err,
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
                                    .insert(short_id, DefectEntry::new(tx, 0, cycles, size))
                                    .is_some()
                                {
                                    self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                                }
                            }

                            OutPointError::Unknown(out_points) => {
                                if self
                                    .add_orphan(cycles, size, tx, out_points.to_owned())
                                    .is_some()
                                {
                                    self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
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
                                self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                            }
                        }
                    }
                    _ => {
                        debug_target!(
                            crate::LOG_TARGET_TX_POOL,
                            "Failed to add tx to {} {}, unknown reason: {:?}",
                            pool_name,
                            tx_hash,
                            err
                        );
                        self.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                    }
                }
                Err(err)
            }
        }
    }

    fn gap_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, Error> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(tx.clone())
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cycles).and_then(|cycles| {
                    let fee = self.calculate_transaction_fee(&rtx);
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
                let tx_hash = entry.transaction.hash();
                if tx_pool.add_gap(entry) {
                    Ok(())
                } else {
                    Err(InternalErrorKind::PoolTransactionDuplicated
                        .cause(tx_hash)
                        .into())
                }
            },
        )
    }

    pub(crate) fn proposed_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, Error> {
        let tx_result = self.resolve_tx_from_proposed(tx.clone()).and_then(|rtx| {
            self.verify_rtx(&rtx, cycles).and_then(|cycles| {
                let fee = self.calculate_transaction_fee(&rtx);
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
                tx_pool.add_proposed(cycles, fee, size, tx, related_dep_out_points);
                Ok(())
            },
        )
    }

    fn pending_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, Error> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(tx.clone())
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cycles).and_then(|cycles| {
                    let fee = self.calculate_transaction_fee(&rtx);
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
                let tx_hash = entry.transaction.hash();
                if tx_pool.enqueue_tx(entry) {
                    Ok(())
                } else {
                    Err(InternalErrorKind::PoolTransactionDuplicated
                        .cause(tx_hash)
                        .into())
                }
            },
        )
    }

    pub(crate) fn proposed_tx_and_descendants(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: TransactionView,
    ) -> Result<Cycle, Error> {
        self.proposed_tx(cycles, size, tx.clone()).map(|cycles| {
            self.try_proposed_orphan_by_ancestor(&tx);
            cycles
        })
    }

    fn readd_dettached_tx(
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
