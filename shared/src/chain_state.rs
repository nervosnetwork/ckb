use crate::cell_set::{CellSet, CellSetDiff, CellSetOpr, CellSetOverlay};
use crate::error::SharedError;
use crate::fee_estimator::Estimator;
use crate::fee_rate::FeeRate;
use crate::tx_pool::types::{DefectEntry, TxEntry};
use crate::tx_pool::{PoolError, TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, CellProvider, CellStatus, HeaderProvider, HeaderStatus,
    OverlayCellProvider, ResolvedTransaction, UnresolvableError,
};
use ckb_core::extras::EpochExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::{Capacity, Cycle};
use ckb_dao::DaoCalculator;
use ckb_logger::{debug_target, error_target, info_target, trace_target};
use ckb_script::ScriptConfig;
use ckb_store::{ChainDB, ChainStore, StoreTransaction};
use ckb_traits::BlockMedianTimeContext;
use ckb_util::LinkedFnvHashSet;
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use failure::Error as FailureError;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cell::{Ref, RefCell};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub struct ChainState {
    store: Arc<ChainDB>,
    tip_header: Header,
    total_difficulty: U256,
    pub(crate) cell_set: CellSet,
    proposal_ids: TxProposalTable,
    // interior mutability for immutable borrow proposal_ids
    tx_pool: RefCell<TxPool>,
    consensus: Arc<Consensus>,
    current_epoch_ext: EpochExt,
    script_config: ScriptConfig,
    fee_estimator: RefCell<Estimator>,
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

        let fee_estimator = RefCell::new(Estimator::new());
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
            fee_estimator,
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
                    for u in us {
                        ids_set.extend(u.proposals);
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
            cell_set.put(tx_hash, tx_meta);
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

    pub fn tip_hash(&self) -> &H256 {
        self.tip_header.hash()
    }

    pub fn current_epoch_ext(&self) -> &EpochExt {
        &self.current_epoch_ext
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn tip_header(&self) -> &Header {
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

    pub fn insert_proposal_ids(&mut self, block: &Block) {
        self.proposal_ids
            .insert(block.header().number(), block.union_proposal_ids());
    }

    pub fn remove_proposal_ids(&mut self, block: &Block) {
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
            .filter_map(|out_point| {
                out_point.cell.and_then(|cell| {
                    // if old_input reference the old_output, skip.
                    if !old_outputs.contains(&cell.tx_hash) {
                        if let Some(tx_meta) = self.cell_set.try_mark_live(&cell) {
                            Some((cell.tx_hash, tx_meta))
                        } else {
                            let ret = self.store.get_transaction(&cell.tx_hash);
                            if ret.is_none() {
                                info_target!(
                                    crate::LOG_TARGET_CHAIN,
                                    "[update_tip] get_transaction error tx_hash {:x} cell {:?}",
                                    &cell.tx_hash,
                                    cell,
                                );
                            }
                            let (tx, block_hash) = ret.expect("we should have this transaction");
                            let block = self
                                .store
                                .get_block(&block_hash)
                                .expect("we should have this block");
                            let cellbase = block.transactions()[0].hash() == tx.hash();
                            let tx_meta = self.cell_set.insert_cell(
                                &cell,
                                block.header().number(),
                                block.header().epoch(),
                                block.header().hash().to_owned(),
                                cellbase,
                                tx.outputs().len(),
                            );
                            Some((cell.tx_hash, tx_meta))
                        }
                    } else {
                        None
                    }
                })
            })
            .collect::<Vec<_>>();

        let removed_old_outputs = old_outputs
            .into_iter()
            .filter_map(|tx_hash| self.cell_set.remove(&tx_hash).map(|_| tx_hash))
            .collect::<Vec<_>>();

        let inserted_new_outputs = new_outputs
            .into_iter()
            .map(|(tx_hash, (number, epoch, hash, cellbase, len))| {
                let tx_meta = self.cell_set.insert_transaction(
                    tx_hash.to_owned(),
                    number,
                    epoch,
                    hash,
                    cellbase,
                    len,
                );
                (tx_hash, tx_meta)
            })
            .collect::<Vec<_>>();

        let mut updated_new_inputs = Vec::new();
        let mut removed_new_inputs = Vec::new();
        new_inputs.into_iter().for_each(|out_point| {
            out_point.cell.and_then(|cell| {
                self.cell_set.mark_dead(&cell).map(|opr| match opr {
                    CellSetOpr::Delete => removed_new_inputs.push(cell.tx_hash),
                    CellSetOpr::Update(tx_meta) => updated_new_inputs.push((cell.tx_hash, tx_meta)),
                })
            });
        });

        for (tx_hash, tx_meta) in updated_old_inputs.iter() {
            txn.update_cell_set(&tx_hash, &tx_meta)?;
        }
        for tx_hash in removed_old_outputs.iter() {
            txn.delete_cell_set(&tx_hash)?;
        }
        for (tx_hash, tx_meta) in inserted_new_outputs.iter() {
            txn.update_cell_set(&tx_hash, &tx_meta)?;
        }
        for (tx_hash, tx_meta) in updated_new_inputs.iter() {
            txn.update_cell_set(&tx_hash, &tx_meta)?;
        }
        for tx_hash in removed_new_inputs.iter() {
            txn.delete_cell_set(&tx_hash)?;
        }
        Ok(())
    }

    pub fn update_tip(
        &mut self,
        header: Header,
        total_difficulty: U256,
    ) -> Result<(), FailureError> {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        Ok(())
    }

    pub fn get_tx_with_cycles_from_pool(
        &self,
        short_id: &ProposalShortId,
    ) -> Option<(Transaction, Option<(Cycle, Capacity)>)> {
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
        tx: Transaction,
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
                            Some((cycles, fee)),
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
                    } else if self
                        .pending_tx(&mut tx_pool, Some((cycles, fee)), tx_size, tx)
                        .is_ok()
                    {
                        tx_pool.update_statics_for_add_tx(tx_size, cycles);
                    }
                    Ok(cycles)
                })
            }
            Err(err) => Err(PoolError::UnresolvableTransaction(err)),
        }
    }

    pub fn resolve_tx_from_pending_and_proposed<'b>(
        &self,
        tx: &'b Transaction,
        tx_pool: &TxPool,
    ) -> Result<ResolvedTransaction<'b>, UnresolvableError> {
        let proposed_provider = OverlayCellProvider::new(&tx_pool.proposed, self);
        let gap_and_proposed_provider = OverlayCellProvider::new(&tx_pool.gap, &proposed_provider);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&tx_pool.pending, &gap_and_proposed_provider);
        let mut seen_inputs = HashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &pending_and_proposed_provider, self)
    }

    pub fn resolve_tx_from_proposed<'a>(
        &self,
        tx: &'a Transaction,
        tx_pool: &TxPool,
    ) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
        let cell_provider = OverlayCellProvider::new(&tx_pool.proposed, self);
        let mut seen_inputs = HashSet::default();
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
    pub(crate) fn try_proposed_orphan_by_ancestor(&self, tx_pool: &mut TxPool, tx: &Transaction) {
        let entries = tx_pool.orphan.remove_by_ancestor(tx);
        for entry in entries {
            let tx_hash = entry.transaction.hash().to_owned();
            if self.contains_proposal_id(&tx.proposal_short_id()) {
                let ret = self.proposed_tx(tx_pool, entry.cached, entry.size, entry.transaction);
                if ret.is_err() {
                    tx_pool.update_statics_for_remove_tx(
                        entry.size,
                        entry.cached.map(|c| c.0).unwrap_or(0),
                    );
                    trace_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "proposed tx {:x} failed {:?}",
                        tx_hash,
                        ret
                    );
                }
            } else {
                let ret = self.pending_tx(tx_pool, entry.cached, entry.size, entry.transaction);
                if ret.is_err() {
                    tx_pool.update_statics_for_remove_tx(
                        entry.size,
                        entry.cached.map(|c| c.0).unwrap_or(0),
                    );
                    trace_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "pending tx {:x} failed {:?}",
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
                    "Failed to generate tx fee for {:x}, reason: {:?}",
                    rtx.transaction.hash(),
                    err
                );
                PoolError::TxFee
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn send_tx_to_pool<F>(
        &self,
        pool_name: &str,
        tx_pool: &mut TxPool,
        cached: Option<(Cycle, Capacity)>,
        size: usize,
        tx: Transaction,
        tx_resolved_result: Result<(Cycle, Capacity), PoolError>,
        send_to_pool: F,
    ) -> Result<(Cycle, Capacity), PoolError>
    where
        F: FnOnce(&mut TxPool, Cycle, Capacity, usize, Transaction) -> Result<(), PoolError>,
    {
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash().to_owned();
        let mut tx_is_removed = false;

        let result = match tx_resolved_result {
            Ok((cycles, fee)) => {
                send_to_pool(tx_pool, cycles, fee, size, tx)?;
                Ok((cycles, fee))
            }
            Err(PoolError::InvalidTx(e)) => {
                tx_is_removed = true;
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to {} {:x}, verify failed, reason: {:?}",
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
                            .insert(short_id, DefectEntry::new(tx, 0, cached, size))
                            .is_some()
                        {
                            tx_is_removed = true;
                        }
                    }
                    UnresolvableError::Unknown(out_points) => {
                        if tx_pool
                            .add_orphan(cached, size, tx, out_points.to_owned())
                            .is_some()
                        {
                            tx_is_removed = true;
                        }
                    }
                    // The remaining errors are Empty, UnspecifiedInputCell and
                    // InvalidHeader. They all represent invalid transactions
                    // that should just be discarded.
                    // OutOfOrder should only appear in BlockCellProvider
                    UnresolvableError::Empty
                    | UnresolvableError::UnspecifiedInputCell(_)
                    | UnresolvableError::InvalidHeader(_)
                    | UnresolvableError::OutOfOrder(_) => {
                        tx_is_removed = true;
                    }
                }
                Err(PoolError::UnresolvableTransaction(err))
            }
            Err(err) => {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to {} {:x}, reason: {:?}",
                    pool_name,
                    tx_hash,
                    err
                );
                tx_is_removed = true;
                Err(err)
            }
        };
        if tx_is_removed {
            tx_pool.update_statics_for_remove_tx(size, cached.map(|c| c.0).unwrap_or(0));
            self.fee_estimator.borrow_mut().drop_tx(&tx_hash);
        }
        result
    }

    fn gap_tx(
        &self,
        tx_pool: &mut TxPool,
        cached: Option<(Cycle, Capacity)>,
        size: usize,
        tx: Transaction,
    ) -> Result<(Cycle, Capacity), PoolError> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(&tx, tx_pool)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cached.map(|c| c.0))
                    .and_then(|cycles| {
                        if let Some((_cycles, fee)) = cached {
                            Ok((cycles, fee))
                        } else {
                            let fee = self.calculate_transaction_fee(&rtx);
                            fee.map(|fee| (cycles, fee))
                        }
                    })
            });
        self.send_tx_to_pool(
            "gap",
            tx_pool,
            cached,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size);
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
        cached: Option<(Cycle, Capacity)>,
        size: usize,
        tx: Transaction,
    ) -> Result<(Cycle, Capacity), PoolError> {
        let tx_result = self
            .resolve_tx_from_proposed(&tx, tx_pool)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cached.map(|c| c.0))
                    .and_then(|cycles| {
                        if let Some((_cycles, fee)) = cached {
                            Ok((cycles, fee))
                        } else {
                            let fee = self.calculate_transaction_fee(&rtx);
                            fee.map(|fee| (cycles, fee))
                        }
                    })
            });
        self.send_tx_to_pool(
            "proposed",
            tx_pool,
            cached,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, tx| {
                tx_pool.add_proposed(cycles, fee, size, tx);
                Ok(())
            },
        )
    }

    fn pending_tx(
        &self,
        tx_pool: &mut TxPool,
        cached: Option<(Cycle, Capacity)>,
        size: usize,
        tx: Transaction,
    ) -> Result<(Cycle, Capacity), PoolError> {
        let tx_result = self
            .resolve_tx_from_pending_and_proposed(&tx, tx_pool)
            .map_err(PoolError::UnresolvableTransaction)
            .and_then(|rtx| {
                self.verify_rtx(&rtx, cached.map(|c| c.0))
                    .and_then(|cycles| {
                        let fee = self.calculate_transaction_fee(&rtx);
                        fee.map(|fee| (cycles, fee))
                    })
            });
        self.send_tx_to_pool(
            "pending",
            tx_pool,
            cached,
            size,
            tx,
            tx_result,
            |tx_pool, cycles, fee, size, tx| {
                let entry = TxEntry::new(tx, cycles, fee, size);
                let tx_hash = entry.transaction.hash().to_owned();
                let fee_rate = entry.fee_rate();
                if tx_pool.enqueue_tx(entry) {
                    self.fee_estimator
                        .borrow_mut()
                        .track_tx(tx_hash, fee_rate, self.tip_number());
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
        cached: Option<(Cycle, Capacity)>,
        size: usize,
        tx: Transaction,
    ) -> Result<(Cycle, Capacity), PoolError> {
        self.proposed_tx(tx_pool, cached, size, tx.clone())
            .map(|cached| {
                self.try_proposed_orphan_by_ancestor(tx_pool, &tx);
                cached
            })
    }

    pub fn update_tx_pool_for_reorg<'a>(
        &self,
        detached_blocks: impl Iterator<Item = &'a Block>,
        attached_blocks: impl Iterator<Item = &'a Block>,
        detached_proposal_id: impl Iterator<Item = &'a ProposalShortId>,
        txs_verify_cache: &mut LruCache<H256, (Cycle, Capacity)>,
    ) {
        fn readd_dettached_tx(
            chain_state: &ChainState,
            tx_pool: &mut TxPool,
            txs_verify_cache: &mut LruCache<H256, (Cycle, Capacity)>,
            tx: Transaction,
        ) {
            let tx_hash = tx.hash().to_owned();
            let cached = txs_verify_cache.get(&tx_hash).cloned();
            let tx_short_id = tx.proposal_short_id();
            let tx_size = tx.serialized_size();
            if chain_state.contains_proposal_id(&tx_short_id) {
                if let Ok((cycles, fee)) =
                    chain_state.proposed_tx_and_descendants(tx_pool, cached, tx_size, tx)
                {
                    if cached.is_none() {
                        txs_verify_cache.insert(tx_hash, (cycles, fee));
                    }
                    tx_pool.update_statics_for_add_tx(tx_size, cycles);
                }
            } else if chain_state.contains_gap(&tx_short_id) {
                if let Ok((cycles, fee)) = chain_state.gap_tx(tx_pool, cached, tx_size, tx) {
                    if cached.is_none() {
                        txs_verify_cache.insert(tx_hash, (cycles, fee));
                    }
                    tx_pool.update_statics_for_add_tx(tx_size, cycles);
                }
            } else if let Ok((cycles, fee)) = chain_state.pending_tx(tx_pool, cached, tx_size, tx) {
                if cached.is_none() {
                    txs_verify_cache.insert(tx_hash, (cycles, fee));
                }
                tx_pool.update_statics_for_add_tx(tx_size, cycles);
            }
        }
        let mut tx_pool = self.tx_pool.borrow_mut();
        let mut detached = LinkedFnvHashSet::default();
        let mut attached = LinkedFnvHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).cloned())
        }

        {
            let mut fee_estimator = self.fee_estimator.borrow_mut();
            for blk in attached_blocks {
                let txs_iter = blk.transactions().iter().skip(1);
                attached.extend(txs_iter.clone().cloned());
                fee_estimator.process_block(blk.header().number(), txs_iter.map(|tx| tx.hash()));
            }
        }

        let retain: Vec<Transaction> = detached.difference(&attached).cloned().collect();

        tx_pool.remove_expired(detached_proposal_id);
        tx_pool.remove_committed_txs_from_proposed(attached.iter());

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
        let mut removed = Vec::with_capacity(tx_pool.gap.size());
        for key in tx_pool.gap.sorted_keys() {
            if self.contains_proposal_id(&key.id) {
                let entry = tx_pool.gap.get(&key.id).expect("exists");
                removed.push(key.id);
                entries.push((
                    Some((entry.cycles, entry.fee)),
                    entry.size,
                    entry.transaction.to_owned(),
                ));
            }
        }
        removed.into_iter().for_each(|id| {
            tx_pool.gap.remove_entry_and_descendants(&id);
        });

        // try move pending to proposed
        let mut removed = Vec::with_capacity(tx_pool.pending.size());
        for key in tx_pool.pending.sorted_keys() {
            let id = &key.id;
            let entry = tx_pool.pending.get(&id).expect("exists");
            if self.contains_proposal_id(&id) {
                removed.push(*id);
                entries.push((
                    Some((entry.cycles, entry.fee)),
                    entry.size,
                    entry.transaction.to_owned(),
                ));
            } else if self.contains_gap(&id) {
                removed.push(*id);
                gaps.push((
                    Some((entry.cycles, entry.fee)),
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
                entries.push((entry.cached, entry.size, entry.transaction));
            } else if self.contains_gap(entry.key()) {
                let entry = entry.remove();
                gaps.push((entry.cached, entry.size, entry.transaction));
            }
        }

        for (cycles, size, tx) in entries {
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.proposed_tx_and_descendants(&mut tx_pool, cycles, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add proposed tx {:x}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }

        for (cached, size, tx) in gaps {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "tx proposed, add to gap {:x}",
                tx.hash()
            );
            let tx_hash = tx.hash().to_owned();
            if let Err(e) = self.gap_tx(&mut tx_pool, cached, size, tx) {
                debug_target!(
                    crate::LOG_TARGET_TX_POOL,
                    "Failed to add tx to gap {:x}, reason: {:?}",
                    tx_hash,
                    e
                );
            }
        }
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.borrow().last_txs_updated_at
    }

    pub fn get_proposals(
        &self,
        proposals_limit: usize,
        min_fee_rate: FeeRate,
    ) -> HashSet<ProposalShortId> {
        use crate::tx_pool::pending::PendingQueue;

        let fill_from_pool = |pool: &PendingQueue, proposals: &mut HashSet<ProposalShortId>| {
            for key in pool.sorted_keys() {
                if proposals.len() == proposals_limit {
                    break;
                } else if proposals.contains(&key.id)
                    || key.ancestors_fee < min_fee_rate.fee(key.ancestors_size)
                {
                    // skip tx if tx fee rate is lower than min fee rate
                    continue;
                }
                let mut ancestors = pool.get_ancestors(&key.id).into_iter().collect::<Vec<_>>();
                ancestors.sort_unstable_by_key(|id| {
                    pool.get(&id)
                        .map(|entry| entry.ancestors_count)
                        .expect("exists")
                });
                ancestors.push(key.id);
                proposals.extend(
                    ancestors
                        .into_iter()
                        .take(proposals_limit - proposals.len()),
                );
            }
        };

        let tx_pool = self.tx_pool.borrow();
        let mut proposals = HashSet::with_capacity(proposals_limit);
        fill_from_pool(&tx_pool.pending, &mut proposals);
        fill_from_pool(&tx_pool.gap, &mut proposals);
        proposals
    }

    pub fn tx_pool(&self) -> Ref<TxPool> {
        self.tx_pool.borrow()
    }

    pub fn mut_tx_pool(&mut self) -> &mut TxPool {
        self.tx_pool.get_mut()
    }

    pub fn fee_estimator(&self) -> Ref<Estimator> {
        self.fee_estimator.borrow()
    }

    pub fn get_tx_from_pool_or_store(&self, proposal_id: &ProposalShortId) -> Option<Transaction> {
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
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &out_point.cell {
            match self.cell_set.get(&cell_out_point.tx_hash) {
                Some(tx_meta) => match tx_meta.is_dead(cell_out_point.index as usize) {
                    Some(false) => {
                        let cell_meta = self
                            .store
                            .get_cell_meta(&cell_out_point.tx_hash, cell_out_point.index)
                            .expect("store should be consistent with cell_set");
                        CellStatus::live_cell(cell_meta)
                    }
                    Some(true) => CellStatus::Dead,
                    None => CellStatus::Unknown,
                },
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unspecified
        }
    }
}

impl HeaderProvider for ChainState {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        if let Some(block_hash) = &out_point.block_hash {
            match self.store.get_block_header(&block_hash) {
                Some(header) => {
                    if let Some(cell_out_point) = &out_point.cell {
                        self.store
                            .get_transaction_info(&cell_out_point.tx_hash)
                            .map_or(HeaderStatus::InclusionFaliure, |info| {
                                if info.block_hash == *block_hash {
                                    HeaderStatus::live_header(header)
                                } else {
                                    HeaderStatus::InclusionFaliure
                                }
                            })
                    } else {
                        HeaderStatus::live_header(header)
                    }
                }
                None => HeaderStatus::Unknown,
            }
        } else {
            HeaderStatus::Unspecified
        }
    }
}

impl<'a, CS: ChainStore<'a>> CellProvider for ChainCellSetOverlay<'a, CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &out_point.cell {
            match self.overlay.get(&cell_out_point.tx_hash) {
                Some(tx_meta) => match tx_meta.is_dead(cell_out_point.index as usize) {
                    Some(false) => {
                        let cell_meta = self
                            .store
                            .get_cell_meta(&cell_out_point.tx_hash, cell_out_point.index)
                            .expect("store should be consistent with cell_set");

                        CellStatus::live_cell(cell_meta)
                    }
                    Some(true) => CellStatus::Dead,
                    None => CellStatus::Unknown,
                },
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unspecified
        }
    }
}

impl BlockMedianTimeContext for &ChainState {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, BlockNumber, H256) {
        let header = self
            .store
            .get_block_header(&block_hash)
            .expect("[ChainState] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.parent_hash().to_owned(),
        )
    }
}
