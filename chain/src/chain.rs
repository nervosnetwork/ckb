use super::{COLUMNS, COLUMN_BLOCK_HEADER};
use bigint::{H256, U256};
use cachedb::CacheDB;
use ckb_notify::{ForkBlocks, Notify};
use consensus::Consensus;
use core::block::IndexedBlock;
use core::cell::{CellProvider, CellStatus};
use core::extras::BlockExt;
use core::header::{BlockNumber, IndexedHeader};
use core::transaction::{Capacity, IndexedTransaction, OutPoint, ProposalShortId, Transaction};
use core::transaction_meta::TransactionMeta;
use core::uncle::UncleBlock;
use db::batch::Batch;
use db::diskdb::RocksDB;
use db::kvdb::KeyValueDB;
use db::memorydb::MemoryKeyValueDB;
use error::Error;
use fnv::{FnvHashMap, FnvHashSet};
use index::ChainIndex;
use log;
use std::cmp;
use std::path::Path;
use std::sync::Arc;
use store::ChainKVStore;
use time::now_ms;
use util::{RwLock, RwLockUpgradableReadGuard};

#[derive(Default, Debug, PartialEq, Clone, Eq)]
pub struct TipHeader {
    pub header: IndexedHeader,
    pub total_difficulty: U256,
    pub output_root: H256,
}

impl TipHeader {
    pub fn number(&self) -> BlockNumber {
        self.header.number
    }
}

pub struct Chain<CS> {
    store: CS,
    tip_header: RwLock<TipHeader>,
    consensus: Consensus,
    candidate_uncles: Arc<RwLock<FnvHashMap<H256, Arc<IndexedBlock>>>>,
    notify: Notify,
}

#[derive(Debug, Clone)]
pub struct BlockInsertionResult {
    pub fork_blks: ForkBlocks,
    pub new_best_block: bool,
}

pub trait ChainProvider: Sync + Send + CellProvider {
    fn process_block(&self, b: &IndexedBlock) -> Result<(), Error>;

    fn block_header(&self, hash: &H256) -> Option<IndexedHeader>;

    fn block_body(&self, hash: &H256) -> Option<Vec<IndexedTransaction>>;

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>>;

    // Proposals in blocks from bn-n(exclusive) to bn(inclusive)
    fn union_proposal_ids_n(&self, bn: BlockNumber, n: usize) -> Vec<Vec<ProposalShortId>>;

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>>;

    fn block_hash(&self, number: BlockNumber) -> Option<H256>;

    fn block_ext(&self, hash: &H256) -> Option<BlockExt>;

    fn output_root(&self, hash: &H256) -> Option<H256>;

    fn block_number(&self, hash: &H256) -> Option<BlockNumber>;

    fn block(&self, hash: &H256) -> Option<IndexedBlock>;

    fn genesis_hash(&self) -> H256;

    fn consensus(&self) -> &Consensus;

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<IndexedHeader>;

    fn get_tip_uncles(&self) -> Vec<UncleBlock>;

    //FIXME: This is bad idea
    fn tip_header(&self) -> &RwLock<TipHeader>;

    fn get_transaction(&self, hash: &H256) -> Option<IndexedTransaction>;

    fn contain_transaction(&self, hash: &H256) -> bool;

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta>;

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta>;

    fn block_reward(&self, block_number: BlockNumber) -> Capacity;

    // Loops through all inputs and outputs of given transaction to calculate
    // fee that miner can obtain. Could result in error state when input
    // transaction is missing.
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<Capacity, Error>;

    fn calculate_difficulty(&self, last: &IndexedHeader) -> Option<U256>;
}

