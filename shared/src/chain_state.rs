use crate::cell_set::{CellSet, CellSetDiff, CellSetOpr, CellSetOverlay};
use crate::error::SharedError;
use crate::fee_rate::FeeRate;
use crate::tx_pool::types::{DefectEntry, TxEntry};
use crate::tx_pool::{PoolError, TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_dao::DaoCalculator;
use ckb_logger::{debug_target, error_target, info_target, trace_target};
use ckb_script::ScriptConfig;
use ckb_store::{ChainDB, ChainStore, StoreTransaction};
use ckb_traits::BlockMedianTimeContext;
use ckb_tx_cache::{TxCache, TxCacheItem};
use ckb_types::{
    core::{
        cell::{
            get_related_dep_out_points, resolve_transaction, CellProvider, CellStatus,
            HeaderChecker, OverlayCellProvider, ResolvedTransaction, UnresolvableError,
        },
        BlockNumber, BlockView, Capacity, Cycle, EpochExt, HeaderView, TransactionView,
    },
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
    H256, U256,
};
use ckb_util::LinkedFnvHashSet;
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use failure::Error as FailureError;
use std::cell::{Ref, RefCell};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub struct ChainState {
    store: Arc<ChainDB>,
    tip_header: HeaderView,
    total_difficulty: U256,
    pub(crate) cell_set: CellSet,
    proposal_ids: TxProposalTable,
    // interior mutability for immutable borrow proposal_ids
    tx_pool: RefCell<TxPool>,
    consensus: Arc<Consensus>,
    current_epoch_ext: EpochExt,
    script_config: ScriptConfig,
}

impl ChainState {
    pub fn init(
        store: &Arc<ChainDB>,
        consensus: Arc<Consensus>,
        tx_pool_config: TxPoolConfig,
        script_config: ScriptConfig,
    ) -> Result<Self, SharedError> {
        // check head in store or save the genesis block as head
        let (tip_header, epoch_ext) = {
            match store
                .get_tip_header()
                .and_then(|header| store.get_current_epoch_ext().map(|epoch| (header, epoch)))
            {
                Some((tip_header, epoch)) => {
                    if let Some(genesis_hash) = store.get_block_hash(0) {
                        let expect_genesis_hash = consensus.genesis_hash();
                        let genesis_hash: H256 = genesis_hash.unpack();
                        if &genesis_hash == expect_genesis_hash {
                            Ok((tip_header, epoch))
                        } else {
                            Err(SharedError::InvalidData(format!(
                                "mismatch genesis hash, expect {:#x} but {:#x} in database",
                                expect_genesis_hash, genesis_hash
                            )))
                        }
                    } else {
                        Err(SharedError::InvalidData(
                            "the genesis hash was not found".to_owned(),
                        ))
                    }
                }
                None => store
                    .init(&consensus)
                    .map_err(|e| {
                        SharedError::InvalidData(format!("failed to init genesis block {:?}", e))
                    })
                    .map(|_| {
                        (
                            consensus.genesis_block().header().to_owned(),
                            consensus.genesis_epoch_ext().to_owned(),
                        )
                    }),
            }
        }?;

        let tx_pool = TxPool::new(tx_pool_config);
        let tip_number = tip_header.number();
        let proposal_window = consensus.tx_proposal_window();
        let proposal_ids = Self::init_proposal_ids(&store, proposal_window, tip_number);

        let cell_set = Self::init_cell_set(&store)
            .map_err(|e| SharedError::InvalidData(format!("failed to load cell set{:?}", e)))?;

        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .ok_or_else(|| SharedError::InvalidData("failed to get block_ext".to_owned()))?
            .total_difficulty;
        Ok(ChainState {
            store: Arc::clone(store),
            tip_header,
            total_difficulty,
            cell_set,
            proposal_ids,
            tx_pool: RefCell::new(tx_pool),
            consensus,
            current_epoch_ext: epoch_ext,
            script_config,
        })
    }

    pub fn store(&self) -> &ChainDB {
        &self.store
    }

    pub(crate) fn init_proposal_ids(
        store: &ChainDB,
        proposal_window: ProposalWindow,
        tip_number: u64,
    ) -> TxProposalTable {
        let mut proposal_ids = TxProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.farthest());
        for bn in proposal_start..=tip_number {
            if let Some(hash) = store.get_block_hash(bn) {
                let mut ids_set = HashSet::default();
                if let Some(ids) = store.get_block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = store.get_block_uncles(&hash) {
                    for u in us.data().into_iter() {
                        ids_set.extend(u.proposals().into_iter());
                    }
                }
                proposal_ids.insert(bn, ids_set);
            }
        }
        proposal_ids.finalize(tip_number);
        proposal_ids
    }

    fn init_cell_set(store: &ChainDB) -> Result<CellSet, FailureError> {
        let mut cell_set = CellSet::new();
        let mut count = 0;
        info_target!(crate::LOG_TARGET_CHAIN, "Start: loading live cells ...");
        store.traverse_cell_set(|tx_hash, tx_meta| {
            count += 1;
            cell_set.put(tx_hash.unpack(), tx_meta.unpack());
            if count % 10_000 == 0 {
                info_target!(
                    crate::LOG_TARGET_CHAIN,
                    "    loading {} transactions which include live cells ...",
                    count
                );
            }
            Ok(())
        })?;
        info_target!(
            crate::LOG_TARGET_CHAIN,
            "Done: total {} transactions.",
            count
        );
        Ok(cell_set)
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    pub fn tip_hash(&self) -> Byte32 {
        self.tip_header.hash()
    }

    pub fn current_epoch_ext(&self) -> &EpochExt {
        &self.current_epoch_ext
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn tip_header(&self) -> &HeaderView {
        &self.tip_header
    }

    pub fn cell_set(&self) -> &CellSet {
        &self.cell_set
    }

    pub fn script_config(&self) -> &ScriptConfig {
        &self.script_config
    }

    pub fn proposal_ids(&self) -> &TxProposalTable {
        &self.proposal_ids
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.proposal_ids.contains(id)
    }

    pub fn contains_gap(&self, id: &ProposalShortId) -> bool {
        self.proposal_ids.contains_gap(id)
    }

    pub fn insert_proposal_ids(&mut self, block: &BlockView) {
        self.proposal_ids
            .insert(block.header().number(), block.union_proposal_ids());
    }

    pub fn remove_proposal_ids(&mut self, block: &BlockView) {
        self.proposal_ids.remove(block.header().number());
    }

    pub fn get_proposal_ids_iter(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.proposal_ids.get_ids_iter()
    }

    pub fn proposal_ids_finalize(&mut self, number: BlockNumber) -> HashSet<ProposalShortId> {
        self.proposal_ids.finalize(number)
    }

    pub fn update_current_epoch_ext(&mut self, epoch_ext: EpochExt) {
        self.current_epoch_ext = epoch_ext;
    }

    pub fn update_cell_set(
        &mut self,
        txo_diff: CellSetDiff,
        txn: &StoreTransaction,
    ) -> Result<(), FailureError> {
        let CellSetDiff {
            old_inputs,
            old_outputs,
            new_inputs,
            new_outputs,
        } = txo_diff;

        // The order is important, do NOT change them, unlese you know them clearly.

        let updated_old_inputs = old_inputs
            .into_iter()
            .filter(|out_point| !out_point.is_null())
            .filter_map(|out_point| {
                // if old_input reference the old_output, skip.
                if !old_outputs.contains(&out_point.tx_hash()) {
                    if let Some(tx_meta) = self.cell_set.try_mark_live(&out_point) {
                        Some((out_point.tx_hash(), tx_meta))
                    } else {
                        let ret = self.store.get_transaction(&out_point.tx_hash());
                        if ret.is_none() {
                            info_target!(
                                crate::LOG_TARGET_CHAIN,
                                "[update_tip] get_transaction out_point={}",
                                out_point,
                            );
                        }
                        let (tx, block_hash) = ret.expect("we should have this transaction");
                        let block = self
                            .store
                            .get_block(&block_hash)
                            .expect("we should have this block");
                        let cellbase = block.transactions()[0].hash() == tx.hash();
                        let tx_meta = self.cell_set.insert_cell(
                            &out_point,
                            block.number(),
                            block.epoch(),
                            block.hash().unpack(),
                            cellbase,
                            tx.outputs().len(),
                        );
                        Some((out_point.tx_hash(), tx_meta))
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let removed_old_outputs = old_outputs
            .into_iter()
            .filter_map(|tx_hash| self.cell_set.remove(&tx_hash.unpack()).map(|_| tx_hash))
            .collect::<Vec<_>>();

        let inserted_new_outputs = new_outputs
            .into_iter()
            .map(|(tx_hash, (number, epoch, hash, cellbase, len))| {
                let tx_meta = self.cell_set.insert_transaction(
                    tx_hash.unpack(),
                    number,
                    epoch,
                    hash.unpack(),
                    cellbase,
                    len,
                );
                (tx_hash, tx_meta)
            })
            .collect::<Vec<_>>();

        let mut updated_new_inputs = Vec::new();
        let mut removed_new_inputs = Vec::new();
        new_inputs
            .into_iter()
            .filter(|out_point| !out_point.is_null())
            .for_each(|out_point| {
                if let Some(opr) = self.cell_set.mark_dead(&out_point) {
                    match opr {
                        CellSetOpr::Delete => removed_new_inputs.push(out_point.tx_hash()),
                        CellSetOpr::Update(tx_meta) => {
                            updated_new_inputs.push((out_point.tx_hash(), tx_meta))
                        }
                    }
                }
            });

        for (tx_hash, tx_meta) in updated_old_inputs.iter() {
            txn.update_cell_set(&tx_hash, &tx_meta.pack())?;
        }
        for tx_hash in removed_old_outputs.iter() {
            txn.delete_cell_set(&tx_hash)?;
        }
        for (tx_hash, tx_meta) in inserted_new_outputs.iter() {
            txn.update_cell_set(&tx_hash, &tx_meta.pack())?;
        }
        for (tx_hash, tx_meta) in updated_new_inputs.iter() {
            txn.update_cell_set(&tx_hash, &tx_meta.pack())?;
        }
        for tx_hash in removed_new_inputs.iter() {
            txn.delete_cell_set(&tx_hash)?;
        }
        Ok(())
    }

    pub fn update_tip(
        &mut self,
        header: HeaderView,
        total_difficulty: U256,
    ) -> Result<(), FailureError> {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        Ok(())
    }

    pub fn get_tx_with_cycles_from_pool(
        &self,
        short_id: &ProposalShortId,
    ) -> Option<(TransactionView, Option<TxCacheItem>)> {
        self.tx_pool.borrow().get_tx_with_cycles(short_id)
    }

    pub(crate) fn reach_tx_pool_limit(&self, tx_size: usize, cycles: Cycle) -> bool {
        let tx_pool = self.tx_pool.borrow();
        tx_pool.reach_size_limit(tx_size) || tx_pool.reach_cycles_limit(cycles)
    }

    // Add a verified tx into pool
    // this method will handle fork related verifications to make sure we are safe during a fork
    pub fn add_tx_to_pool(
        &self,
        tx: TransactionView,
        cycles: Cycle,
        fee: Capacity,
    ) -> Result<Cycle, PoolError> {
        let short_id = tx.proposal_short_id();
        let tx_size = tx.serialized_size();
        if self.reach_tx_pool_limit(tx_size, cycles) {
            return Err(PoolError::LimitReached);
        }
        let resolve_result = {
            let tx_pool = self.tx_pool.borrow();
            self.resolve_tx_from_pending_and_proposed(&tx, &tx_pool)
        };
        match resolve_result {
            Ok(rtx) => {
                self.verify_rtx(&rtx, Some(cycles)).and_then(|cycles| {
                    let mut tx_pool = self.tx_pool.borrow_mut();
                    if self.contains_proposal_id(&short_id) {
                        // if tx is proposed, we resolve from proposed, verify again
                        if let Err(e) = self.proposed_tx_and_descendants(
                            &mut tx_pool,
                            Some(TxCacheItem::new(cycles, fee)),
                            tx_size,
                            tx,
                        ) {
                            debug_target!(
                                crate::LOG_TARGET_TX_POOL,
                                "Failed to add proposed tx {:?}, reason: {:?}",
                                short_id,
                                e
                            );
                            return Err(e);
                        }
                        tx_pool.update_statics_for_add_tx(tx_size, cycles);
                        return Ok(cycles);
                    }
                    if let Err(e) = self.pending_tx(
                        &mut tx_pool,
                        Some(TxCacheItem::new(cycles, fee)),
                        tx_size,
                        tx,
                    ) {
                        return Err(e);
                    } else {
                        tx_pool.update_statics_for_add_tx(tx_size, cycles);
                        return Ok(cycles);
                    }
                })
            }
            Err(err) => Err(PoolError::UnresolvableTransaction(err)),
        }
    }

    pub fn resolve_tx_from_pending_and_proposed<'b>(
        &self,
        tx: &'b TransactionView,
        tx_pool: &TxPool,
    ) -> Result<ResolvedTransaction<'b>, UnresolvableError> {
        let proposed_provider = OverlayCellProvider::new(&tx_pool.proposed, self);
        let gap_and_proposed_provider = OverlayCellProvider::new(&tx_pool.gap, &proposed_provider);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&tx_pool.pending, &gap_and_proposed_provider);
        let mut seen_inputs: HashSet<OutPoint> = HashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &pending_and_proposed_provider, self)
    }

    pub fn resolve_tx_from_proposed<'a>(
        &self,
        tx: &'a TransactionView,
        tx_pool: &TxPool,
    ) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
        let cell_provider = OverlayCellProvider::new(&tx_pool.proposed, self);
        let mut seen_inputs: HashSet<OutPoint> = HashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &cell_provider, self)
    }

    pub(crate) fn verify_rtx(
        &self,
        rtx: &ResolvedTransaction,
        cycles: Option<Cycle>,
    ) -> Result<Cycle, PoolError> {
        match cycles {
            Some(cycles) => {
                ContextualTransactionVerifier::new(
                    &rtx,
                    &self,
                    self.tip_number() + 1,
                    self.current_epoch_ext().number(),
                    self.tip_hash(),
                    &self.consensus(),
                )
                .verify()
                .map_err(PoolError::InvalidTx)?;
                Ok(cycles)
            }
            None => {
                let max_cycles = self.consensus.max_block_cycles();
                let cycles = TransactionVerifier::new(
                    &rtx,
                    &self,
                    self.tip_number() + 1,
                    self.current_epoch_ext().number(),
                    self.tip_hash(),
                    &self.consensus(),
                    &self.script_config,
                    self.store(),
                )
                .verify(max_cycles)
                .map_err(PoolError::InvalidTx)?;
                Ok(cycles)
            }
        }
    }

    // remove resolved tx from orphan pool
    pub(crate) fn try_proposed_orphan_by_ancestor(
        &self,
        tx_pool: &mut TxPool,
        tx: &TransactionView,
    ) {
        let entries = tx_pool.orphan.remove_by_ancestor(tx);
        for entry in entries {
            let tx_hash = entry.transaction.hash().to_owned();
            if self.contains_proposal_id(&tx.proposal_short_id()) {
                let ret = self.proposed_tx(tx_pool, entry.tx_cache, entry.size, entry.transaction);
                if ret.is_err() {
                    tx_pool.update_statics_for_remove_tx(
                        entry.size,
                        entry.tx_cache.map(|c| c.cycles).unwrap_or(0),
                    );
                    trace_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "proposed tx {} failed {:?}",
                        tx_hash,
                        ret
                    );
                }
            } else {
                let ret = self.pending_tx(tx_pool, entry.tx_cache, entry.size, entry.transaction);
                if ret.is_err() {
                    tx_pool.update_statics_for_remove_tx(
                        entry.size,
                        entry.tx_cache.map(|c| c.cycles).unwrap_or(0),
                    );
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

    fn calculate_transaction_fee(&self, rtx: &ResolvedTransaction) -> Result<Capacity, PoolError> {
        DaoCalculator::new(&self.consensus, self.store())
            .transaction_fee(&rtx)
            .map_err(|err| {
                error_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to generate tx fee for {}, reason: {:?}",
                    rtx.transaction.hash(),
                    err
                );
                PoolError::TxFee
            })
    }

    fn handle_tx_by_resolved_result<F>(
        pool_name: &str,
        tx_pool: &mut TxPool,
        tx_cache: Option<TxCacheItem>,
        size: usize,
        tx: TransactionView,
        tx_resolved_result: Result<(Cycle, Capacity, Vec<OutPoint>), PoolError>,
        add_to_pool: F,
    ) -> Result<TxCacheItem, PoolError>
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
                add_to_pool(tx_pool, cycles, fee, size, related_dep_out_points, tx)?;
                Ok(TxCacheItem::new(cycles, fee))
            }
            Err(PoolError::InvalidTx(e)) => {
                tx_pool.update_statics_for_remove_tx(size, tx_cache.map(|c| c.cycles).unwrap_or(0));
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
                        if tx_pool
                            .conflict
                            .insert(short_id, DefectEntry::new(tx, 0, tx_cache, size))
                            .is_some()
                        {
                            tx_pool.update_statics_for_remove_tx(
                                size,
                                tx_cache.map(|c| c.cycles).unwrap_or(0),
                            );
                        }
                    }
                    UnresolvableError::Unknown(out_points) => {
                        if tx_pool
                            .add_orphan(tx_cache, size, tx, out_points.to_owned())
                            .is_some()
                        {
                            tx_pool.update_statics_for_remove_tx(
                                size,
                                tx_cache.map(|c| c.cycles).unwrap_or(0),
                            );
                        }
                    }
                    // The remaining errors are InvalidHeader/InvalidDepGroup.
                    // They all represent invalid transactions
                    // that should just be discarded.
                    // OutOfOrder should only appear in BlockCellProvider
                    UnresolvableError::InvalidDepGroup(_)
                    | UnresolvableError::InvalidHeader(_)
                    | UnresolvableError::OutOfOrder(_) => {
                        tx_pool.update_statics_for_remove_tx(
                            size,
                            tx_cache.map(|c| c.cycles).unwrap_or(0),
                        );
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
                tx_pool.update_statics_for_remove_tx(size, tx_cache.map(|c| c.cycles).unwrap_or(0));
                Err(err)
            }
        }
    }

    fn gap_tx(
        &self,
        tx_pool: &mut TxPool,
        tx_cache: Option<TxCacheItem>,
        size: usize,
        tx: TransactionView,
    ) -> Result<TxCacheItem, PoolError> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(&tx, tx_pool)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, tx_cache.map(|c| c.cycles))
                    .and_then(|cycles| {
                        let fee = tx_cache
                            .map(|c| c.fee)
                            .ok_or(PoolError::TxFee)
                            .or_else(|_| self.calculate_transaction_fee(&rtx));
                        let related_dep_out_points = rtx.related_dep_out_points();
                        fee.map(|fee| (cycles, fee, related_dep_out_points))
                    })
            });
        Self::handle_tx_by_resolved_result(
            "gap",
            tx_pool,
            tx_cache,
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
        &self,
        tx_pool: &mut TxPool,
        tx_cache: Option<TxCacheItem>,
        size: usize,
        tx: TransactionView,
    ) -> Result<TxCacheItem, PoolError> {
        let tx_result = self
            .resolve_tx_from_proposed(&tx, tx_pool)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, tx_cache.map(|c| c.cycles))
                    .and_then(|cycles| {
                        let fee = tx_cache
                            .map(|c| c.fee)
                            .ok_or(PoolError::TxFee)
                            .or_else(|_| self.calculate_transaction_fee(&rtx));
                        let related_dep_out_points = rtx.related_dep_out_points();
                        fee.map(|fee| (cycles, fee, related_dep_out_points))
                    })
            });
        Self::handle_tx_by_resolved_result(
            "proposed",
            tx_pool,
            tx_cache,
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
        &self,
        tx_pool: &mut TxPool,
        tx_cache: Option<TxCacheItem>,
        size: usize,
        tx: TransactionView,
    ) -> Result<TxCacheItem, PoolError> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(&tx, tx_pool)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then({
                |rtx| {
                    self.verify_rtx(&rtx, tx_cache.map(|c| c.cycles))
                        .and_then(|cycles| {
                            let fee = tx_cache
                                .map(|c| c.fee)
                                .ok_or(PoolError::TxFee)
                                .or_else(|_| self.calculate_transaction_fee(&rtx));
                            let related_dep_out_points = rtx.related_dep_out_points();
                            fee.map(|fee| (cycles, fee, related_dep_out_points))
                        })
                }
            });
        Self::handle_tx_by_resolved_result(
            "pending",
            tx_pool,
            tx_cache,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, related_dep_out_points, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size, related_dep_out_points);
                if tx_pool.enqueue_tx(entry) {
                    Ok(())
                } else {
                    Err(PoolError::Duplicate)
                }
            },
        )
    }

    pub(crate) fn proposed_tx_and_descendants(
        &self,
        tx_pool: &mut TxPool,
        tx_cache: Option<TxCacheItem>,
        size: usize,
        tx: TransactionView,
    ) -> Result<TxCacheItem, PoolError> {
        self.proposed_tx(tx_pool, tx_cache, size, tx.clone())
            .map(|tx_cache| {
                self.try_proposed_orphan_by_ancestor(tx_pool, &tx);
                tx_cache
            })
    }

    pub fn update_tx_pool_for_reorg<'a>(
        &self,
        detached_blocks: impl Iterator<Item = &'a BlockView>,
        attached_blocks: impl Iterator<Item = &'a BlockView>,
        detached_proposal_id: impl Iterator<Item = &'a ProposalShortId>,
        txs_verify_cache: &mut TxCache,
    ) {
        fn readd_dettached_tx(
            chain_state: &ChainState,
            tx_pool: &mut TxPool,
            txs_verify_cache: &mut TxCache,
            tx: TransactionView,
        ) {
            let tx_hash = tx.hash().to_owned();
            let tx_cache = txs_verify_cache.get(&tx_hash).cloned();
            let tx_short_id = tx.proposal_short_id();
            let tx_size = tx.serialized_size();
            if chain_state.contains_proposal_id(&tx_short_id) {
                if let Ok(new_tx_cache) =
                    chain_state.proposed_tx_and_descendants(tx_pool, tx_cache, tx_size, tx)
                {
                    if tx_cache.is_none() {
                        txs_verify_cache.insert(tx_hash, new_tx_cache);
                    }
                    tx_pool.update_statics_for_add_tx(tx_size, new_tx_cache.cycles);
                }
            } else if chain_state.contains_gap(&tx_short_id) {
                if let Ok(new_tx_cache) = chain_state.gap_tx(tx_pool, tx_cache, tx_size, tx) {
                    if tx_cache.is_none() {
                        txs_verify_cache.insert(tx_hash, new_tx_cache);
                    }
                    tx_pool.update_statics_for_add_tx(tx_size, new_tx_cache.cycles);
                }
            } else if let Ok(new_tx_cache) = chain_state.pending_tx(tx_pool, tx_cache, tx_size, tx)
            {
                if tx_cache.is_none() {
                    txs_verify_cache.insert(tx_hash, new_tx_cache);
                }
                tx_pool.update_statics_for_add_tx(tx_size, new_tx_cache.cycles);
            }
        }
        let mut tx_pool = self.tx_pool.borrow_mut();
        let mut detached = LinkedFnvHashSet::default();
        let mut attached = LinkedFnvHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().iter().skip(1).cloned())
        }

        let retain: Vec<TransactionView> = detached.difference(&attached).cloned().collect();

        let txs_iter = attached.iter().map(|tx| {
            let get_cell_data = |out_point: &OutPoint| {
                self.store
                    .get_cell_data(&out_point.tx_hash(), out_point.index().unpack())
            };
            let related_out_points =
                get_related_dep_out_points(tx, get_cell_data).expect("Get dep out points failed");
            (tx, related_out_points)
        });
        tx_pool.remove_expired(detached_proposal_id);
        tx_pool.remove_committed_txs_from_proposed(txs_iter);

        for tx in retain {
            readd_dettached_tx(self, &mut tx_pool, txs_verify_cache, tx);
        }

        for tx in &attached {
            self.try_proposed_orphan_by_ancestor(&mut tx_pool, tx);
        }

        let mut entries = Vec::new();
        let mut gaps = Vec::new();

        // pending ---> gap ----> proposed
        // try move gap to proposed
        let mut removed: Vec<ProposalShortId> = Vec::with_capacity(tx_pool.gap.size());
        for key in tx_pool.gap.sorted_keys() {
            if self.contains_proposal_id(&key.id) {
                let entry = tx_pool.gap.get(&key.id).expect("exists");
                removed.push(key.id.clone());
                entries.push((
                    Some(TxCacheItem::new(entry.cycles, entry.fee)),
                    entry.size,
                    entry.transaction.to_owned(),
                ));
            }
        }
        removed.into_iter().for_each(|id| {
            tx_pool.gap.remove_entry_and_descendants(&id);
        });

        // try move pending to proposed
        let mut removed: Vec<ProposalShortId> = Vec::with_capacity(tx_pool.pending.size());
        for key in tx_pool.pending.sorted_keys() {
            let entry = tx_pool.pending.get(&key.id).expect("exists");
            if self.contains_proposal_id(&key.id) {
                removed.push(key.id.clone());
                entries.push((
                    Some(TxCacheItem::new(entry.cycles, entry.fee)),
                    entry.size,
                    entry.transaction.to_owned(),
                ));
            } else if self.contains_gap(&key.id) {
                removed.push(key.id.clone());
                gaps.push((
                    Some(TxCacheItem::new(entry.cycles, entry.fee)),
                    entry.size,
                    entry.transaction.to_owned(),
                ));
            }
        }
        removed.into_iter().for_each(|id| {
            tx_pool.pending.remove_entry_and_descendants(&id);
        });

        // try move conflict to proposed
        for entry in tx_pool.conflict.entries() {
            if self.contains_proposal_id(entry.key()) {
                let entry = entry.remove();
                entries.push((entry.tx_cache, entry.size, entry.transaction));
            } else if self.contains_gap(entry.key()) {
                let entry = entry.remove();
                gaps.push((entry.tx_cache, entry.size, entry.transaction));
            }
        }

        for (tx_cache, size, tx) in entries {
            let tx_hash = tx.hash();
            if let Err(e) = self.proposed_tx_and_descendants(&mut tx_pool, tx_cache, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add proposed tx {}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }

        for (tx_cache, size, tx) in gaps {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "tx proposed, add to gap {}",
                tx.hash()
            );
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.gap_tx(&mut tx_pool, tx_cache, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to gap {}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.borrow().last_txs_updated_at
    }

    pub fn get_proposals(&self, limit: usize, min_fee_rate: FeeRate) -> HashSet<ProposalShortId> {
        let tx_pool = self.tx_pool.borrow();
        let mut proposals = HashSet::with_capacity(limit);
        tx_pool
            .pending
            .fill_proposals(limit, min_fee_rate, &mut proposals);
        tx_pool
            .gap
            .fill_proposals(limit, min_fee_rate, &mut proposals);
        proposals
    }

    pub fn tx_pool(&self) -> Ref<TxPool> {
        self.tx_pool.borrow()
    }

    pub fn mut_tx_pool(&mut self) -> &mut TxPool {
        self.tx_pool.get_mut()
    }

    pub fn get_tx_from_pool_or_store(
        &self,
        proposal_id: &ProposalShortId,
    ) -> Option<TransactionView> {
        let tx_pool = self.tx_pool();
        tx_pool
            .get_tx_from_proposed_and_others(proposal_id)
            .or_else(|| {
                tx_pool
                    .committed_txs_hash_cache
                    .get(proposal_id)
                    .and_then(|tx_hash| self.store().get_transaction(tx_hash).map(|(tx, _)| tx))
            })
    }

    pub fn consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }

    pub fn new_cell_set_overlay<'a, CS: ChainStore<'a>>(
        &'a self,
        diff: &CellSetDiff,
        store: &'a CS,
    ) -> ChainCellSetOverlay<'a, CS> {
        ChainCellSetOverlay {
            overlay: self.cell_set.new_overlay(diff, store),
            store,
        }
    }
}

