use crate::cell_set::{CellSet, CellSetDiff, CellSetOpr, CellSetOverlay};
use crate::error::SharedError;
use crate::tx_pool::types::{DefectEntry, ProposedEntry};
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
use ckb_core::Cycle;
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
            .filter(|out_point| !out_point.is_null())
            .filter_map(|out_point| {
                // if old_input reference the old_output, skip.
                if !old_outputs.contains(&out_point.tx_hash) {
                    if let Some(tx_meta) = self.cell_set.try_mark_live(&out_point) {
                        Some((out_point.tx_hash, tx_meta))
                    } else {
                        let ret = self.store.get_transaction(&out_point.tx_hash);
                        if ret.is_none() {
                            info_target!(
                                crate::LOG_TARGET_CHAIN,
                                "[update_tip] get_transaction error tx_hash {:x} cell {:?}",
                                &out_point.tx_hash,
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
                            block.header().number(),
                            block.header().epoch(),
                            block.header().hash().to_owned(),
                            cellbase,
                            tx.outputs().len(),
                        );
                        Some((out_point.tx_hash, tx_meta))
                    }
                } else {
                    None
                }
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
        new_inputs
            .into_iter()
            .filter(|out_point| !out_point.is_null())
            .for_each(|out_point| {
                if let Some(opr) = self.cell_set.mark_dead(&out_point) {
                    match opr {
                        CellSetOpr::Delete => removed_new_inputs.push(out_point.tx_hash),
                        CellSetOpr::Update(tx_meta) => {
                            updated_new_inputs.push((out_point.tx_hash, tx_meta))
                        }
                    }
                }
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
    ) -> Option<(Transaction, Option<Cycle>)> {
        self.tx_pool.borrow().get_tx_with_cycles(short_id)
    }

    pub(crate) fn reach_tx_pool_limit(&self, tx_size: usize, cycles: Cycle) -> bool {
        let tx_pool = self.tx_pool.borrow();
        tx_pool.reach_size_limit(tx_size) || tx_pool.reach_cycles_limit(cycles)
    }

    // Add a verified tx into pool
    // this method will handle fork related verifications to make sure we are safe during a fork
    pub fn add_tx_to_pool(&self, tx: Transaction, cycles: Cycle) -> Result<Cycle, PoolError> {
        let short_id = tx.proposal_short_id();
        let tx_size = tx.serialized_size();
        if self.reach_tx_pool_limit(tx_size, cycles) {
            return Err(PoolError::LimitReached);
        }
        match self.resolve_tx_from_pending_and_proposed(&tx) {
            Ok(rtx) => {
                self.verify_rtx(&rtx, Some(cycles)).and_then(|cycles| {
                    let mut tx_pool = self.tx_pool.borrow_mut();
                    if self.contains_proposal_id(&short_id) {
                        // if tx is proposed, we resolve from proposed, verify again
                        if let Err(e) = self.proposed_tx_and_descendants(
                            &mut tx_pool,
                            Some(cycles),
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
                    } else if tx_pool.enqueue_tx(Some(cycles), tx_size, tx) {
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
    ) -> Result<ResolvedTransaction<'b>, UnresolvableError> {
        let tx_pool = self.tx_pool.borrow_mut();
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
            if self.contains_proposal_id(&tx.proposal_short_id()) {
                let tx_hash = entry.transaction.hash().to_owned();
                let ret = self.proposed_tx(tx_pool, entry.cycles, entry.size, entry.transaction);
                if ret.is_err() {
                    tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles.unwrap_or(0));
                    trace_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "proposed tx {:x} failed {:?}",
                        tx_hash,
                        ret
                    );
                }
            } else {
                tx_pool.enqueue_tx(entry.cycles, entry.size, entry.transaction);
            }
        }
    }

    pub(crate) fn proposed_tx(
        &self,
        tx_pool: &mut TxPool,
        cycles: Option<Cycle>,
        size: usize,
        tx: Transaction,
    ) -> Result<Cycle, PoolError> {
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        match self.resolve_tx_from_proposed(&tx, tx_pool) {
            Ok(rtx) => match self.verify_rtx(&rtx, cycles) {
                Ok(cycles) => {
                    let fee = DaoCalculator::new(&self.consensus, self.store())
                        .transaction_fee(&rtx)
                        .map_err(|e| {
                            error_target!(
                                crate::LOG_TARGET_TX_POOL,
                                "Failed to generate tx fee for {:x}, reason: {:?}",
                                tx_hash,
                                e
                            );
                            tx_pool.update_statics_for_remove_tx(size, cycles);
                            PoolError::TxFee
                        })?;

                    // Because we have dep group
                    let resolved_deps = rtx
                        .resolved_deps
                        .into_iter()
                        .filter_map(|dep| dep.destruct().0)
                        .map(|cell_meta| cell_meta.out_point)
                        .collect::<Vec<_>>();
                    tx_pool.add_proposed(cycles, fee, size, tx, resolved_deps);
                    Ok(cycles)
                }
                Err(e) => {
                    tx_pool.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                    debug_target!(
                        crate::LOG_TARGET_TX_POOL,
                        "Failed to add proposed tx {:x}, reason: {:?}",
                        tx_hash,
                        e
                    );
                    Err(e)
                }
            },
            Err(err) => {
                match &err {
                    UnresolvableError::Dead(_) => {
                        if tx_pool
                            .conflict
                            .insert(short_id, DefectEntry::new(tx, 0, cycles, size))
                            .is_some()
                        {
                            tx_pool.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                        }
                    }
                    UnresolvableError::Unknown(out_points) => {
                        if tx_pool
                            .add_orphan(cycles, size, tx, out_points.to_owned())
                            .is_some()
                        {
                            tx_pool.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                        }
                    }
                    // The remaining errors are Empty, UnspecifiedInputCell and
                    // InvalidHeader. They all represent invalid transactions
                    // that should just be discarded.
                    // OutOfOrder should only appear in BlockCellProvider
                    UnresolvableError::InvalidHeader(_, _)
                    | UnresolvableError::InvalidDepGroup(_)
                    | UnresolvableError::OutOfOrder(_) => {
                        tx_pool.update_statics_for_remove_tx(size, cycles.unwrap_or(0));
                    }
                }
                Err(PoolError::UnresolvableTransaction(err))
            }
        }
    }

    pub(crate) fn proposed_tx_and_descendants(
        &self,
        tx_pool: &mut TxPool,
        cycles: Option<Cycle>,
        size: usize,
        tx: Transaction,
    ) -> Result<Cycle, PoolError> {
        self.proposed_tx(tx_pool, cycles, size, tx.clone())
            .map(|cycles| {
                self.try_proposed_orphan_by_ancestor(tx_pool, &tx);
                cycles
            })
    }

    pub fn update_tx_pool_for_reorg<'a>(
        &self,
        detached_blocks: impl Iterator<Item = &'a Block>,
        attached_blocks: impl Iterator<Item = &'a Block>,
        detached_proposal_id: impl Iterator<Item = &'a ProposalShortId>,
        txs_verify_cache: &mut LruCache<H256, Cycle>,
    ) {
        let mut tx_pool = self.tx_pool.borrow_mut();

        let mut detached = LinkedFnvHashSet::default();
        let mut attached = LinkedFnvHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().iter().skip(1).cloned())
        }

        let retain: Vec<Transaction> = detached.difference(&attached).cloned().collect();

        let get_cell_data = |out_point: &OutPoint| {
            self.store
                .get_cell_data(&out_point.tx_hash, out_point.index)
        };
        tx_pool.remove_expired(detached_proposal_id);
        tx_pool.remove_committed_txs_from_proposed(attached.iter(), get_cell_data);

        for tx in retain {
            let tx_hash = tx.hash().to_owned();
            let cached_cycles = txs_verify_cache.get(&tx_hash).cloned();
            let tx_short_id = tx.proposal_short_id();
            let tx_size = tx.serialized_size();
            if self.contains_proposal_id(&tx_short_id) {
                if let Ok(cycles) =
                    self.proposed_tx_and_descendants(&mut tx_pool, cached_cycles, tx_size, tx)
                {
                    if cached_cycles.is_none() {
                        txs_verify_cache.insert(tx_hash, cycles);
                    }
                    tx_pool.update_statics_for_add_tx(tx_size, cycles);
                }
            } else if self.contains_gap(&tx_short_id) {
                if tx_pool.add_gap(cached_cycles, tx_size, tx) {
                    tx_pool.update_statics_for_add_tx(tx_size, cached_cycles.unwrap_or(0));
                }
            } else if tx_pool.enqueue_tx(cached_cycles, tx_size, tx) {
                tx_pool.update_statics_for_add_tx(tx_size, cached_cycles.unwrap_or(0));
            }
        }

        for tx in &attached {
            self.try_proposed_orphan_by_ancestor(&mut tx_pool, tx);
        }

        let mut entries = Vec::new();
        let mut gaps = Vec::new();

        // pending ---> gap ----> proposed
        // try move gap to proposed
        for entry in tx_pool.gap.entries() {
            if self.contains_proposal_id(entry.key()) {
                let entry = entry.remove();
                entries.push((entry.cycles, entry.size, entry.transaction));
            }
        }

        // try move pending to proposed
        for entry in tx_pool.pending.entries() {
            if self.contains_proposal_id(entry.key()) {
                let entry = entry.remove();
                entries.push((entry.cycles, entry.size, entry.transaction));
            } else if self.contains_gap(entry.key()) {
                let entry = entry.remove();
                gaps.push((entry.cycles, entry.size, entry.transaction));
            }
        }

        // try move conflict to proposed
        for entry in tx_pool.conflict.entries() {
            if self.contains_proposal_id(entry.key()) {
                let entry = entry.remove();
                entries.push((entry.cycles, entry.size, entry.transaction));
            } else if self.contains_gap(entry.key()) {
                let entry = entry.remove();
                gaps.push((entry.cycles, entry.size, entry.transaction));
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

        for (cycles, size, tx) in gaps {
            debug_target!(
                crate::LOG_TARGET_TX_POOL,
                "tx proposed, add to gap {:x}",
                tx.hash()
            );
            tx_pool.add_gap(cycles, size, tx);
        }
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.borrow().last_txs_updated_at
    }

    pub fn get_proposals(&self, proposals_limit: usize) -> HashSet<ProposalShortId> {
        let tx_pool = self.tx_pool.borrow();
        tx_pool
            .pending
            .keys()
            .chain(tx_pool.gap.keys())
            .take(proposals_limit)
            .cloned()
            .collect()
    }

    pub fn get_proposed_txs(
        &self,
        txs_size_limit: usize,
        cycles_limit: Cycle,
    ) -> (Vec<ProposedEntry>, usize, Cycle) {
        let mut size = 0;
        let mut cycles = 0;
        let tx_pool = self.tx_pool.borrow();
        let entries = tx_pool
            .proposed
            .txs_iter()
            .take_while(|tx| {
                cycles += tx.cycles;
                size += tx.size;
                (size < txs_size_limit) && (cycles < cycles_limit)
            })
            .cloned()
            .collect();
        (entries, size, cycles)
    }

    pub fn tx_pool(&self) -> Ref<TxPool> {
        self.tx_pool.borrow()
    }

    pub fn mut_tx_pool(&mut self) -> &mut TxPool {
        self.tx_pool.get_mut()
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
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.cell_set.get(&out_point.tx_hash) {
            Some(tx_meta) => match tx_meta.is_dead(out_point.index as usize) {
                Some(false) => {
                    let mut cell_meta = self
                        .store
                        .get_cell_meta(&out_point.tx_hash, out_point.index)
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data = self
                            .store
                            .get_cell_data(&out_point.tx_hash, out_point.index)
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

impl HeaderProvider for ChainState {
    fn header(&self, block_hash: &H256, out_point: Option<&OutPoint>) -> HeaderStatus {
        match self.store.get_block_header(&block_hash) {
            Some(header) => {
                if let Some(out_point) = out_point {
                    self.store.get_transaction_info(&out_point.tx_hash).map_or(
                        HeaderStatus::InclusionFaliure,
                        |info| {
                            if info.block_hash == *block_hash {
                                HeaderStatus::live_header(header)
                            } else {
                                HeaderStatus::InclusionFaliure
                            }
                        },
                    )
                } else {
                    HeaderStatus::live_header(header)
                }
            }
            None => HeaderStatus::Unknown,
        }
    }
}

impl<'a, CS: ChainStore<'a>> CellProvider for ChainCellSetOverlay<'a, CS> {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.overlay.get(&out_point.tx_hash) {
            Some(tx_meta) => match tx_meta.is_dead(out_point.index as usize) {
                Some(false) => {
                    let mut cell_meta = self
                        .store
                        .get_cell_meta(&out_point.tx_hash, out_point.index)
                        .expect("store should be consistent with cell_set");
                    if with_data {
                        cell_meta.mem_cell_data = self
                            .store
                            .get_cell_data(&out_point.tx_hash, out_point.index)
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
