use crate::cell_set::CellSet;
use crate::cell_set::CellSetDiff;
use crate::store::ChainStore;
use crate::tx_pool::{PoolEntry, PoolError, StagingTxResult, TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, CellMeta, CellProvider, CellStatus, OverlayCellProvider,
    ResolvedTransaction,
};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use ckb_traits::BlockMedianTimeContext;
use ckb_verification::{TransactionError, TransactionVerifier};
use fnv::FnvHashSet;
use log::{error, trace};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::cell::{Ref, RefCell};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ChainState<CS> {
    store: Arc<CS>,
    tip_header: Header,
    total_difficulty: U256,
    cell_set: CellSet,
    proposal_ids: TxProposalTable,
    // interior mutability for immutable borrow proposal_ids
    tx_pool: RefCell<TxPool>,
    txs_verify_cache: RefCell<LruCache<H256, Cycle>>,
    consensus: Arc<Consensus>,
}

impl<CS: ChainStore> ChainState<CS> {
    pub fn new(store: &Arc<CS>, consensus: Arc<Consensus>, tx_pool_config: TxPoolConfig) -> Self {
        // check head in store or save the genesis block as head
        let tip_header = {
            let genesis = consensus.genesis_block();
            match store.get_tip_header() {
                Some(h) => h,
                None => {
                    store
                        .init(&genesis)
                        .expect("init genesis block should be ok");
                    genesis.header().clone()
                }
            }
        };

        let txs_verify_cache = LruCache::new(tx_pool_config.txs_verify_cache_size);
        let tx_pool = TxPool::new(tx_pool_config);

        let tip_number = tip_header.number();
        let proposal_window = consensus.tx_proposal_window();
        let proposal_ids = Self::init_proposal_ids(&store, proposal_window, tip_number);

        let cell_set = Self::init_cell_set(&store, tip_number);

        let total_difficulty = store
            .get_block_ext(&tip_header.hash())
            .expect("block_ext stored")
            .total_difficulty;
        ChainState {
            store: Arc::clone(store),
            tip_header,
            total_difficulty,
            cell_set,
            proposal_ids,
            tx_pool: RefCell::new(tx_pool),
            txs_verify_cache: RefCell::new(txs_verify_cache),
            consensus,
        }
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
                        let ids = u.proposal_transactions;
                        ids_set.extend(ids);
                    }
                }
                proposal_ids.update_or_insert(bn, ids_set);
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

    pub fn is_dead(&self, o: &OutPoint) -> Option<bool> {
        self.cell_set.is_dead(o)
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

    pub fn update_tip(&mut self, header: Header, total_difficulty: U256, txo_diff: CellSetDiff) {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        self.cell_set.update(txo_diff);
    }

    pub fn add_tx_to_pool(&self, tx: Transaction, max_cycles: Cycle) -> Result<Cycle, PoolError> {
        let mut tx_pool = self.tx_pool.borrow_mut();
        let short_id = tx.proposal_short_id();
        let rtx = self.resolve_tx_from_pool(&tx, &tx_pool);
        let verify_result = self.verify_rtx(&rtx, max_cycles);
        let tx_hash = tx.hash();
        if self.contains_proposal_id(&short_id) {
            if !tx_pool.filter.insert(tx_hash.clone()) {
                trace!(target: "tx_pool", "discarding already known transaction {:#x}", tx_hash);
                return Err(PoolError::Duplicate);
            }
            let entry = PoolEntry::new(tx, 0, verify_result.ok());
            self.staging_tx(&mut tx_pool, entry, max_cycles)?;
            Ok(verify_result.map_err(PoolError::InvalidTx)?)
        } else {
            match verify_result {
                Ok(cycles) => {
                    // enqueue tx with cycles
                    let entry = PoolEntry::new(tx, 0, Some(cycles));
                    if !tx_pool.enqueue_tx(entry) {
                        return Err(PoolError::Duplicate);
                    }
                    Ok(cycles)
                }
                Err(TransactionError::UnknownInput) => {
                    let entry = PoolEntry::new(tx, 0, None);
                    if !tx_pool.enqueue_tx(entry) {
                        return Err(PoolError::Duplicate);
                    }
                    Err(PoolError::InvalidTx(TransactionError::UnknownInput))
                }
                Err(err) => Err(PoolError::InvalidTx(err)),
            }
        }
    }

    pub fn resolve_tx_from_pool(&self, tx: &Transaction, tx_pool: &TxPool) -> ResolvedTransaction {
        let cell_provider = OverlayCellProvider::new(&tx_pool.staging, self);
        let mut seen_inputs = FnvHashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &cell_provider)
    }

    pub fn verify_rtx(
        &self,
        rtx: &ResolvedTransaction,
        max_cycles: Cycle,
    ) -> Result<Cycle, TransactionError> {
        let tx_hash = rtx.transaction.hash();
        let ret = { self.txs_verify_cache.borrow().get(&tx_hash).cloned() };
        match ret {
            Some(cycles) => Ok(cycles),
            None => {
                let cycles =
                    TransactionVerifier::new(&rtx, &self, self.tip_number()).verify(max_cycles)?;
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
        let cycles = entry.cycles.expect("cycles must exists");
        tx_pool.add_staging(entry);
        Ok(StagingTxResult::Normal(cycles))
    }

    pub fn update_tx_pool_for_reorg(
        &self,
        detached_blocks: &[Block],
        attached_blocks: &[Block],
        detached_proposal_id: &[ProposalShortId],
        max_cycles: Cycle,
    ) {
        let mut tx_pool = self.tx_pool.borrow_mut();
        tx_pool.remove_expired(detached_proposal_id);

        let mut detached = FnvHashSet::default();
        let mut attached = FnvHashSet::default();

        //skip cellbase
        for blk in detached_blocks {
            detached.extend(blk.commit_transactions().iter().skip(1).cloned())
        }

        for blk in attached_blocks {
            attached.extend(blk.commit_transactions().iter().skip(1).cloned())
        }

        if !detached.is_empty() {
            self.txs_verify_cache.borrow_mut().clear();
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
            tx_pool.committed(tx);
        }

        for id in self.get_proposal_ids_iter() {
            if let Some(entry) = tx_pool.remove_pending_from_proposal(id) {
                let tx = entry.transaction.clone();
                let tx_hash = tx.hash();
                match self.staging_tx(&mut tx_pool, entry, max_cycles) {
                    Ok(StagingTxResult::Normal(_)) => {
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

    pub fn consensus(&self) -> Arc<Consensus> {
        Arc::clone(&self.consensus)
    }
}

impl<CS: ChainStore> CellProvider for ChainState<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        match self.cell_set().get(&out_point.hash) {
            Some(tx_meta) => {
                if tx_meta.is_dead(out_point.index as usize) {
                    CellStatus::Dead
                } else {
                    let tx = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("store should be consistent with cell_set");
                    CellStatus::Live(CellMeta {
                        cell_output: tx.outputs()[out_point.index as usize].clone(),
                        block_number: Some(tx_meta.block_number()),
                    })
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
