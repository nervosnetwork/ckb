use crate::cell_set::{CellSet, CellSetDiff, CellSetOverlay};
use crate::error::SharedError;
use crate::tx_pool::types::{PoolEntry, ProposedEntry};
use crate::tx_pool::{PoolError, TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::Block;
use ckb_core::cell::{
    resolve_transaction, CellMeta, CellProvider, CellStatus, HeaderProvider, HeaderStatus,
    OverlayCellProvider, ResolvedTransaction, UnresolvableError,
};
use ckb_core::extras::EpochExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::script::Script;
use ckb_core::transaction::CellOutput;
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use ckb_script::ScriptConfig;
use ckb_store::ChainStore;
use ckb_traits::BlockMedianTimeContext;
use ckb_util::LinkedFnvHashSet;
use ckb_util::{FnvHashMap, FnvHashSet};
use ckb_verification::{ContextualTransactionVerifier, TransactionVerifier};
use log::{debug, trace};
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
    pub(crate) cell_set: CellSet,
    proposal_ids: TxProposalTable,
    // interior mutability for immutable borrow proposal_ids
    tx_pool: RefCell<TxPool>,
    consensus: Arc<Consensus>,
    current_epoch_ext: EpochExt,
    script_config: ScriptConfig,
}

impl<CS: ChainStore> ChainState<CS> {
    pub fn init(
        store: &Arc<CS>,
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
            current_epoch_ext: epoch_ext,
            script_config,
        })
    }

    pub fn store(&self) -> &Arc<CS> {
        &self.store
    }

    fn init_proposal_ids(
        store: &CS,
        proposal_window: ProposalWindow,
        tip_number: u64,
    ) -> TxProposalTable {
        let mut proposal_ids = TxProposalTable::new(tip_number, proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.start());
        for bn in proposal_start..=tip_number {
            if let Some(hash) = store.get_block_hash(bn) {
                let block = store.get_block(&hash).expect("index correct");
                proposal_ids.insert(
                    bn,
                    block.union_proposal_ids(),
                    block
                        .get_cellbase_lock()
                        .cloned()
                        .expect("block must have cellbase"),
                );
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

                cell_set.insert(tx.hash().to_owned(), n, tx.is_cellbase(), output_len);
            }
        }

        cell_set
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

    pub fn get_proposer_by_id(&self, id: &ProposalShortId) -> Option<Script> {
        self.proposal_ids.get_proposer_by_id(id)
    }

    /// block should be verified
    pub fn insert_proposal_ids(&mut self, block: &Block) {
        self.proposal_ids.insert(
            block.header().number(),
            block.union_proposal_ids(),
            block.get_cellbase_lock().cloned().expect("block verified"),
        );
    }

    pub fn remove_proposal_ids(&mut self, block: &Block) {
        self.proposal_ids.remove(block.header().number());
    }

    // pub fn get_proposal_ids_iter(&self) -> impl Iterator<Item = &ProposalShortId> {
    //     self.proposal_ids.get_ids_iter()
    // }

    pub fn get_proposal_ids_iter(
        &self,
    ) -> impl Iterator<Item = (&BlockNumber, &FnvHashSet<ProposalShortId>)> {
        self.proposal_ids.get_ids_iter()
    }

    pub fn proposal_ids_finalize(&mut self, number: BlockNumber) -> FnvHashSet<ProposalShortId> {
        self.proposal_ids.finalize(number)
    }

    pub fn update_current_epoch_ext(&mut self, epoch_ext: EpochExt) {
        self.current_epoch_ext = epoch_ext;
    }

    pub fn update_tip(&mut self, header: Header, total_difficulty: U256, txo_diff: CellSetDiff) {
        self.tip_header = header;
        self.total_difficulty = total_difficulty;
        self.cell_set.update(txo_diff);
    }

    pub fn get_entry_from_pool(
        &self,
        short_id: &ProposalShortId,
    ) -> Option<(Transaction, Option<Cycle>)> {
        self.tx_pool.borrow().get_entry(short_id)
    }

    // Add a verified tx into pool
    // this method will handle fork related verifications to make sure we are safe during a fork
    pub fn add_tx_to_pool(&self, tx: Transaction, cycles: Cycle) -> Result<Cycle, PoolError> {
        let short_id = tx.proposal_short_id();
        match self.resolve_tx_from_pending_and_proposed(&tx) {
            Ok(rtx) => {
                self.verify_rtx(&rtx, Some(cycles)).map(|cycles| {
                    let mut tx_pool = self.tx_pool.borrow_mut();
                    if let Some(proposer) = self.get_proposer_by_id(&tx.proposal_short_id()) {
                        // if tx is proposed, we resolve from staging, verify again
                        if let Err(e) = self.proposed_tx_and_descendants(&mut tx_pool, Some(cycles), tx, proposer) {
                            debug!(target: "tx_pool", "Failed to add proposed tx {:?}, reason: {:?}", short_id, e)
                        }
                    } else {
                        tx_pool.enqueue_tx(Some(cycles), tx);
                    }
                    cycles
                })
            }
            Err(err) => Err(PoolError::UnresolvableTransaction(err)),
        }
    }

    pub fn resolve_tx_from_pending_and_proposed<'a>(
        &self,
        tx: &'a Transaction,
    ) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
        let tx_pool = self.tx_pool.borrow_mut();
        let proposed_provider = OverlayCellProvider::new(&tx_pool.proposed, self);
        let pending_and_proposed_provider =
            OverlayCellProvider::new(&tx_pool.pending, &proposed_provider);
        let mut seen_inputs = FnvHashSet::default();
        resolve_transaction(tx, &mut seen_inputs, &pending_and_proposed_provider, self)
    }

    pub fn resolve_tx_from_proposed<'a>(
        &self,
        tx: &'a Transaction,
        tx_pool: &TxPool,
    ) -> Result<ResolvedTransaction<'a>, UnresolvableError> {
        let cell_provider = OverlayCellProvider::new(&tx_pool.proposed, self);
        let mut seen_inputs = FnvHashSet::default();
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
                    Arc::clone(self.store()),
                    &self,
                    self.tip_number(),
                    self.consensus().cellbase_maturity,
                    &self.script_config,
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
            if let Some(proposer) = self.get_proposer_by_id(&tx.proposal_short_id()) {
                let tx_hash = entry.transaction.hash().to_owned();
                let ret = self.proposed_tx(tx_pool, entry.cycles, entry.transaction, proposer);
                if ret.is_err() {
                    trace!(target: "tx_pool", "proposed tx {:x} failed {:?}", tx_hash, ret);
                }
            } else {
                tx_pool.enqueue_tx(entry.cycles, entry.transaction);
            }
        }
    }

    pub(crate) fn proposed_tx(
        &self,
        tx_pool: &mut TxPool,
        cycles: Option<Cycle>,
        tx: Transaction,
        proposer: Script,
    ) -> Result<Cycle, PoolError> {
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        match self.resolve_tx_from_proposed(&tx, tx_pool) {
            Ok(rtx) => {
                let fee = rtx.fee().map_err(|_| PoolError::InvalidFee)?;
                match self.verify_rtx(&rtx, cycles) {
                    Ok(cycles) => {
                        tx_pool.add_proposed(cycles, tx, fee, proposer);
                        Ok(cycles)
                    }
                    Err(e) => {
                        debug!(target: "tx_pool", "Failed to add proposed tx {:x}, reason: {:?}", tx_hash, e);
                        Err(e)
                    }
                }
            }
            Err(err) => {
                match &err {
                    UnresolvableError::Dead(_) => {
                        tx_pool
                            .conflict
                            .insert(short_id, PoolEntry::new(tx, 0, cycles));
                    }
                    UnresolvableError::Unknown(out_points) => {
                        tx_pool.add_orphan(cycles, tx, out_points.clone());
                    }
                    // The remaining errors are Empty, UnspecifiedInputCell and
                    // InvalidHeader. They all represent invalid transactions
                    // that should just be discarded.
                    UnresolvableError::Empty => (),
                    UnresolvableError::UnspecifiedInputCell(_) => (),
                    UnresolvableError::InvalidHeader(_) => (),
                }
                Err(PoolError::UnresolvableTransaction(err))
            }
        }
    }

    pub(crate) fn proposed_tx_and_descendants(
        &self,
        tx_pool: &mut TxPool,
        cycles: Option<Cycle>,
        tx: Transaction,
        proposer: Script,
    ) -> Result<Cycle, PoolError> {
        self.proposed_tx(tx_pool, cycles, tx.clone(), proposer)
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

        tx_pool.remove_expired(detached_proposal_id);
        tx_pool.remove_committed_txs_from_proposed(attached.iter());

        for tx in retain {
            let tx_hash = tx.hash().to_owned();
            let cached_cycles = txs_verify_cache.get(&tx_hash).cloned();
            if let Some(proposer) = self.get_proposer_by_id(&tx.proposal_short_id()) {
                if let Ok(cycles) =
                    self.proposed_tx_and_descendants(&mut tx_pool, cached_cycles, tx, proposer)
                {
                    if cached_cycles.is_none() {
                        txs_verify_cache.insert(tx_hash, cycles);
                    }
                }
            } else {
                tx_pool.enqueue_tx(cached_cycles, tx);
            }
        }

        for tx in &attached {
            self.try_proposed_orphan_by_ancestor(&mut tx_pool, tx);
        }

        let mut entries = Vec::new();
        for entry in tx_pool.pending.entries() {
            if let Some(proposer) = self.get_proposer_by_id(entry.key()) {
                entries.push((entry.remove(), proposer.clone()));
            }
        }

        for entry in tx_pool.conflict.entries() {
            if let Some(proposer) = self.get_proposer_by_id(entry.key()) {
                entries.push((entry.remove(), proposer.clone()));
            }
        }

        for (entry, proposer) in entries {
            let tx_hash = entry.transaction.hash().to_owned();
            if let Err(e) = self.proposed_tx_and_descendants(
                &mut tx_pool,
                entry.cycles,
                entry.transaction,
                proposer,
            ) {
                debug!(target: "tx_pool", "Failed to add proposed tx {:}, reason: {:?}", tx_hash, e);
            }
        }
    }

    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.borrow().last_txs_updated_at
    }

    pub fn get_proposals(&self, proposals_limit: usize) -> Vec<ProposalShortId> {
        let tx_pool = self.tx_pool.borrow();
        tx_pool.pending.fetch(proposals_limit)
    }

    pub fn get_proposed_txs(
        &self,
        txs_size_limit: usize,
        cycles_limit: Cycle,
    ) -> Vec<ProposedEntry> {
        let mut size = 0;
        let mut cycles = 0;
        let tx_pool = self.tx_pool.borrow();
        tx_pool
            .proposed
            .txs_iter()
            .take_while(|tx| {
                cycles += tx.cycles;
                size += tx.transaction.serialized_size();
                (size < txs_size_limit) && (cycles < cycles_limit)
            })
            .cloned()
            .collect()
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
            store: Arc::clone(self.store()),
            outputs,
        }
    }
}

