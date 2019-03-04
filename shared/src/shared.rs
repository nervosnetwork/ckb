use crate::cachedb::CacheDB;
use crate::chain_state::ChainState;
use crate::error::SharedError;
use crate::index::ChainIndex;
use crate::store::ChainKVStore;
use crate::tx_pool::{PoolEntry, PoolError, PromoteTxResult, TxPool, TxPoolConfig};
use crate::tx_proposal_table::TxProposalTable;
use crate::txo_set::TxoSet;
use crate::{COLUMNS, COLUMN_BLOCK_HEADER};
use ckb_chain_spec::consensus::{Consensus, ProposalWindow};
use ckb_core::block::Block;
use ckb_core::cell::{CellProvider, CellStatus, ResolvedTransaction};
use ckb_core::extras::BlockExt;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{Capacity, OutPoint, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_core::Cycle;
use ckb_db::{DBConfig, KeyValueDB, MemoryKeyValueDB, RocksDB};
use ckb_traits::{BlockMedianTimeContext, ChainProvider};
use ckb_util::RwLock;
use ckb_verification::{TransactionError, TransactionVerifier};
use failure::Error;
use fnv::FnvHashSet;
use log::error;
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::sync::Arc;

pub struct Shared<CI> {
    store: Arc<CI>,
    chain_state: Arc<RwLock<ChainState>>,
    txs_verify_cache: Arc<RwLock<LruCache<H256, Cycle>>>,
    consensus: Arc<Consensus>,
    tx_pool: Arc<RwLock<TxPool>>,
}

// https://github.com/rust-lang/rust/issues/40754
impl<CI: ChainIndex> ::std::clone::Clone for Shared<CI> {
    fn clone(&self) -> Self {
        Shared {
            store: Arc::clone(&self.store),
            chain_state: Arc::clone(&self.chain_state),
            txs_verify_cache: Arc::clone(&self.txs_verify_cache),
            consensus: Arc::clone(&self.consensus),
            tx_pool: Arc::clone(&self.tx_pool),
        }
    }
}

impl<CI: ChainIndex> Shared<CI> {
    pub fn new(
        store: CI,
        consensus: Consensus,
        txs_verify_cache_size: usize,
        tx_pool_config: TxPoolConfig,
    ) -> Self {
        let chain_state = {
            // check head in store or save the genesis block as head
            let header = {
                let genesis = consensus.genesis_block();
                match store.get_tip_header() {
                    Some(h) => h,
                    None => {
                        store.init(&genesis);
                        genesis.header().clone()
                    }
                }
            };

            let tip_number = header.number();
            let proposal_window = consensus.tx_proposal_window();
            let proposal_ids = Self::init_proposal_ids(&store, proposal_window, tip_number);

            let txo_set = Self::init_txo_set(&store, tip_number);

            let total_difficulty = store
                .get_block_ext(&header.hash())
                .expect("block_ext stored")
                .total_difficulty;

            Arc::new(RwLock::new(ChainState::new(
                header,
                total_difficulty,
                txo_set,
                proposal_ids,
            )))
        };

        Shared {
            store: Arc::new(store),
            chain_state,
            txs_verify_cache: Arc::new(RwLock::new(LruCache::new(txs_verify_cache_size))),
            consensus: Arc::new(consensus),
            tx_pool: Arc::new(RwLock::new(TxPool::new(tx_pool_config))),
        }
    }

    pub fn chain_state(&self) -> &RwLock<ChainState> {
        &self.chain_state
    }

    pub fn store(&self) -> &Arc<CI> {
        &self.store
    }

    pub fn txs_verify_cache(&self) -> &RwLock<LruCache<H256, Cycle>> {
        &self.txs_verify_cache
    }

    pub fn init_proposal_ids(
        store: &CI,
        proposal_window: ProposalWindow,
        tip_number: u64,
    ) -> TxProposalTable {
        let mut proposal_ids = TxProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.1);
        let proposal_end = tip_number.saturating_sub(proposal_window.0);
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
        proposal_ids.reconstruct(tip_number);
        proposal_ids
    }

    pub fn init_txo_set(store: &CI, number: u64) -> TxoSet {
        let mut txo_set = TxoSet::new();

        for n in 0..=number {
            let hash = store.get_block_hash(n).unwrap();
            for tx in store.get_block_body(&hash).unwrap() {
                let inputs = tx.input_pts();
                let tx_hash = tx.hash();
                let output_len = tx.outputs().len();

                for o in inputs {
                    txo_set.mark_spent(&o);
                }

                txo_set.insert(tx_hash, output_len);
            }
        }

        txo_set
    }

    pub fn resolve_pool_tx(
        &self,
        chain_state: &ChainState,
        tx_pool: &TxPool,
        tx: &Transaction,
    ) -> ResolvedTransaction {
        let fetch_cell = |op| match tx_pool.promote.cell(op) {
            CellStatus::Unknown => self.cell_at(op, |op| chain_state.is_spent(op)),
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
        txs_cache: &mut LruCache<H256, Cycle>,
    ) -> Result<Cycle, TransactionError> {
        let tx_hash = rtx.transaction.hash();
        match txs_cache.get(&tx_hash) {
            Some(cycles) => Ok(*cycles),
            None => {
                let cycles =
                    TransactionVerifier::new(&rtx).verify(self.consensus.max_block_cycles())?;
                // write cache
                txs_cache.insert(tx_hash, cycles);
                Ok(cycles)
            }
        }
    }

    // *************************
    // Acquire chain_state Read Lock
    // *************************
    // Acquire tx_pool Write Lock
    // *************************
    // FIXME: review duplicate check
    pub fn add_tx_to_pool(&self, tx: Transaction) -> Result<(), PoolError> {
        let chain_state = self.chain_state.read();
        let mut tx_pool = self.tx_pool.write();
        let mut txs_cache = self.txs_verify_cache.write();

        let short_id = tx.proposal_short_id();

        if chain_state.contains_proposal_id(&short_id) {
            let entry = PoolEntry::new(tx, 0, None);
            self.promote_tx(&chain_state, &mut tx_pool, &mut txs_cache, entry)?;
        } else {
            tx_pool.enqueue_tx(tx);
        }
        Ok(())
    }

    // promote_tx moves entry that have become minable from the pending queue.
    // During this process, transactions has unknown input or deps will be added to orphan_pool.
    // conflict transactions will temporarily cached.
    pub(crate) fn promote_tx(
        &self,
        chain_state: &ChainState,
        tx_pool: &mut TxPool,
        txs_cache: &mut LruCache<H256, Cycle>,
        mut entry: PoolEntry,
    ) -> Result<PromoteTxResult, PoolError> {
        let tx = &entry.transaction;

        let inputs = tx.input_pts();
        let deps = tx.dep_pts();
        let short_id = tx.proposal_short_id();
        let tx_hash = tx.hash();

        let rtx = self.resolve_pool_tx(chain_state, &tx_pool, tx);

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
            let cycles = self.verify_rtx(&rtx, txs_cache).map_err(|e| {
                error!(target: "txs_pool", "Failed to promote tx {:}, reason: {:?}", tx_hash, e);
                PoolError::InvalidTx(e)
            })?;
            entry.cycles = Some(cycles);
        }

        if !unknowns.is_empty() {
            tx_pool.add_orphan(entry, unknowns);
            return Ok(PromoteTxResult::Orphan);
        }

        tx_pool.add_promote(entry);
        Ok(PromoteTxResult::Normal)
    }

    pub(crate) fn demote_tx(&self, tx_pool: &mut TxPool, ids: &[ProposalShortId]) {
        for id in ids {
            if let Some(txs) = tx_pool.promote.remove(id) {
                tx_pool.pending.insert(*id, txs[0].clone());

                for tx in txs.iter().skip(1) {
                    tx_pool
                        .conflict
                        .insert(tx.transaction.proposal_short_id(), tx.clone());
                }
            } else if let Some(tx) = tx_pool.conflict.remove(id) {
                tx_pool.pending.insert(*id, tx);
            } else if let Some(tx) = tx_pool.orphan.remove(id) {
                tx_pool.pending.insert(*id, tx);
            }
        }
    }

    // remove deps resolved tx from orphan pool, add
    pub(crate) fn reconcile_orphan(
        &self,
        chain_state: &ChainState,
        tx_pool: &mut TxPool,
        txs_cache: &mut LruCache<H256, Cycle>,
        tx: &Transaction,
    ) {
        let entries = tx_pool.orphan.reconcile_tx(tx);

        for mut entry in entries {
            let verify_result = match entry.cycles {
                Some(cycles) => Ok(cycles),
                None => {
                    let rtx = self.resolve_pool_tx(chain_state, &tx_pool, tx);
                    self.verify_rtx(&rtx, txs_cache)
                }
            };

            match verify_result {
                Ok(cycles) => {
                    entry.cycles = Some(cycles);
                    tx_pool.add_promote(entry);
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

    // *************************
    // Acquire tx_pool Write Lock
    // *************************
    // Acquire txs_verify_cache Write Lock
    // *************************
    pub fn reconcile_tx_pool(
        &self,
        chain_state: &ChainState,
        detached_blocks: &[Block],
        attached_blocks: &[Block],
        detached_proposal_id: &[ProposalShortId],
    ) {
        let mut tx_pool = self.tx_pool.write();
        let mut txs_cache = self.txs_verify_cache.write();

        self.demote_tx(&mut tx_pool, detached_proposal_id);
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
            let rtx = self.resolve_pool_tx(chain_state, &tx_pool, tx);
            if let Ok(cycles) = self.verify_rtx(&rtx, &mut txs_cache) {
                tx_pool.promote.readd_tx(&tx, cycles);
            }
        }

        for tx in &attached {
            self.reconcile_orphan(chain_state, &mut tx_pool, &mut txs_cache, tx);
        }

        for tx in &attached {
            tx_pool.promote.commit_tx(tx);
        }

        for id in chain_state.get_proposal_ids_iter() {
            if let Some(entry) = tx_pool.reconcile_proposal(id) {
                let tx = entry.transaction.clone();
                let tx_hash = tx.hash();
                match self.promote_tx(chain_state, &mut tx_pool, &mut txs_cache, entry) {
                    Ok(PromoteTxResult::Normal) => {
                        self.reconcile_orphan(chain_state, &mut tx_pool, &mut txs_cache, &tx);
                    }
                    Err(e) => {
                        error!(target: "txs_pool", "Failed to promote tx {:}, reason: {:?}", tx_hash, e);
                    }
                    _ => {}
                }
            }
        }
    }

    // *************************
    // Acquire tx_pool Read Lock
    // *************************
    pub fn get_last_txs_updated_at(&self) -> u64 {
        self.tx_pool.read().last_txs_updated_at
    }

    // *************************
    // Acquire tx_pool Read Lock
    // *************************
    pub fn get_proposal_commit_txs(
        &self,
        max_prop: usize,
        max_tx: usize,
    ) -> (Vec<ProposalShortId>, Vec<PoolEntry>) {
        let tx_pool = self.tx_pool.read();
        let proposal = tx_pool.pending.fetch(max_prop);
        let commit_txs = tx_pool.promote.get_mineable_txs(max_tx);
        (proposal, commit_txs)
    }

    pub fn tx_pool(&self) -> &RwLock<TxPool> {
        &self.tx_pool
    }
}

impl<CI: ChainIndex> CellProvider for Shared<CI> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        self.cell_at(out_point, |op| self.chain_state.read().is_spent(op))
    }

    fn cell_at<F: Fn(&OutPoint) -> Option<bool>>(
        &self,
        out_point: &OutPoint,
        is_spent: F,
    ) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(f) = is_spent(out_point) {
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
}

impl<CI: ChainIndex> ChainProvider for Shared<CI> {
    fn block(&self, hash: &H256) -> Option<Block> {
        self.store.get_block(hash)
    }

    fn block_body(&self, hash: &H256) -> Option<Vec<Transaction>> {
        self.store.get_block_body(hash)
    }

    fn block_header(&self, hash: &H256) -> Option<Header> {
        self.store.get_header(hash)
    }

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>> {
        self.store.get_block_proposal_txs_ids(hash)
    }

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>> {
        self.store.get_block_uncles(hash)
    }

    fn block_hash(&self, number: BlockNumber) -> Option<H256> {
        self.store.get_block_hash(number)
    }

    fn block_ext(&self, hash: &H256) -> Option<BlockExt> {
        self.store.get_block_ext(hash)
    }

    fn block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.store.get_block_number(hash)
    }

    fn genesis_hash(&self) -> H256 {
        self.consensus.genesis_block().header().hash()
    }

    fn get_transaction(&self, hash: &H256) -> Option<Transaction> {
        self.store.get_transaction(hash)
    }

    fn contain_transaction(&self, hash: &H256) -> bool {
        self.store.get_transaction_address(hash).is_some()
    }

    fn block_reward(&self, _block_number: BlockNumber) -> Capacity {
        // TODO: block reward calculation algorithm
        self.consensus.initial_block_reward()
    }

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header> {
        // if base in the main chain
        if let Some(n_number) = self.block_number(base) {
            if number > n_number {
                return None;
            } else {
                return self
                    .block_hash(number)
                    .and_then(|hash| self.block_header(&hash));
            }
        }

        // if base in the fork
        if let Some(header) = self.block_header(base) {
            let mut n_number = header.number();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.block_header(&index_walk.parent_hash()) {
                    index_walk = header;
                    n_number -= 1;
                } else {
                    return None;
                }
            }
            return Some(index_walk);
        }
        None
    }

    // TODO: find a way to write test for this once we can build a mock on
    // ChainIndex
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<Capacity, Error> {
        let mut fee = 0;
        for input in transaction.inputs() {
            let previous_output = &input.previous_output;
            match self.get_transaction(&previous_output.hash) {
                Some(previous_transaction) => {
                    let index = previous_output.index as usize;
                    if index < previous_transaction.outputs().len() {
                        fee += previous_transaction.outputs()[index].capacity;
                    } else {
                        Err(SharedError::InvalidInput)?;
                    }
                }
                None => Err(SharedError::InvalidInput)?,
            }
        }
        let spent_capacity: Capacity = transaction
            .outputs()
            .iter()
            .map(|output| output.capacity)
            .sum();
        if spent_capacity > fee {
            Err(SharedError::InvalidOutput)?;
        }
        fee -= spent_capacity;
        Ok(fee)
    }

    // T_interval = L / C_m
    // HR_m = HR_last/ (1 + o)
    // Diff= HR_m * T_interval / H = Diff_last * o_last / o
    #[allow(clippy::op_ref)]
    fn calculate_difficulty(&self, last: &Header) -> Option<U256> {
        let last_hash = last.hash();
        let last_number = last.number();
        let last_difficulty = last.difficulty();

        let interval = self.consensus.difficulty_adjustment_interval();

        if (last_number + 1) % interval != 0 {
            return Some(last_difficulty.clone());
        }

        let start = last_number.saturating_sub(interval);
        if let Some(start_header) = self.get_ancestor(&last_hash, start) {
            let start_total_uncles_count = self
                .block_ext(&start_header.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let last_total_uncles_count = self
                .block_ext(&last_hash)
                .expect("block_ext exist")
                .total_uncles_count;

            let difficulty = last_difficulty
                * U256::from(last_total_uncles_count - start_total_uncles_count)
                * U256::from((1.0 / self.consensus.orphan_rate_target()) as u64)
                / U256::from(interval);

            let min_difficulty = self.consensus.min_difficulty();
            let max_difficulty = last_difficulty * 2u32;
            if &difficulty > &max_difficulty {
                return Some(max_difficulty);
            }

            if &difficulty < min_difficulty {
                return Some(min_difficulty.clone());
            }
            return Some(difficulty);
        }
        None
    }

    fn consensus(&self) -> &Consensus {
        &*self.consensus
    }
}

impl<CI: ChainIndex> BlockMedianTimeContext for Shared<CI> {
    fn block_count(&self) -> u32 {
        self.consensus.median_time_block_count() as u32
    }
    fn timestamp(&self, hash: &H256) -> Option<u64> {
        self.block_header(hash).map(|header| header.timestamp())
    }
    fn parent_hash(&self, hash: &H256) -> Option<H256> {
        self.block_header(hash)
            .map(|header| header.parent_hash().to_owned())
    }
}

pub struct SharedBuilder<DB: KeyValueDB> {
    db: Option<DB>,
    consensus: Option<Consensus>,
    txs_verify_cache_size: Option<usize>,
    tx_pool_config: Option<TxPoolConfig>,
}

impl<DB: KeyValueDB> Default for SharedBuilder<DB> {
    fn default() -> Self {
        SharedBuilder {
            db: None,
            consensus: None,
            txs_verify_cache_size: None,
            tx_pool_config: None,
        }
    }
}

impl SharedBuilder<MemoryKeyValueDB> {
    pub fn new() -> Self {
        SharedBuilder {
            db: Some(MemoryKeyValueDB::open(COLUMNS as usize)),
            consensus: None,
            txs_verify_cache_size: None,
            tx_pool_config: None,
        }
    }
}

impl SharedBuilder<CacheDB<RocksDB>> {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn db(mut self, config: &DBConfig) -> Self {
        self.db = Some(CacheDB::new(
            RocksDB::open(config, COLUMNS),
            &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
        ));
        self
    }
}

pub const MIN_TXS_VERIFY_CACHE_SIZE: Option<usize> = Some(100);

impl<DB: 'static + KeyValueDB> SharedBuilder<DB> {
    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = Some(value);
        self
    }

    pub fn tx_pool_config(mut self, config: TxPoolConfig) -> Self {
        self.tx_pool_config = Some(config);
        self
    }

    pub fn txs_verify_cache_size(mut self, value: usize) -> Self {
        self.txs_verify_cache_size = Some(value);
        self
    }

    pub fn build(self) -> Shared<ChainKVStore<DB>> {
        let store = ChainKVStore::new(self.db.unwrap());
        let consensus = self.consensus.unwrap_or_else(Consensus::default);
        let tx_pool_config = self.tx_pool_config.unwrap_or_else(Default::default);
        let txs_verify_cache_size =
            std::cmp::max(MIN_TXS_VERIFY_CACHE_SIZE, self.txs_verify_cache_size)
                .expect("txs_verify_cache_size MUST not be none");
        Shared::new(store, consensus, txs_verify_cache_size, tx_pool_config)
    }
}
