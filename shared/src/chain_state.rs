use crate::cell_set::{CellSet, CellSetDiff, CellSetOverlay};
use crate::error::SharedError;
use crate::tx_pool::types::PoolEntry;
use crate::tx_pool::{PoolError, TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, CellStatus, LiveCell, OverlayCellProvider, ResolvedTransaction,
};
#[allow(unused_imports)] // incorrect lint
use ckb_core::cell::{CellMeta, CellProvider};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::CellOutput;
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use ckb_verification::{PoolTransactionVerifier, TransactionVerifier};
use fnv::{FnvHashMap, FnvHashSet};
use log::{error, trace};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cell::{Ref, RefCell};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ChainState<CS> {
    store: Arc<CS>,
    tip_header: Header,
    total_difficulty: U256,
    pub(crate) cell_set: CellSet,
    proposal_ids: TxProposalTable,
    // interior mutability for immutable borrow proposal_ids
    tx_pool: RefCell<TxPool>,
    consensus: Arc<Consensus>,
}

impl<CS: ChainStore> ChainState<CS> {
    pub fn init(
        store: &Arc<CS>,
        consensus: Arc<Consensus>,
        tx_pool_config: TxPoolConfig,
    ) -> Result<Self, SharedError> {
        // check head in store or save the genesis block as head
        let tip_header = {
            let genesis = consensus.genesis_block();
            match store.get_tip_header() {
                Some(tip_header) => {
                    if let Some(genesis_hash) = store.get_block_hash(0) {
                        let expect_genesis_hash = consensus.genesis_hash();
                        if &genesis_hash == expect_genesis_hash {
                            Ok(tip_header)
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
                    .init(&genesis)
                    .map_err(|_| {
                        SharedError::InvalidData("failed to init genesis block".to_owned())
                    })
                    .map(|_| genesis.header().to_owned()),
            }
        }?;

        let tx_pool = TxPool::new(tx_pool_config);

        let tip_number = tip_header.number();
        let proposal_window = consensus.tx_proposal_window();
        let proposal_ids = Self::init_proposal_ids(&store, proposal_window, tip_number);

        let cell_set = Self::init_cell_set(&store, tip_number);

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
        })
    }

    fn init_proposal_ids(
        store: &CS,
        proposal_window: ProposalWindow,
        tip_number: u64,
    ) -> TxProposalTable {
        let mut proposal_ids = TxProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.start());
        let proposal_end = tip_number.saturating_sub(proposal_window.end());
        for bn in proposal_start..=proposal_end {
            if let Some(hash) = store.get_block_hash(bn) {
                let mut ids_set = FnvHashSet::default();
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

    fn init_cell_set(store: &CS, number: u64) -> CellSet {
        let mut cell_set = CellSet::new();

        for n in 0..=number {
            let hash = store.get_block_hash(n).unwrap();
            for tx in store.get_block_body(&hash).unwrap() {
                let inputs = tx.input_pts();
                let output_len = tx.outputs().len();

                for o in inputs {
                    cell_set.mark_dead(&o);
                }

                cell_set.insert(tx.hash(), n, tx.is_cellbase(), output_len);
            }
        }

        cell_set
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

    pub fn cell_set(&self) -> &CellSet {
        &self.cell_set
    }

    pub fn is_dead_cell(&self, o: &OutPoint) -> Option<bool> {
        self.cell_set.is_dead(o)
    }

    pub fn proposal_ids(&self) -> &TxProposalTable {
        &self.proposal_ids
    }

    pub fn contains_proposal_id(&self, id: &ProposalShortId) -> bool {
        self.proposal_ids.contains(id)
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

    pub fn proposal_ids_finalize(&mut self, number: BlockNumber) -> FnvHashSet<ProposalShortId> {
        self.proposal_ids.finalize(number)
    }

    pub fn update_tip(&mut self, header: Header, total_difficulty: U256, txo_diff: CellSetDiff) {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        self.cell_set.update(txo_diff);
    }

    pub fn get_entry_from_pool(&self, short_id: &ProposalShortId) -> Option<PoolEntry> {
        self.tx_pool.borrow().get_entry(short_id).cloned()
    }

    pub fn add_tx_to_pool(&self, tx: Transaction) -> Result<Cycle, PoolError> {
        let mut tx_pool = self.tx_pool.borrow_mut();
        let short_id = tx.proposal_short_id();
        let rtx = self.resolve_tx_from_pending_and_staging(&tx, &tx_pool);

        self.verify_rtx(&rtx, None).map(|cycles| {
            if self.contains_proposal_id(&short_id) {
                // if tx is proposed, we resolve from staging, verify again
                self.staging_tx_and_descendants(&mut tx_pool, Some(cycles), tx);
            } else {
                tx_pool.enqueue_tx(Some(cycles), tx);
            }
            cycles
        })
    }

    pub fn resolve_tx_from_pending_and_staging<'a>(
        &self,
        tx: &'a Transaction,
        tx_pool: &TxPool,
    ) -> ResolvedTransaction<'a> {
        let staging_provider = OverlayCellProvider::new(&tx_pool.staging, self);
        let pending_and_staging_provider =
            OverlayCellProvider::new(&tx_pool.pending, &staging_provider);
        let mut seen_inputs = FnvHashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &pending_and_staging_provider)
    }

    pub fn resolve_tx_from_staging<'a>(
        &self,
        tx: &'a Transaction,
        tx_pool: &TxPool,
    ) -> ResolvedTransaction<'a> {
        let cell_provider = OverlayCellProvider::new(&tx_pool.staging, self);
        let mut seen_inputs = FnvHashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &cell_provider)
    }

    // FIXME: we may need redesign orphan pool, this is not short-circuiting
    fn verify_rtx_inputs(&self, rtx: &ResolvedTransaction) -> Result<(), PoolError> {
        let mut unknowns = Vec::new();
        let inputs = rtx.transaction.input_pts();
        let deps = rtx.transaction.dep_pts();
        for (cs, input) in rtx.input_cells.iter().zip(inputs.iter()) {
            match cs {
                CellStatus::Unknown => {
                    unknowns.push(input.clone());
                }
                CellStatus::Dead => {
                    return Err(PoolError::Conflict);
                }
                CellStatus::Live(LiveCell::Null) => {
                    return Err(PoolError::NullInput);
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
                    return Err(PoolError::Conflict);
                }
                CellStatus::Live(LiveCell::Null) => {
                    return Err(PoolError::NullInput);
                }
                _ => {}
            }
        }

        if !unknowns.is_empty() {
            return Err(PoolError::UnknownInputs(unknowns));
        }
        Ok(())
    }

    pub(crate) fn verify_rtx(
        &self,
        rtx: &ResolvedTransaction,
        cycles: Option<Cycle>,
    ) -> Result<Cycle, PoolError> {
        self.verify_rtx_inputs(rtx)?;

        match cycles {
            Some(cycles) => {
                PoolTransactionVerifier::new(
                    &rtx,
                    &self,
                    self.tip_number(),
                    self.consensus().cellbase_maturity,
                )
                .verify()
                .map_err(PoolError::InvalidTx)?;
                Ok(cycles)
            }
            None => {
                let max_cycles = self.consensus.max_block_cycles();
                let cycles = TransactionVerifier::new(
                    &rtx,
                    Arc::clone(&self.store),
                    &self,
                    self.tip_number(),
                    self.consensus().cellbase_maturity,
                )
                .verify(max_cycles)
                .map_err(PoolError::InvalidTx)?;
                Ok(cycles)
            }
        }
    }

    // remove resolved tx from orphan pool
    pub(crate) fn try_staging_orphan_by_ancestor(&self, tx_pool: &mut TxPool, tx: &Transaction) {
        let entries = tx_pool.orphan.remove_by_ancestor(tx);
        for entry in entries {
            if self.contains_proposal_id(&tx.proposal_short_id()) {
                let tx_hash = entry.transaction.hash();
                let ret = self.staging_tx(tx_pool, entry.cycles, entry.transaction);
                if ret.is_err() {
                    trace!(target: "tx_pool", "staging tx {:x} failed {:?}", tx_hash, ret);
                }
            } else {
                tx_pool.enqueue_tx(entry.cycles, entry.transaction);
            }
        }
    }

    pub(crate) fn staging_tx(
        &self,
        tx_pool: &mut TxPool,
        cycles: Option<Cycle>,
        tx: Transaction,
    ) -> Result<Cycle, PoolError> {
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        let rtx = self.resolve_tx_from_staging(&tx, tx_pool);

        match self.verify_rtx(&rtx, cycles) {
            Err(PoolError::Conflict) => {
                tx_pool
                    .conflict
                    .insert(short_id, PoolEntry::new(tx, 0, cycles));
                Err(PoolError::Conflict)
            }
            Err(PoolError::UnknownInputs(unknowns)) => {
                tx_pool.add_orphan(cycles, tx, unknowns.clone());
                Err(PoolError::UnknownInputs(unknowns))
            }
            Ok(cycles) => {
                tx_pool.add_staging(cycles, tx);
                Ok(cycles)
            }
            Err(e) => {
                error!(target: "tx_pool", "Failed to staging tx {:}, reason: {:?}", tx_hash, e);
                Err(e)
            }
        }
    }

    pub(crate) fn staging_tx_and_descendants(
        &self,
        tx_pool: &mut TxPool,
        cycles: Option<Cycle>,
        tx: Transaction,
    ) {
        match self.staging_tx(tx_pool, cycles, tx.clone()) {
            Ok(_) => {
                self.try_staging_orphan_by_ancestor(tx_pool, &tx);
            }
            Err(e) => {
                error!(target: "tx_pool", "Failed to staging tx {:}, reason: {:?}", tx.hash(), e);
            }
        }
    }

    pub fn update_tx_pool_for_reorg<'a>(
        &self,
        detached_blocks: impl Iterator<Item = &'a Block>,
        attached_blocks: impl Iterator<Item = &'a Block>,
        detached_proposal_id: impl Iterator<Item = &'a ProposalShortId>,
    ) {
        let mut tx_pool = self.tx_pool.borrow_mut();

        let mut detached = FnvHashSet::default();
        let mut attached = FnvHashSet::default();

        for blk in detached_blocks {
            detached.extend(blk.transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.transactions().iter().skip(1).cloned())
        }

        let retain: Vec<Transaction> = detached.difference(&attached).cloned().collect();

        tx_pool.remove_expired(detached_proposal_id);
        tx_pool.remove_committed_txs_from_staging(attached.iter());

        for tx in retain {
            if self.contains_proposal_id(&tx.proposal_short_id()) {
                self.staging_tx_and_descendants(&mut tx_pool, None, tx);
            } else {
                tx_pool.enqueue_tx(None, tx);
            }
        }

        for tx in &attached {
            self.try_staging_orphan_by_ancestor(&mut tx_pool, tx);
        }

        for id in self.get_proposal_ids_iter() {
            if let Some(entry) = tx_pool.remove_pending_and_conflict(id) {
                self.staging_tx_and_descendants(&mut tx_pool, entry.cycles, entry.transaction);
            }
        }
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.borrow().last_txs_updated_at
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

    pub fn consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }

    pub fn new_cell_set_overlay<'a>(
        &'a self,
        diff: &CellSetDiff,
        outputs: &'a FnvHashMap<H256, &'a [CellOutput]>,
    ) -> ChainCellSetOverlay<'a, CS> {
        ChainCellSetOverlay {
            overlay: self.cell_set.new_overlay(diff),
            store: Arc::clone(&self.store),
            outputs,
        }
    }
}