pub struct ChainCellSetOverlay<'a, CS> {
    pub(crate) overlay: CellSetOverlay<'a>,
    pub(crate) store: Arc<CS>,
    pub(crate) outputs: &'a FnvHashMap<H256, &'a [CellOutput]>,
}

impl<CS: ChainStore> CellProvider for ChainState<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &out_point.cell {
            match self.cell_set().get(&cell_out_point.tx_hash) {
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

impl<CS: ChainStore> HeaderProvider for ChainState<CS> {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        if let Some(block_hash) = &out_point.block_hash {
            match self.store.get_header(&block_hash) {
                Some(header) => {
                    if let Some(cell_out_point) = &out_point.cell {
                        self.store
                            .get_transaction_address(&cell_out_point.tx_hash)
                            .map_or(HeaderStatus::InclusionFaliure, |address| {
                                if address.block_hash == *block_hash {
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

impl<'a, CS: ChainStore> CellProvider for ChainCellSetOverlay<'a, CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &out_point.cell {
            match self.overlay.get(&cell_out_point.tx_hash) {
                Some(tx_meta) => match tx_meta.is_dead(cell_out_point.index as usize) {
                    Some(false) => {
                        let cell_meta = self
                            .outputs
                            .get(&cell_out_point.tx_hash)
                            .map(|outputs| {
                                let output = &outputs[cell_out_point.index as usize];
                                CellMeta {
                                    cell_output: Some(output.clone()),
                                    out_point: cell_out_point.to_owned(),
                                    block_number: Some(tx_meta.block_number()),
                                    cellbase: tx_meta.is_cellbase(),
                                    capacity: output.capacity,
                                    data_hash: None,
                                }
                            })
                            .or_else(|| {
                                self.store
                                    .get_cell_meta(&cell_out_point.tx_hash, cell_out_point.index)
                            })
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