impl<'a, CS: ChainIndex> CellProvider for Chain<CS> {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta(&out_point.hash) {
            if index < meta.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    CellStatus::Current(transaction.outputs.swap_remove(index))
                } else {
                    CellStatus::Old
                }
            } else {
                CellStatus::Unknown
            }
        } else {
            CellStatus::Unknown
        }
    }

    fn cell_at(&self, out_point: &OutPoint, parent: &H256) -> CellStatus {
        let index = out_point.index as usize;
        if let Some(meta) = self.get_transaction_meta_at(&out_point.hash, parent) {
            if index < meta.len() {
                if !meta.is_spent(index) {
                    let mut transaction = self
                        .store
                        .get_transaction(&out_point.hash)
                        .expect("transaction must exist");
                    CellStatus::Current(transaction.outputs.swap_remove(index))
                } else {
                    CellStatus::Old
                }
            } else {
                CellStatus::Unknown
            }
        } else {
            CellStatus::Unknown
        }
    }
}

impl<CS: ChainIndex> Chain<CS> {
    pub fn init(store: CS, consensus: Consensus, notify: Notify) -> Result<Chain<CS>, Error> {
        // check head in store or save the genesis block as head
        let header = {
            let genesis = consensus.genesis_block();
            match store.get_tip_header() {
                Some(h) => h,
                None => {
                    store.init(&genesis);
                    genesis.header.clone()
                }
            }
        };

        let output_root = match store.get_output_root(&header.hash()) {
            Some(h) => h,
            None => H256::zero(),
        };

        let total_difficulty = store
            .get_block_ext(&header.hash())
            .expect("block_ext stored")
            .total_difficulty;

        let tip_header = TipHeader {
            header,
            total_difficulty,
            output_root,
        };

        Ok(Chain {
            store,
            consensus,
            tip_header: RwLock::new(tip_header),
            candidate_uncles: Default::default(),
            notify,
        })
    }

    fn check_transactions(&self, batch: &mut Batch, b: &IndexedBlock) -> Result<H256, Error> {
        let mut cells = Vec::with_capacity(b.commit_transactions.len());

        for tx in &b.commit_transactions {
            let ins = if tx.is_cellbase() {
                Vec::new()
            } else {
                tx.input_pts()
            };
            let outs = tx.output_pts();

            cells.push((ins, outs));
        }

        let root = self
            .output_root(&b.header.parent_hash)
            .ok_or(Error::InvalidOutput)?;

        self.store
            .update_transaction_meta(batch, root, cells)
            .ok_or(Error::InvalidOutput)
    }

    fn insert_block(&self, b: &IndexedBlock) -> Result<BlockInsertionResult, Error> {
        let mut new_best_block = false;
        let mut old_cumulative_blks = Vec::new();
        let mut new_cumulative_blks = Vec::new();
        self.store.save_with_batch(|batch| {
            let root = self.check_transactions(batch, b)?;
            let parent_ext = self
                .store
                .get_block_ext(&b.header.parent_hash)
                .expect("parent already store");
            let cannon_total_difficulty = parent_ext.total_difficulty + b.header.difficulty;

            let ext = BlockExt {
                received_at: now_ms(),
                total_difficulty: cannon_total_difficulty,
                total_uncles_count: parent_ext.total_uncles_count + b.uncles().len() as u64,
            };

            self.store.insert_block(batch, b);
            self.store.insert_output_root(batch, b.hash(), root);
            self.store.insert_block_ext(batch, &b.hash(), &ext);

            {
                debug!(target: "chain", "acquire lock");
                let mut tip_header = self.tip_header.write();
                let current_total_difficulty = tip_header.total_difficulty;
                debug!(
                    "difficulty diff = {}; current = {}, cannon = {}",
                    cannon_total_difficulty.low_u64() as i64
                        - current_total_difficulty.low_u64() as i64,
                    current_total_difficulty,
                    cannon_total_difficulty,
                );

                if cannon_total_difficulty > current_total_difficulty
                    || (current_total_difficulty == cannon_total_difficulty
                        && b.hash() < tip_header.header.hash())
                {
                    debug!(target: "chain", "new best block found: {} => {}", b.header().number, b.hash());
                    new_best_block = true;
                    let new_tip_header = TipHeader {
                        header: b.header.clone(),
                        total_difficulty: cannon_total_difficulty,
                        output_root: root,
                    };

                    self.update_index(batch, tip_header.number(), b, &mut old_cumulative_blks, &mut new_cumulative_blks);
                    // TODO: Move out
                    *tip_header = new_tip_header;
                    self.store.insert_tip_header(batch, &b.header);
                    self.store.rebuild_tree(root);
                }
                debug!(target: "chain", "lock release");
            }
            Ok(())
        })?;

        Ok(BlockInsertionResult {
            new_best_block,
            fork_blks: ForkBlocks::new(old_cumulative_blks, new_cumulative_blks),
        })
    }

