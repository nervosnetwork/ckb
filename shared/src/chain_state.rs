use crate::index::ChainIndex;
use crate::tx_pool::{PoolEntry, PoolError, StagingTxResult, TxPool};
use crate::tx_proposal_table::TxProposalTable;
use crate::txo_set::TxoSet;
use crate::txo_set::TxoSetDiff;
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus, ResolvedTransaction};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use ckb_verification::{TransactionError, TransactionVerifier};
use fnv::FnvHashSet;
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cell::{Ref, RefCell};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ChainState<CI> {
    store: Arc<CI>,
    tip_header: Header,
    total_difficulty: U256,
    txo_set: TxoSet,
    proposal_ids: TxProposalTable,
    // interior mutability for immutable borrow proposal_ids
    tx_pool: RefCell<TxPool>,
    txs_verify_cache: RefCell<LruCache<H256, Cycle>>,
}

impl<CI: ChainIndex> ChainState<CI> {
    pub fn new(
        store: &Arc<CI>,
        tip_header: Header,
        total_difficulty: U256,
        txo_set: TxoSet,
        proposal_ids: TxProposalTable,
        tx_pool: TxPool,
        txs_verify_cache: LruCache<H256, Cycle>,
    ) -> Self {
        ChainState {
            store: Arc::clone(store),
            tip_header,
            total_difficulty,
            txo_set,
            proposal_ids,
            tx_pool: RefCell::new(tx_pool),
            txs_verify_cache: RefCell::new(txs_verify_cache),
        }
    }

    pub fn tip_number(&self) -> BlockNumber {
        self.tip_header.number()
    }

    pub fn tip_hash(&self) -> H256 {
        self.tip_header.hash()
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn tip_header(&self) -> &Header {
        &self.tip_header
    }

    pub fn txo_set(&self) -> &TxoSet {
        &self.txo_set
    }

    pub fn is_spent(&self, o: &OutPoint) -> Option<bool> {
        self.txo_set.is_spent(o)
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.proposal_ids.contains(id)
    }

    pub fn update_proposal_ids(&mut self, block: &Block) {
        self.proposal_ids
            .update_or_insert(block.header().number(), block.union_proposal_ids())
    }

    pub fn get_proposal_ids_iter(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.proposal_ids.get_ids_iter()
    }

    pub fn proposal_ids_finalize(&mut self, number: BlockNumber) -> Vec<ProposalShortId> {
        self.proposal_ids.finalize(number)
    }

    pub fn update_tip(&mut self, header: Header, total_difficulty: U256, txo_diff: TxoSetDiff) {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        self.txo_set.update(txo_diff);
    }

    pub fn add_tx_to_pool(&self, tx: Transaction, max_cycles: Cycle) -> Result<(), PoolError> {
        let mut tx_pool = self.tx_pool.borrow_mut();
        let short_id = tx.proposal_short_id();
        if self.contains_proposal_id(&short_id) {
            let entry = PoolEntry::new(tx, 0, None);
            self.staging_tx(&mut tx_pool, entry, max_cycles)?;
        } else {
            tx_pool.enqueue_tx(tx);
        }
        Ok(())
    }

    fn get_cell_status_from_store(&self, out_point: &OutPoint) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(f) = self.is_spent(out_point) {
            if f {
                CellStatus::Dead
            } else {
                let transaction = self
                    .store
                    .get_transaction(&out_point.hash)
                    .expect("transaction must exist");
                CellStatus::Live(transaction.outputs()[index].clone())
            }
        } else {
            CellStatus::Unknown
        }
    }

    pub fn resolve_tx_from_pool(&self, tx: &Transaction, tx_pool: &TxPool) -> ResolvedTransaction {
        let fetch_cell = |op| match tx_pool.staging.cell(op) {
            CellStatus::Unknown => self.get_cell_status_from_store(op),
            cs => cs,
        };
        let mut seen_inputs = FnvHashSet::default();
        let inputs = tx.input_pts();
        let input_cells = inputs
            .iter()
            .map(|input| {
                if seen_inputs.insert(input.clone()) {
                    fetch_cell(input)
                } else {
                    CellStatus::Dead
                }
            })
            .collect();

        let dep_cells = tx
            .dep_pts()
            .iter()
            .map(|dep| {
                if seen_inputs.insert(dep.clone()) {
                    fetch_cell(dep)
                } else {
                    CellStatus::Dead
                }
            })
            .collect();

        ResolvedTransaction {
            transaction: tx.clone(),
            input_cells,
            dep_cells,
        }
    }

    fn verify_rtx(
        &self,
        rtx: &ResolvedTransaction,
        max_cycles: Cycle,
    ) -> Result<Cycle, TransactionError> {
        let tx_hash = rtx.transaction.hash();
        let ret = { self.txs_verify_cache.borrow().get(&tx_hash).cloned() };
        match ret {
            Some(cycles) => Ok(cycles),
            None => {
                let cycles = TransactionVerifier::new(&rtx).verify(max_cycles)?;
                // write cache
                self.txs_verify_cache.borrow_mut().insert(tx_hash, cycles);
                Ok(cycles)
            }
        }
    }