#[allow(dead_code)] // incorrect lint
pub struct ChainCellSetOverlay<'a, CS> {
    pub(crate) overlay: CellSetOverlay<'a>,
    pub(crate) store: Arc<CS>,
    pub(crate) outputs: &'a FnvHashMap<H256, &'a [CellOutput]>,
}

#[cfg(not(test))]
impl<CS: ChainStore + Sync> CellProvider for ChainState<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.cell_set().get(&out_point.tx_hash) {
            Some(tx_meta) => {
                if tx_meta.is_dead(out_point.index as usize) {
                    CellStatus::Dead
                } else {
                    let cell_meta = self
                        .store
                        .get_cell_meta(&out_point.tx_hash, out_point.index)
                        .expect("store should be consistent with cell_set");
                    CellStatus::live_cell(cell_meta)
                }
            }
            None => CellStatus::Unknown,
        }
    }
}

#[cfg(not(test))]
impl<'a, CS: ChainStore> CellProvider for ChainCellSetOverlay<'a, CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.overlay.get(&out_point.tx_hash) {
            Some(tx_meta) => {
                if tx_meta.is_dead(out_point.index as usize) {
                    CellStatus::Dead
                } else {
                    let cell_meta = self
                        .outputs
                        .get(&out_point.tx_hash)
                        .map(|outputs| {
                            let output = &outputs[out_point.index as usize];
                            CellMeta {
                                cell_output: Some(output.clone()),
                                out_point: out_point.to_owned(),
                                block_number: Some(tx_meta.block_number()),
                                cellbase: tx_meta.is_cellbase(),
                                capacity: output.capacity,
                                data_hash: None,
                            }
                        })
                        .or_else(|| {
                            self.store
                                .get_cell_meta(&out_point.tx_hash, out_point.index)
                        })
                        .expect("store should be consistent with cell_set");

                    CellStatus::live_cell(cell_meta)
                }
            }
            None => CellStatus::Unknown,
        }
    }
}

impl<CS: ChainStore> BlockMedianTimeContext for &ChainState<CS> {
    fn median_block_count(&self) -> u64 {
        self.consensus.median_time_block_count() as u64
    }

    fn timestamp(&self, number: BlockNumber) -> Option<u64> {
        self.store.get_block_hash(number).and_then(|hash| {
            self.store
                .get_header(&hash)
                .map(|header| header.timestamp())
        })
    }
}