    fn post_insert_result(&self, block: &IndexedBlock, result: BlockInsertionResult) {
        let BlockInsertionResult {
            new_best_block,
            mut fork_blks,
        } = result;
        if !fork_blks.old_blks().is_empty() {
            fork_blks.push_new(block.clone());
            self.notify.notify_switch_fork(fork_blks);
        }

        if new_best_block {
            self.notify.notify_new_tip(block);
            if log_enabled!(target: "chain", log::Level::Debug) {
                self.print_chain(10);
            }
        } else {
            self.candidate_uncles
                .write()
                .insert(block.hash(), Arc::new(block.clone()));
        }
    }

    // we found new best_block total_difficulty > old_chain.total_difficulty
    pub fn update_index(
        &self,
        batch: &mut Batch,
        tip_number: BlockNumber,
        block: &IndexedBlock,
        old_cumulative_blks: &mut Vec<IndexedBlock>,
        new_cumulative_blks: &mut Vec<IndexedBlock>,
    ) {
        let mut number = block.header.number - 1;

        // The old fork may longer than new fork
        if number < tip_number {
            for n in number..tip_number + 1 {
                let hash = self.block_hash(n).unwrap();
                let old_block = self.block(&hash).unwrap();
                self.store.delete_block_hash(batch, n);
                self.store.delete_block_number(batch, &hash);
                self.store
                    .delete_transaction_address(batch, &old_block.commit_transactions());

                old_cumulative_blks.push(old_block);
            }
        }

        // The best block should always be insert
        {
            let number = block.header.number;
            let hash = block.hash();
            self.store.insert_block_hash(batch, number, &hash);
            self.store.insert_block_number(batch, &hash, number);
            self.store
                .insert_transaction_address(batch, &hash, &block.commit_transactions());
        }

        let mut hash = block.header.parent_hash;

        loop {
            if let Some(old_hash) = self.block_hash(number) {
                if old_hash == hash {
                    break;
                }
                let old_block = self.block(&old_hash).unwrap();

                self.store.delete_block_hash(batch, number);
                self.store.delete_block_number(batch, &old_hash);
                self.store
                    .delete_transaction_address(batch, &old_block.commit_transactions());
                old_cumulative_blks.push(old_block);
            }

            let new_block = self.block(&hash).unwrap();

            self.store.insert_block_hash(batch, number, &hash);
            self.store.insert_block_number(batch, &hash, number);
            self.store
                .insert_transaction_address(batch, &hash, &new_block.commit_transactions());

            hash = new_block.header.parent_hash;
            number -= 1;

            new_cumulative_blks.push(new_block);
        }

        new_cumulative_blks.reverse();
    }

    fn print_chain(&self, len: u64) {
        debug!(target: "chain", "Chain {{");

        let tip = { self.tip_header().read().header.number };
        let bottom = tip - cmp::min(tip, len);

        for number in (bottom..tip + 1).rev() {
            let hash = self.block_hash(number).unwrap_or_else(|| {
                panic!(format!("invaild block number({}), tip={}", number, tip))
            });
            debug!(target: "chain", "   {} => {}", number, hash);
        }

        debug!(target: "chain", "}}");

        // TODO: remove me when block explorer is available
        debug!(target: "chain", "Tx in Head Block {{");
        for transaction in self
            .block_hash(tip)
            .and_then(|hash| self.store.get_block_body(&hash))
            .expect("invalid block number")
        {
            debug!(target: "chain", "   {} => {:?}", transaction.hash(), transaction);
        }
        debug!(target: "chain", "}}");

        debug!(target: "chain", "Uncle block {{");
        for (index, uncle) in self
            .block_hash(tip)
            .and_then(|hash| self.store.get_block_uncles(&hash))
            .expect("invalid block number")
            .iter()
            .enumerate()
        {
            debug!(target: "chain", "   {} => {:#?}", index, uncle);
        }
        debug!(target: "chain", "}}");
    }
}