    // remove resolved tx from orphan pool
    pub(crate) fn update_orphan_from_tx(
        &self,
        tx_pool: &mut TxPool,
        tx: &Transaction,
        max_cycles: Cycle,
    ) {
        let entries = tx_pool.orphan.remove_by_ancestor(tx);

        for mut entry in entries {
            let verify_result = match entry.cycles {
                Some(cycles) => Ok(cycles),
                None => {
                    let rtx = self.resolve_tx_from_pool(tx, tx_pool);
                    self.verify_rtx(&rtx, max_cycles)
                }
            };

            match verify_result {
                Ok(cycles) => {
                    entry.cycles = Some(cycles);
                    tx_pool.add_staging(entry);
                }
                Err(TransactionError::Conflict) => {
                    tx_pool
                        .conflict
                        .insert(entry.transaction.proposal_short_id(), entry);
                }
                _ => (),
            }
        }
    }

    pub(crate) fn staging_tx(
        &self,
        tx_pool: &mut TxPool,
        mut entry: PoolEntry,
        max_cycles: Cycle,
    ) -> Result<StagingTxResult, PoolError> {
        let tx = &entry.transaction;
        let inputs = tx.input_pts();
        let deps = tx.dep_pts();
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        let rtx = self.resolve_tx_from_pool(tx, tx_pool);

        let mut unknowns = Vec::new();
        for (cs, input) in rtx.input_cells.iter().zip(inputs.iter()) {
            match cs {
                CellStatus::Unknown => {
                    unknowns.push(input.clone());
                }
                CellStatus::Dead => {
                    tx_pool.conflict.insert(short_id, entry);
                    return Err(PoolError::Conflict);
                }
                _ => {}
            }
        }

        for (cs, dep) in rtx.dep_cells.iter().zip(deps.iter()) {
            match cs {
                CellStatus::Unknown => {
                    unknowns.push(dep.clone());
                }
                CellStatus::Dead => {
                    tx_pool.conflict.insert(short_id, entry);
                    return Err(PoolError::Conflict);
                }
                _ => {}
            }
        }

        if unknowns.is_empty() && entry.cycles.is_none() {
            let cycles = self.verify_rtx(&rtx, max_cycles).map_err(|e| {
                error!(target: "txs_pool", "Failed to staging tx {:}, reason: {:?}", tx_hash, e);
                PoolError::InvalidTx(e)
            })?;
            entry.cycles = Some(cycles);
        }

        if !unknowns.is_empty() {
            tx_pool.add_orphan(entry, unknowns);
            return Ok(StagingTxResult::Orphan);
        }

        tx_pool.add_staging(entry);
        Ok(StagingTxResult::Normal)
    }

    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: &[Block],
        attached_blocks: &[Block],
        detached_proposal_id: &[ProposalShortId],
        max_cycles: Cycle,
    ) {
        let mut tx_pool = self.tx_pool.borrow_mut();
        tx_pool.remove_staged(detached_proposal_id);

        let mut detached = FnvHashSet::default();
        let mut attached = FnvHashSet::default();

        //skip cellbase
        for blk in detached_blocks {
            detached.extend(blk.commit_transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.commit_transactions().iter().skip(1).cloned())
        }

        let retain: Vec<&Transaction> = detached.difference(&attached).collect();

        for tx in retain {
            let rtx = self.resolve_tx_from_pool(tx, &tx_pool);
            if let Ok(cycles) = self.verify_rtx(&rtx, max_cycles) {
                tx_pool.staging.readd_tx(&tx, cycles);
            }
        }

        for tx in &attached {
            self.update_orphan_from_tx(&mut tx_pool, tx, max_cycles);
        }

        for tx in &attached {
            tx_pool.staging.commit_tx(tx);
        }

        for id in self.get_proposal_ids_iter() {
            if let Some(entry) = tx_pool.remove_pending_from_proposal(id) {
                let tx = entry.transaction.clone();
                let tx_hash = tx.hash();
                match self.staging_tx(&mut tx_pool, entry, max_cycles) {
                    Ok(StagingTxResult::Normal) => {
                        self.update_orphan_from_tx(&mut tx_pool, &tx, max_cycles);
                    }
                    Err(e) => {
                        error!(target: "txs_pool", "Failed to staging tx {:}, reason: {:?}", tx_hash, e);
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.borrow().last_txs_updated_at
    }

    pub fn mut_txs_verify_cache(&mut self) -> &mut LruCache<H256, Cycle> {
        self.txs_verify_cache.get_mut()
    }

    pub fn get_proposal_and_staging_txs(
        &self,
        max_prop: usize,
        max_tx: usize,
    ) -> (Vec<ProposalShortId>, Vec<PoolEntry>) {
        let tx_pool = self.tx_pool.borrow();
        let proposal = tx_pool.pending.fetch(max_prop);
        let staging_txs = tx_pool.staging.get_txs(max_tx);
        (proposal, staging_txs)
    }

    pub fn tx_pool(&self) -> Ref<TxPool> {
        self.tx_pool.borrow()
    }

    pub fn mut_tx_pool(&mut self) -> &mut TxPool {
        self.tx_pool.get_mut()
    }
}