pub struct ChainCellSetOverlay<'a, CS> {
    pub(crate) overlay: CellSetOverlay<'a>,
    pub(crate) store: &'a CS,
}

impl CellProvider for ChainState {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.cell_set.get(&out_point.tx_hash().unpack()) {
            Some(tx_meta) => match tx_meta.is_dead(out_point.index().unpack()) {
                Some(false) => {
                    let mut cell_meta = self
                        .store
                        .get_cell_meta(&out_point.tx_hash(), out_point.index().unpack())
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data = self
                            .store
                            .get_cell_data(&out_point.tx_hash(), out_point.index().unpack());
                    }
                    CellStatus::live_cell(cell_meta)
                }
                Some(true) => CellStatus::Dead,
                None => CellStatus::Unknown,
            },
            None => CellStatus::Unknown,
        }
    }
}

impl HeaderChecker for ChainState {
    fn is_valid(&self, block_hash: &Byte32) -> bool {
        self.store.get_block_number(block_hash).is_some()
    }
}

impl<'a, CS: ChainStore<'a>> CellProvider for ChainCellSetOverlay<'a, CS> {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.overlay.get(&out_point.tx_hash()) {
            Some(tx_meta) => match tx_meta.is_dead(out_point.index().unpack()) {
                Some(false) => {
                    let mut cell_meta = self
                        .store
                        .get_cell_meta(&out_point.tx_hash(), out_point.index().unpack())
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data = self
                            .store
                            .get_cell_data(&out_point.tx_hash(), out_point.index().unpack());
                    }
                    CellStatus::live_cell(cell_meta)
                }
                Some(true) => CellStatus::Dead,
                None => CellStatus::Unknown,
            },
            None => CellStatus::Unknown,
        }
    }
}

impl BlockMedianTimeContext for &ChainState {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        let header = self
            .store
            .get_block_header(block_hash)
            .expect("[ChainState] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }
}