impl<CS: ChainIndex> ChainProvider for Chain<CS> {
    fn process_block(&self, b: &IndexedBlock) -> Result<(), Error> {
        debug!(target: "chain", "begin processing block: {}", b.hash());
        let insert_result = self.insert_block(b)?;
        self.post_insert_result(b, insert_result);
        debug!(target: "chain", "finish processing block");
        Ok(())
    }

    fn block(&self, hash: &H256) -> Option<IndexedBlock> {
        self.store.get_block(hash)
    }

    fn block_body(&self, hash: &H256) -> Option<Vec<IndexedTransaction>> {
        self.store.get_block_body(hash)
    }

    fn block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>> {
        self.store.get_block_proposal_txs_ids(hash)
    }

    fn uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>> {
        self.store.get_block_uncles(hash)
    }

    fn union_proposal_ids_n(&self, bn: BlockNumber, n: usize) -> Vec<Vec<ProposalShortId>> {
        let m = if bn > n as u64 { n } else { bn as usize };
        let mut ret = Vec::new();

        if let Some(mut hash) = self.block_hash(bn) {
            for _ in 0..m {
                let mut ids_set = FnvHashSet::default();

                if let Some(ids) = self.block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = self.uncles(&hash) {
                    for u in us {
                        let ids = u.proposal_transactions;
                        ids_set.extend(ids);
                    }
                }

                let ids_vec: Vec<ProposalShortId> = ids_set.into_iter().collect();
                ret.push(ids_vec);

                hash = self.block_header(&hash).unwrap().parent_hash;
            }
        }

        ret
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
        self.consensus.genesis_block().hash()
    }

    fn consensus(&self) -> &Consensus {
        &self.consensus
    }

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<IndexedHeader> {
        {
            let tip = self.tip_header().read();
            if let Some(n_number) = self.block_number(base) {
                if number > n_number {
                    return None;
                } else if number == n_number {
                    return Some(tip.header.clone());
                } else {
                    return self
                        .block_hash(number)
                        .and_then(|hash| self.block_header(&hash));
                }
            }
        }
        if let Some(header) = self.block_header(base) {
            let mut n_number = header.number;
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.block_header(&index_walk.parent_hash) {
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

    fn block_header(&self, hash: &H256) -> Option<IndexedHeader> {
        let tip_header = self.tip_header.read();
        if &tip_header.header.hash() == hash {
            Some(tip_header.header.clone())
        } else {
            self.store.get_header(hash)
        }
    }

    // TODO: Move to uncle provider
    fn get_tip_uncles(&self) -> Vec<UncleBlock> {
        let max_uncles_age = self.consensus().max_uncles_age();
        let header = &self.tip_header().read().header;
        let mut excluded = FnvHashSet::default();

        // cB
        // tip      1 depth, valid uncle
        // tip.p^0  ---/  2
        // tip.p^1  -----/  3
        // tip.p^2  -------/  4
        // tip.p^3  ---------/  5
        // tip.p^4  -----------/  6
        // tip.p^5  -------------/
        // tip.p^6
        let mut block_hash = header.hash();
        excluded.insert(block_hash);
        for _depth in 0..max_uncles_age {
            if let Some(block) = self.block(&block_hash) {
                excluded.insert(block.header.parent_hash);
                for uncle in block.uncles() {
                    excluded.insert(uncle.header.hash());
                }

                block_hash = block.header.parent_hash;
            } else {
                break;
            }
        }

        let tip_difficulty_epoch =
            header.raw.number / self.consensus().difficulty_adjustment_interval();

        let max_uncles_len = self.consensus().max_uncles_len();
        let mut included = FnvHashSet::default();
        let mut uncles = Vec::with_capacity(max_uncles_len);
        let mut bad_uncles = Vec::new();
        let r_candidate_uncle = self.candidate_uncles.upgradable_read();
        let current_number = self.tip_header().read().header.number + 1;
        for (hash, block) in r_candidate_uncle.iter() {
            if uncles.len() == max_uncles_len {
                break;
            }

            let block_difficulty_epoch =
                block.number() / self.consensus().difficulty_adjustment_interval();

            // uncle must be same difficulty epoch with tip
            if !block.header.difficulty == header.difficulty
                || !block_difficulty_epoch == tip_difficulty_epoch
            {
                bad_uncles.push(*hash);
                continue;
            }

            let depth = current_number.saturating_sub(block.number());
            if depth > max_uncles_age as u64
                || depth < 1
                || included.contains(hash)
                || excluded.contains(hash)
            {
                bad_uncles.push(*hash);
            } else if let Some(cellbase) = block.commit_transactions.first() {
                let uncle = UncleBlock {
                    header: block.header.header.clone(),
                    cellbase: cellbase.clone().into(),
                    proposal_transactions: block.proposal_transactions.clone(),
                };
                uncles.push(uncle);
                included.insert(*hash);
            } else {
                bad_uncles.push(*hash);
            }
        }

        if !bad_uncles.is_empty() {
            let mut w_candidate_uncles = RwLockUpgradableReadGuard::upgrade(r_candidate_uncle);
            for bad in bad_uncles {
                w_candidate_uncles.remove(&bad);
            }
        }

        uncles
    }

    fn tip_header(&self) -> &RwLock<TipHeader> {
        &self.tip_header
    }

    fn output_root(&self, hash: &H256) -> Option<H256> {
        self.store.get_output_root(hash)
    }

    fn get_transaction(&self, hash: &H256) -> Option<IndexedTransaction> {
        self.store.get_transaction(hash)
    }

    fn contain_transaction(&self, hash: &H256) -> bool {
        self.store.get_transaction_address(hash).is_some()
    }

    fn get_transaction_meta(&self, hash: &H256) -> Option<TransactionMeta> {
        self.store
            .get_transaction_meta(self.tip_header.read().output_root, *hash)
    }

    fn get_transaction_meta_at(&self, hash: &H256, parent: &H256) -> Option<TransactionMeta> {
        self.output_root(parent)
            .and_then(|root| self.store.get_transaction_meta(root, *hash))
    }

    fn block_reward(&self, _block_number: BlockNumber) -> Capacity {
        // TODO: block reward calculation algorithm
        self.consensus.initial_block_reward()
    }

    // TODO: find a way to write test for this once we can build a mock on
    // ChainIndex
    fn calculate_transaction_fee(&self, transaction: &Transaction) -> Result<Capacity, Error> {
        let mut fee = 0;
        for input in &transaction.inputs {
            let previous_output = &input.previous_output;
            match self.get_transaction(&previous_output.hash) {
                Some(previous_transaction) => {
                    let index = previous_output.index as usize;
                    if index < previous_transaction.outputs.len() {
                        fee += previous_transaction.outputs[index].capacity;
                    } else {
                        return Err(Error::InvalidInput);
                    }
                }
                None => return Err(Error::InvalidInput),
            }
        }
        let spent_capacity: Capacity = transaction
            .outputs
            .iter()
            .map(|output| output.capacity)
            .sum();
        if spent_capacity > fee {
            return Err(Error::InvalidOutput);
        }
        fee -= spent_capacity;
        Ok(fee)
    }

    // T_interval = L / C_m
    // HR_m = HR_last/ (1 + o)
    // Diff= HR_m * T_interval / H = Diff_last * o_last / o
    fn calculate_difficulty(&self, last: &IndexedHeader) -> Option<U256> {
        let interval = self.consensus().difficulty_adjustment_interval();

        if (last.number + 1) % interval != 0 {
            return Some(last.difficulty);
        }

        let start = last.number.saturating_sub(interval);
        if let Some(start_header) = self.get_ancestor(&last.hash(), start) {
            let start_total_uncles_count = self
                .block_ext(&start_header.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let last_total_uncles_count = self
                .block_ext(&last.hash())
                .expect("block_ext exist")
                .total_uncles_count;

            let difficulty = last.difficulty
                * U256::from(last_total_uncles_count - start_total_uncles_count)
                * U256::from((1.0 / self.consensus().orphan_rate_target()) as u64)
                / U256::from(interval);

            let min_difficulty = self.consensus().min_difficulty();
            let max_difficulty = last.difficulty * 2;
            if difficulty > max_difficulty {
                return Some(max_difficulty);
            }

            if difficulty < min_difficulty {
                return Some(min_difficulty);
            }
            return Some(difficulty);
        }
        None
    }
}

pub struct ChainBuilder<CS> {
    store: CS,
    consensus: Consensus,
    notify: Option<Notify>,
}

impl<CS: ChainIndex> ChainBuilder<CS> {
    pub fn new_memory() -> ChainBuilder<ChainKVStore<MemoryKeyValueDB>> {
        let db = MemoryKeyValueDB::open(COLUMNS as usize);
        ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_simple(db)
    }

    pub fn new_rocks<P: AsRef<Path>>(path: P) -> ChainBuilder<ChainKVStore<CacheDB<RocksDB>>> {
        let db = CacheDB::new(
            RocksDB::open(path, COLUMNS),
            &[(COLUMN_BLOCK_HEADER.unwrap(), 4096)],
        );
        ChainBuilder::<ChainKVStore<CacheDB<RocksDB>>>::new_simple(db)
    }

    pub fn new_simple<T: 'static + KeyValueDB>(db: T) -> ChainBuilder<ChainKVStore<T>> {
        let mut consensus = Consensus::default();
        consensus.initial_block_reward = 50;
        ChainBuilder {
            store: ChainKVStore::new(db),
            consensus,
            notify: None,
        }
    }

    // pub fn config(mut self, value: Config) -> Self {
    //     self.config = value;
    //     self
    // }

    // pub fn get_config(&self) -> &Config {
    //     &self.config
    // }

    pub fn consensus(mut self, value: Consensus) -> Self {
        self.consensus = value;
        self
    }

    pub fn get_consensus(&self) -> &Consensus {
        &self.consensus
    }

    pub fn notify(mut self, value: Notify) -> Self {
        self.notify = Some(value);
        self
    }

    pub fn build(self) -> Result<Chain<CS>, Error> {
        let notify = self.notify.unwrap_or_else(Notify::new);
        Chain::init(self.store, self.consensus, notify)
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    use consensus::GenesisBuilder;
    use core::header::{Header, RawHeader, Seal};
    use core::transaction::{
        CellInput, CellOutput, IndexedTransaction, ProposalShortId, Transaction, VERSION,
    };
    use core::uncle::UncleBlock;
    use db::memorydb::MemoryKeyValueDB;
    use store::ChainKVStore;

    fn create_cellbase(number: BlockNumber) -> IndexedTransaction {
        let inputs = vec![CellInput::new_cellbase_input(number)];
        let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
        Transaction::new(VERSION, Vec::new(), inputs, outputs).into()
    }

    fn gen_block(parent_header: IndexedHeader, nonce: u64, difficulty: U256) -> IndexedBlock {
        let time = now_ms();
        let number = parent_header.number + 1;
        let cellbase = create_cellbase(number);
        let header = Header {
            raw: RawHeader {
                number,
                version: 0,
                parent_hash: parent_header.hash(),
                timestamp: time,
                txs_commit: H256::zero(),
                txs_proposal: H256::zero(),
                difficulty: difficulty,
                cellbase_id: H256::zero(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce,
                proof: Default::default(),
            },
        };

        IndexedBlock {
            header: header.into(),
            uncles: vec![],
            commit_transactions: vec![cellbase],
            proposal_transactions: vec![ProposalShortId::from_slice(&[1; 10]).unwrap()],
        }
    }

    fn create_transaction(parent: H256) -> IndexedTransaction {
        let mut output = CellOutput::default();
        output.capacity = 100_000_000 / 100 as u64;
        let outputs: Vec<CellOutput> = vec![output.clone(); 100];

        Transaction::new(
            0,
            vec![],
            vec![CellInput::new(OutPoint::new(parent, 0), Default::default())],
            outputs,
        ).into()
    }

    #[test]
    fn test_genesis_transaction_spend() {
        let tx: IndexedTransaction = Transaction::new(
            0,
            vec![],
            vec![CellInput::new(OutPoint::null(), Default::default())],
            vec![CellOutput::new(100_000_000, vec![], H256::default()); 100],
        ).into();
        let mut root_hash = tx.hash();

        let genesis_builder = GenesisBuilder::default();
        let genesis_block = genesis_builder
            .difficulty(U256::from(1000))
            .add_commit_transaction(tx)
            .build();

        let consensus = Consensus::default().set_genesis_block(genesis_block);
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap();

        let end = 21;

        let mut blocks1: Vec<IndexedBlock> = vec![];
        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..end {
            let difficulty = parent.difficulty;
            let tx = create_transaction(root_hash);
            root_hash = tx.hash();
            let mut new_block = gen_block(parent, i, difficulty + U256::from(1));
            new_block.commit_transactions.push(tx);
            blocks1.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &blocks1[0..10] {
            assert!(chain.process_block(&block).is_ok());
        }
    }

    #[test]
    fn test_genesis_transaction_fetch() {
        let tx: IndexedTransaction = Transaction::new(
            0,
            vec![],
            vec![CellInput::new(OutPoint::null(), Default::default())],
            vec![CellOutput::new(100_000_000, vec![], H256::default()); 100],
        ).into();
        let root_hash = tx.hash();

        let genesis_builder = GenesisBuilder::default();
        let genesis_block = genesis_builder
            .difficulty(U256::from(1000))
            .add_commit_transaction(tx)
            .build();

        let consensus = Consensus::default().set_genesis_block(genesis_block);
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus)
            .build()
            .unwrap();

        let outpoint = OutPoint::new(root_hash, 0);
        let state = chain.cell(&outpoint);
        assert!(state.is_current());
    }

    #[test]
    fn test_chain_fork_by_total_difficulty() {
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100));
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let j = if i > 10 { 110 } else { 99 };
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(j));
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain.process_block(&block).expect("process block ok");
        }

        for block in &chain2 {
            chain.process_block(&block).expect("process block ok");
        }
        assert_eq!(chain.block_hash(8), chain2.get(7).map(|b| b.hash()));
    }

    #[test]
    fn test_chain_fork_by_hash() {
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100));
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(100));
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain.process_block(&block).expect("process block ok");
        }

        for block in &chain2 {
            chain.process_block(&block).expect("process block ok");
        }

        //if total_difficulty equal, we chose block which have smaller hash as best
        assert!(
            chain1
                .iter()
                .zip(chain2.iter())
                .all(|(a, b)| a.header.difficulty == b.header.difficulty)
        );

        let best = if chain1[(final_number - 2) as usize].hash()
            < chain2[(final_number - 2) as usize].hash()
        {
            chain1
        } else {
            chain2
        };
        assert_eq!(chain.block_hash(8), best.get(7).map(|b| b.hash()));
        assert_eq!(chain.block_hash(19), best.get(18).map(|b| b.hash()));
    }

    #[test]
    fn test_chain_get_ancestor() {
        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .build()
            .unwrap();
        let final_number = 20;

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i, difficulty + U256::from(100));
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = parent.difficulty;
            let new_block = gen_block(parent, i + 1000, difficulty + U256::from(100));
            chain2.push(new_block.clone());
            parent = new_block.header;
        }

        for block in &chain1 {
            chain.process_block(&block).expect("process block ok");
        }

        for block in &chain2 {
            chain.process_block(&block).expect("process block ok");
        }

        assert_eq!(
            chain1[9].header,
            chain
                .get_ancestor(&chain1.last().unwrap().hash(), 10)
                .unwrap()
        );

        assert_eq!(
            chain2[9].header,
            chain
                .get_ancestor(&chain2.last().unwrap().hash(), 10)
                .unwrap()
        );
    }

    fn push_uncle(block: &mut IndexedBlock, uncle: &IndexedBlock) {
        let uncle = UncleBlock {
            header: uncle.header.header.clone(),
            cellbase: uncle.commit_transactions.first().cloned().unwrap().into(),
            proposal_transactions: uncle.proposal_transactions.clone(),
        };

        block.uncles.push(uncle);
        block.header.uncles_hash = block.cal_uncles_hash();
        block.finalize_dirty();
    }

    #[test]
    fn test_calculate_difficulty() {
        let genesis_builder = GenesisBuilder::default();
        let genesis_block = genesis_builder.difficulty(U256::from(1000)).build();
        let mut consensus = Consensus::default().set_genesis_block(genesis_block);
        consensus.pow_time_span = 200;
        consensus.pow_spacing = 1;

        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let final_number = chain.consensus().difficulty_adjustment_interval();

        let mut chain1: Vec<IndexedBlock> = Vec::new();
        let mut chain2: Vec<IndexedBlock> = Vec::new();

        let mut parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number - 1 {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let new_block = gen_block(parent, i, difficulty);
            chain.process_block(&new_block).expect("process block ok");
            chain1.push(new_block.clone());
            parent = new_block.header;
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let mut new_block = gen_block(parent, i + 100, difficulty);
            if i < 26 {
                push_uncle(&mut new_block, &chain1[i as usize]);
            }
            chain.process_block(&new_block).expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header;
        }
        let tip = { chain.tip_header().read().header.clone() };
        let total_uncles_count = chain.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 25);
        let difficulty = chain.calculate_difficulty(&tip).unwrap();

        // 25 * 10 * 1000 / 200
        assert_eq!(difficulty, U256::from(1250));

        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let mut chain2: Vec<IndexedBlock> = Vec::new();
        for i in 1..final_number - 1 {
            chain
                .process_block(&chain1[(i - 1) as usize])
                .expect("process block ok");
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let mut new_block = gen_block(parent, i + 100, difficulty);
            if i < 11 {
                push_uncle(&mut new_block, &chain1[i as usize]);
            }
            chain.process_block(&new_block).expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header;
        }
        let tip = { chain.tip_header().read().header.clone() };
        let total_uncles_count = chain.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 10);
        let difficulty = chain.calculate_difficulty(&tip).unwrap();

        // min[10 * 10 * 1000 / 200, 1000]
        assert_eq!(difficulty, U256::from(1000));

        let chain = ChainBuilder::<ChainKVStore<MemoryKeyValueDB>>::new_memory()
            .consensus(consensus.clone())
            .build()
            .unwrap();
        let mut chain2: Vec<IndexedBlock> = Vec::new();
        for i in 1..final_number - 1 {
            chain
                .process_block(&chain1[(i - 1) as usize])
                .expect("process block ok");
        }

        parent = chain.block_header(&chain.block_hash(0).unwrap()).unwrap();
        for i in 1..final_number {
            let difficulty = chain.calculate_difficulty(&parent).unwrap();
            let mut new_block = gen_block(parent, i + 100, difficulty);
            if i < 151 {
                push_uncle(&mut new_block, &chain1[i as usize]);
            }
            chain.process_block(&new_block).expect("process block ok");
            chain2.push(new_block.clone());
            parent = new_block.header;
        }
        let tip = { chain.tip_header().read().header.clone() };
        let total_uncles_count = chain.block_ext(&tip.hash()).unwrap().total_uncles_count;
        assert_eq!(total_uncles_count, 150);
        let difficulty = chain.calculate_difficulty(&tip).unwrap();

        // max[150 * 10 * 1000 / 200, 2 * 1000]
        assert_eq!(difficulty, U256::from(2000));
    }
}
