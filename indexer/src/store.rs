use crate::types::{
    CellTransaction, LiveCell, LockHashCellOutput, LockHashIndex, LockHashIndexState,
    TransactionPoint,
};
use bincode::{deserialize, serialize};
use ckb_core::block::Block;
use ckb_core::transaction::{CellOutPoint, CellOutput};
use ckb_core::BlockNumber;
use ckb_db::{
    rocksdb::{RocksDB, RocksdbBatch},
    Col, DBConfig, DbBatch, Direction, IterableKeyValueDB, KeyValueDB,
};
use ckb_logger::{debug, error, trace};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use numext_fixed_hash::H256;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const BATCH_ATTACH_BLOCK_NUMS: usize = 100;
const SYNC_INTERVAL: Duration = Duration::from_secs(1);
const COLUMNS: u32 = 4;

/// +---------------------------------+---------------+--------------------------+
/// |             Column              |      Key      |          Value           |
/// +---------------------------------+---------------+--------------------------+
/// | COLUMN_LOCK_HASH_INDEX_STATE    | H256          | LockHashIndexState       |
/// | COLUMN_LOCK_HASH_LIVE_CELL      | LockHashIndex | CellOutput               |
/// | COLUMN_LOCK_HASH_TRANSACTION    | LockHashIndex | Option<TransactionPoint> |
/// | COLUMN_CELL_OUT_POINT_LOCK_HASH | CellOutPoint  | LockHashCellOutput       |
/// +---------------------------------+---------------+--------------------------+

const COLUMN_LOCK_HASH_INDEX_STATE: Col = 0;
const COLUMN_LOCK_HASH_LIVE_CELL: Col = 1;
const COLUMN_LOCK_HASH_TRANSACTION: Col = 2;
const COLUMN_CELL_OUT_POINT_LOCK_HASH: Col = 3;

pub trait IndexerStore: Sync + Send {
    fn get_live_cells(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<LiveCell>;

    fn get_transactions(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<CellTransaction>;

    fn get_lock_hash_index_states(&self) -> HashMap<H256, LockHashIndexState>;

    fn insert_lock_hash(
        &self,
        lock_hash: &H256,
        index_from: Option<BlockNumber>,
    ) -> LockHashIndexState;

    fn remove_lock_hash(&self, lock_hash: &H256);
}

pub struct DefaultIndexerStore<CS> {
    db: Arc<RocksDB>,
    shared: Shared<CS>,
}

impl<CS: ChainStore> Clone for DefaultIndexerStore<CS> {
    fn clone(&self) -> Self {
        DefaultIndexerStore {
            db: Arc::clone(&self.db),
            shared: self.shared.clone(),
        }
    }
}

impl<CS: ChainStore + 'static> IndexerStore for DefaultIndexerStore<CS> {
    fn get_live_cells(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<LiveCell> {
        let mut from_key = lock_hash.to_vec();
        let iter = if reverse_order {
            from_key.extend_from_slice(&BlockNumber::max_value().to_be_bytes());
            self.db
                .iter(COLUMN_LOCK_HASH_LIVE_CELL, &from_key, Direction::Reverse)
        } else {
            self.db
                .iter(COLUMN_LOCK_HASH_LIVE_CELL, &from_key, Direction::Forward)
        };
        iter.expect("indexer db iter should be ok")
            .skip(skip_num)
            .take(take_num)
            .take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
            .map(|(key, value)| {
                let cell_output: CellOutput =
                    deserialize(&value).expect("deserialize CellOutput should be ok");
                let lock_hash_index = LockHashIndex::from_slice(&key);
                LiveCell {
                    created_by: lock_hash_index.into(),
                    cell_output,
                }
            })
            .collect()
    }

    fn get_transactions(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<CellTransaction> {
        let mut from_key = lock_hash.to_vec();
        let iter = if reverse_order {
            from_key.extend_from_slice(&BlockNumber::max_value().to_be_bytes());
            self.db
                .iter(COLUMN_LOCK_HASH_TRANSACTION, &from_key, Direction::Reverse)
        } else {
            self.db
                .iter(COLUMN_LOCK_HASH_TRANSACTION, &from_key, Direction::Forward)
        };
        iter.expect("indexer db iter should be ok")
            .skip(skip_num)
            .take(take_num)
            .take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
            .map(|(key, value)| {
                let consumed_by: Option<TransactionPoint> =
                    deserialize(&value).expect("deserialize TransactionPoint should be ok");
                let lock_hash_index = LockHashIndex::from_slice(&key);
                CellTransaction {
                    created_by: lock_hash_index.into(),
                    consumed_by,
                }
            })
            .collect()
    }

    fn get_lock_hash_index_states(&self) -> HashMap<H256, LockHashIndexState> {
        self.db
            .iter(COLUMN_LOCK_HASH_INDEX_STATE, &[], Direction::Forward)
            .expect("indexer db iter should be ok")
            .map(|(key, value)| {
                (
                    H256::from_slice(&key).expect("db safe access"),
                    deserialize(&value).expect("deserialize LockHashIndexState should be ok"),
                )
            })
            .collect()
    }

    fn insert_lock_hash(
        &self,
        lock_hash: &H256,
        index_from: Option<BlockNumber>,
    ) -> LockHashIndexState {
        let index_state = {
            let tip_number = self
                .shared
                .store()
                .get_tip_header()
                .expect("tip header exists")
                .number();
            let block_number = index_from.unwrap_or_else(|| tip_number).min(tip_number);
            LockHashIndexState {
                block_number,
                block_hash: self
                    .shared
                    .store()
                    .get_block_hash(block_number)
                    .expect("block exists"),
            }
        };
        self.commit_batch(|batch| {
            batch.insert_lock_hash_index_state(lock_hash, &index_state);
        });
        index_state
    }

    fn remove_lock_hash(&self, lock_hash: &H256) {
        self.commit_batch(|batch| {
            let iter = self
                .db
                .iter(
                    COLUMN_LOCK_HASH_LIVE_CELL,
                    lock_hash.as_bytes(),
                    Direction::Forward,
                )
                .expect("indexer db iter should be ok");

            iter.take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
                .for_each(|(key, _)| {
                    let lock_hash_index = LockHashIndex::from_slice(&key);
                    batch.delete_lock_hash_live_cell(&lock_hash_index);
                    batch.delete_cell_out_point_lock_hash(&lock_hash_index.cell_out_point);
                });

            let iter = self
                .db
                .iter(
                    COLUMN_LOCK_HASH_TRANSACTION,
                    lock_hash.as_bytes(),
                    Direction::Forward,
                )
                .expect("indexer db iter should be ok");

            iter.take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
                .for_each(|(key, _)| {
                    let lock_hash_index = LockHashIndex::from_slice(&key);
                    batch.delete_lock_hash_transaction(&lock_hash_index);
                });

            batch.delete_lock_hash_index_state(&lock_hash);
        });
    }
}

impl<CS: ChainStore + 'static> DefaultIndexerStore<CS> {
    pub fn new(db_config: &DBConfig, shared: Shared<CS>) -> Self {
        let db = RocksDB::open(db_config, COLUMNS);
        DefaultIndexerStore {
            db: Arc::new(db),
            shared,
        }
    }

    pub fn start<S: ToString>(self, thread_name: Option<S>) {
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        thread_builder
            .spawn(move || loop {
                self.sync_index_states();
                thread::sleep(SYNC_INTERVAL);
            })
            .expect("start DefaultIndexerStore failed");
    }

    // helper function
    fn commit_batch<F>(&self, process: F)
    where
        F: FnOnce(&mut IndexerStoreBatch),
    {
        match self.db.batch() {
            Ok(batch) => {
                let mut batch = IndexerStoreBatch {
                    batch,
                    insert_buffer: HashMap::new(),
                    delete_buffer: HashSet::new(),
                };
                process(&mut batch);
                batch.commit();
            }
            Err(err) => {
                error!("indexer db failed to create new batch, error: {:?}", err);
            }
        }
    }

    pub fn sync_index_states(&self) {
        debug!("Start sync index states with chain store");
        let mut lock_hash_index_states = self.get_lock_hash_index_states();
        if lock_hash_index_states.is_empty() {
            return;
        }

        // retains the lock hashes on fork chain and detach blocks
        lock_hash_index_states.retain(|_, index_state| {
            self.shared
                .store()
                .get_block_number(&index_state.block_hash)
                != Some(index_state.block_number)
        });
        lock_hash_index_states
            .iter()
            .for_each(|(lock_hash, index_state)| {
                let mut index_lock_hashes = HashSet::new();
                index_lock_hashes.insert(lock_hash.to_owned());

                let mut block = self
                    .shared
                    .store()
                    .get_block(&index_state.block_hash)
                    .expect("block exists");
                // detach blocks until reach a block on main chain
                self.commit_batch(|batch| {
                    self.detach_block(batch, &index_lock_hashes, &block);
                    while self
                        .shared
                        .store()
                        .get_block_hash(block.header().number() - 1)
                        != Some(block.header().parent_hash().to_owned())
                    {
                        block = self
                            .shared
                            .store()
                            .get_block(block.header().parent_hash())
                            .expect("block exists");
                        self.detach_block(batch, &index_lock_hashes, &block);
                    }
                    let index_state = LockHashIndexState {
                        block_number: block.header().number() - 1,
                        block_hash: block.header().parent_hash().to_owned(),
                    };
                    batch.insert_lock_hash_index_state(lock_hash, &index_state);
                });
            });

        // attach blocks until reach tip or batch limit
        // need to check empty again because `remove_lock_hash` may be called during detach
        let mut lock_hash_index_states = self.get_lock_hash_index_states();
        if lock_hash_index_states.is_empty() {
            return;
        }
        let min_block_number: BlockNumber = lock_hash_index_states
            .values()
            .min_by_key(|index_state| index_state.block_number)
            .expect("none empty index states")
            .block_number;

        // should index genesis block also
        let start_number = if min_block_number == 0 {
            0
        } else {
            min_block_number + 1
        };

        let (tip_number, tip_hash) = {
            let tip_header = self
                .shared
                .store()
                .get_tip_header()
                .expect("tip header exists");
            (tip_header.number(), tip_header.hash().to_owned())
        };
        self.commit_batch(|batch| {
            (start_number..=tip_number)
                .take(BATCH_ATTACH_BLOCK_NUMS)
                .for_each(|block_number| {
                    let index_lock_hashes = lock_hash_index_states
                        .iter()
                        .filter(|(_, index_state)| index_state.block_number <= block_number)
                        .map(|(lock_hash, _)| lock_hash)
                        .cloned()
                        .collect();
                    let block = self
                        .shared
                        .store()
                        .get_ancestor(&tip_hash, block_number)
                        .and_then(|header| self.shared.store().get_block(&header.hash()))
                        .expect("block exists");
                    self.attach_block(batch, &index_lock_hashes, &block);
                    let index_state = LockHashIndexState {
                        block_number,
                        block_hash: block.header().hash().to_owned(),
                    };
                    index_lock_hashes.into_iter().for_each(|lock_hash| {
                        lock_hash_index_states.insert(lock_hash, index_state.clone());
                    })
                });

            lock_hash_index_states
                .iter()
                .for_each(|(lock_hash, index_state)| {
                    batch.insert_lock_hash_index_state(lock_hash, index_state);
                })
        });

        debug!("End sync index states with chain store");
    }

    fn detach_block(
        &self,
        batch: &mut IndexerStoreBatch,
        index_lock_hashes: &HashSet<H256>,
        block: &Block,
    ) {
        trace!("detach block {:x}", block.header().hash());
        let block_number = block.header().number();
        block.transactions().iter().rev().for_each(|tx| {
            let tx_hash = tx.hash();
            tx.outputs().iter().enumerate().for_each(|(index, output)| {
                let index = index as u32;
                let lock_hash = output.lock.hash();
                if index_lock_hashes.contains(&lock_hash) {
                    let lock_hash_index =
                        LockHashIndex::new(lock_hash, block_number, tx_hash.clone(), index);
                    batch.delete_lock_hash_live_cell(&lock_hash_index);
                    batch.delete_lock_hash_transaction(&lock_hash_index);
                    batch.delete_cell_out_point_lock_hash(&lock_hash_index.cell_out_point);
                }
            });

            if !tx.is_cellbase() {
                tx.inputs().iter().for_each(|input| {
                    if let Some(cell_out_point) = input.previous_output.cell.clone() {
                        if let Some(lock_hash_cell_output) =
                            batch.get_lock_hash_cell_output(&cell_out_point, &self.db)
                        {
                            if index_lock_hashes.contains(&lock_hash_cell_output.lock_hash) {
                                if let Some(cell_output) = lock_hash_cell_output.cell_output {
                                    let lock_hash_index = LockHashIndex::new(
                                        lock_hash_cell_output.lock_hash.clone(),
                                        lock_hash_cell_output.block_number,
                                        cell_out_point.tx_hash.clone(),
                                        cell_out_point.index,
                                    );
                                    batch.generate_live_cell(lock_hash_index, cell_output);
                                }
                            }
                        }
                    }
                });
            }
        })
    }

    fn attach_block(
        &self,
        batch: &mut IndexerStoreBatch,
        index_lock_hashes: &HashSet<H256>,
        block: &Block,
    ) {
        trace!("attach block {:x}", block.header().hash());
        let block_number = block.header().number();
        block.transactions().iter().for_each(|tx| {
            let tx_hash = tx.hash();
            if !tx.is_cellbase() {
                tx.inputs().iter().enumerate().for_each(|(index, input)| {
                    let index = index as u32;
                    if let Some(cell_out_point) = input.previous_output.cell.clone() {
                        if let Some(lock_hash_cell_output) =
                            batch.get_lock_hash_cell_output(&cell_out_point, &self.db)
                        {
                            if index_lock_hashes.contains(&lock_hash_cell_output.lock_hash) {
                                let lock_hash_index = LockHashIndex::new(
                                    lock_hash_cell_output.lock_hash,
                                    lock_hash_cell_output.block_number,
                                    cell_out_point.tx_hash,
                                    cell_out_point.index,
                                );
                                let consumed_by = TransactionPoint {
                                    block_number,
                                    tx_hash: tx_hash.clone(),
                                    index,
                                };
                                batch.consume_live_cell(lock_hash_index, consumed_by, &self.db);
                            }
                        }
                    }
                });
            }

            tx.outputs().iter().enumerate().for_each(|(index, output)| {
                let index = index as u32;
                let lock_hash = output.lock.hash();
                if index_lock_hashes.contains(&lock_hash) {
                    let lock_hash_index =
                        LockHashIndex::new(lock_hash.clone(), block_number, tx_hash.clone(), index);
                    batch.generate_live_cell(lock_hash_index, output.clone());
                }
            });
        })
    }
}

// rocksdb rust binding doesn't support transactional batch, have to use batch buffer as tranaction overlay here.
struct IndexerStoreBatch {
    pub batch: RocksdbBatch,
    pub insert_buffer: HashMap<CellOutPoint, LockHashCellOutput>,
    pub delete_buffer: HashSet<CellOutPoint>,
}

impl IndexerStoreBatch {
    fn generate_live_cell(&mut self, lock_hash_index: LockHashIndex, cell_output: CellOutput) {
        self.insert_lock_hash_live_cell(&lock_hash_index, &cell_output);
        self.insert_lock_hash_transaction(&lock_hash_index, &None);

        let mut lock_hash_cell_output = LockHashCellOutput {
            lock_hash: lock_hash_index.lock_hash.clone(),
            block_number: lock_hash_index.block_number,
            cell_output: None,
        };
        self.insert_cell_out_point_lock_hash(
            &lock_hash_index.cell_out_point,
            &lock_hash_cell_output,
        );
        lock_hash_cell_output.cell_output = Some(cell_output);
        self.delete_buffer.remove(&lock_hash_index.cell_out_point);
        self.insert_buffer
            .insert(lock_hash_index.cell_out_point, lock_hash_cell_output);
    }

    fn consume_live_cell(
        &mut self,
        lock_hash_index: LockHashIndex,
        consumed_by: TransactionPoint,
        db: &RocksDB,
    ) {
        if let Some(lock_hash_cell_output) = self
            .insert_buffer
            .get(&lock_hash_index.cell_out_point)
            .cloned()
            .or_else(|| {
                db.read(COLUMN_LOCK_HASH_LIVE_CELL, &lock_hash_index.to_vec())
                    .expect("indexer db read should be ok")
                    .map(|value| deserialize(&value).expect("deserialize CellOutput should be ok"))
                    .map(|cell_output| LockHashCellOutput {
                        lock_hash: lock_hash_index.lock_hash.clone(),
                        block_number: lock_hash_index.block_number,
                        cell_output,
                    })
            })
        {
            self.delete_lock_hash_live_cell(&lock_hash_index);
            self.insert_lock_hash_transaction(&lock_hash_index, &Some(consumed_by));
            self.insert_cell_out_point_lock_hash(
                &lock_hash_index.cell_out_point,
                &lock_hash_cell_output,
            );
        }
    }

    fn insert_lock_hash_index_state(&mut self, lock_hash: &H256, index_state: &LockHashIndexState) {
        self.batch
            .insert(
                COLUMN_LOCK_HASH_INDEX_STATE,
                lock_hash.as_bytes(),
                &serialize(index_state).expect("serialize LockHashIndexState should be ok"),
            )
            .expect("batch insert COLUMN_LOCK_HASH_INDEX_STATE failed");
    }

    fn insert_lock_hash_live_cell(
        &mut self,
        lock_hash_index: &LockHashIndex,
        cell_output: &CellOutput,
    ) {
        self.batch
            .insert(
                COLUMN_LOCK_HASH_LIVE_CELL,
                &lock_hash_index.to_vec(),
                &serialize(cell_output).expect("serialize CellOutput should be ok"),
            )
            .expect("batch insert COLUMN_LOCK_HASH_LIVE_CELL failed");
    }

    fn insert_lock_hash_transaction(
        &mut self,
        lock_hash_index: &LockHashIndex,
        consumed_by: &Option<TransactionPoint>,
    ) {
        self.batch
            .insert(
                COLUMN_LOCK_HASH_TRANSACTION,
                &lock_hash_index.to_vec(),
                &serialize(consumed_by).expect("serialize TransactionPoint should be ok"),
            )
            .expect("batch insert COLUMN_LOCK_HASH_TRANSACTION failed");
    }

    fn insert_cell_out_point_lock_hash(
        &mut self,
        cell_out_point: &CellOutPoint,
        lock_hash_cell_output: &LockHashCellOutput,
    ) {
        self.batch
            .insert(
                COLUMN_CELL_OUT_POINT_LOCK_HASH,
                &serialize(&cell_out_point).expect("serialize OutPoint should be ok"),
                &serialize(&lock_hash_cell_output)
                    .expect("serialize LockHashCellOutput should be ok"),
            )
            .expect("batch insert COLUMN_CELL_OUT_POINT_LOCK_HASH failed");
    }

    fn delete_lock_hash_index_state(&mut self, lock_hash: &H256) {
        self.batch
            .delete(COLUMN_LOCK_HASH_INDEX_STATE, lock_hash.as_bytes())
            .expect("batch delete COLUMN_LOCK_HASH_INDEX_STATE failed");
    }

    fn delete_lock_hash_live_cell(&mut self, lock_hash_index: &LockHashIndex) {
        self.batch
            .delete(COLUMN_LOCK_HASH_LIVE_CELL, &lock_hash_index.to_vec())
            .expect("batch delete COLUMN_LOCK_HASH_LIVE_CELL failed");
    }

    fn delete_lock_hash_transaction(&mut self, lock_hash_index: &LockHashIndex) {
        self.batch
            .delete(COLUMN_LOCK_HASH_TRANSACTION, &lock_hash_index.to_vec())
            .expect("batch delete COLUMN_LOCK_HASH_TRANSACTION failed");
    }

    fn delete_cell_out_point_lock_hash(&mut self, cell_out_point: &CellOutPoint) {
        self.batch
            .delete(
                COLUMN_CELL_OUT_POINT_LOCK_HASH,
                &serialize(cell_out_point).expect("serialize CellOutPoint should be ok"),
            )
            .expect("batch delete COLUMN_CELL_OUT_POINT_LOCK_HASH failed");
        self.insert_buffer.remove(cell_out_point);
        self.delete_buffer.insert(cell_out_point.clone());
    }

    fn get_lock_hash_cell_output(
        &self,
        cell_out_point: &CellOutPoint,
        db: &RocksDB,
    ) -> Option<LockHashCellOutput> {
        if self.delete_buffer.contains(cell_out_point) {
            None
        } else {
            self.insert_buffer.get(cell_out_point).cloned().or_else(|| {
                db.read(
                    COLUMN_CELL_OUT_POINT_LOCK_HASH,
                    &serialize(cell_out_point).expect("serialize OutPoint should be ok"),
                )
                .expect("indexer db read should be ok")
                .map(|value| {
                    deserialize(&value).expect("deserialize LockHashCellOutput should be ok")
                })
            })
        }
    }

    fn commit(self) {
        // only log the error, indexer store commit failure should not causing the thread to panic entirely.
        if let Err(err) = self.batch.commit() {
            error!("indexer db failed to commit batch, error: {:?}", err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_chain::chain::{ChainController, ChainService};
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, Bytes, Capacity};
    use ckb_db::{DBConfig, MemoryKeyValueDB};
    use ckb_notify::NotifyService;
    use ckb_resource::CODE_HASH_DAO;
    use ckb_shared::shared::{Shared, SharedBuilder};
    use ckb_store::ChainKVStore;
    use numext_fixed_uint::U256;
    use std::sync::Arc;
    use tempfile;

    fn setup(
        prefix: &str,
    ) -> (
        DefaultIndexerStore<ChainKVStore<MemoryKeyValueDB>>,
        ChainController,
        Shared<ChainKVStore<MemoryKeyValueDB>>,
    ) {
        let builder = SharedBuilder::<MemoryKeyValueDB>::new();
        let shared = builder.consensus(Consensus::default()).build().unwrap();

        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let notify = NotifyService::default().start::<&str>(None);
        let chain_service = ChainService::new(shared.clone(), notify);
        let chain_controller = chain_service.start::<&str>(None);
        (
            DefaultIndexerStore::new(&config, shared.clone()),
            chain_controller,
            shared,
        )
    }

    #[test]
    fn lock_hash_index() {
        let (store, _, _) = setup("lock_hash_index");
        store.insert_lock_hash(&CODE_HASH_DAO, None);
        store.insert_lock_hash(&H256::zero(), None);

        assert_eq!(2, store.get_lock_hash_index_states().len());

        store.remove_lock_hash(&CODE_HASH_DAO);
        assert_eq!(1, store.get_lock_hash_index_states().len());
    }

    #[test]
    fn get_live_cells() {
        let (store, chain, shared) = setup("get_live_cells");
        let script1 = Script::new(Vec::new(), CODE_HASH_DAO);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash(), None);
        store.insert_lock_hash(&script2.hash(), None);

        let tx11 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(1000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx12 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(2000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(1u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        let tx21 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(3000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx22 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(4000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21.clone())
            .transaction(tx22.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(2u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(5000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx12.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(6000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block2_fork = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(20u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        chain.process_block(Arc::new(block1), false).unwrap();
        chain.process_block(Arc::new(block2), false).unwrap();
        store.sync_index_states();

        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(2, cells.len());
        assert_eq!(capacity_bytes!(1000), cells[0].cell_output.capacity);
        assert_eq!(capacity_bytes!(3000), cells[1].cell_output.capacity);

        // test reverse order
        let cells = store.get_live_cells(&script1.hash(), 0, 100, true);
        assert_eq!(2, cells.len());
        assert_eq!(capacity_bytes!(3000), cells[0].cell_output.capacity);
        assert_eq!(capacity_bytes!(1000), cells[1].cell_output.capacity);

        let cells = store.get_live_cells(&script2.hash(), 0, 100, false);
        assert_eq!(2, cells.len());
        assert_eq!(capacity_bytes!(2000), cells[0].cell_output.capacity);
        assert_eq!(capacity_bytes!(4000), cells[1].cell_output.capacity);

        chain.process_block(Arc::new(block2_fork), false).unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(capacity_bytes!(5000), cells[0].cell_output.capacity);

        let cells = store.get_live_cells(&script2.hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(capacity_bytes!(6000), cells[0].cell_output.capacity);

        // remove script1's lock hash should remove its indexed data also
        store.remove_lock_hash(&script1.hash());
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cells = store.get_live_cells(&script2.hash(), 0, 100, false);
        assert_eq!(1, cells.len());
    }

    #[test]
    fn get_transactions() {
        let (store, chain, shared) = setup("get_transactions");
        let script1 = Script::new(Vec::new(), CODE_HASH_DAO);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash(), None);
        store.insert_lock_hash(&script2.hash(), None);

        let tx11 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(1000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx12 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(2000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(1u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        let tx21 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(3000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx22 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(4000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21.clone())
            .transaction(tx22.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(2u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(5000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx12.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(6000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block2_fork = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(20u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        chain.process_block(Arc::new(block1), false).unwrap();
        chain.process_block(Arc::new(block2), false).unwrap();
        store.sync_index_states();

        let transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx21.hash().to_owned(), transactions[1].created_by.tx_hash);

        // test reverse order
        let transactions = store.get_transactions(&script1.hash(), 0, 100, true);
        assert_eq!(2, transactions.len());
        assert_eq!(tx21.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx11.hash().to_owned(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx22.hash().to_owned(), transactions[1].created_by.tx_hash);

        chain.process_block(Arc::new(block2_fork), false).unwrap();
        store.sync_index_states();
        let transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(
            Some(tx31.hash().to_owned()),
            transactions[0]
                .consumed_by
                .as_ref()
                .map(|transaction_point| transaction_point.tx_hash.clone())
        );
        assert_eq!(tx31.hash().to_owned(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx32.hash().to_owned(), transactions[1].created_by.tx_hash);

        // remove script1's lock hash should remove its indexed data also
        store.remove_lock_hash(&script1.hash());
        let transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(0, transactions.len());
        let transactions = store.get_transactions(&script2.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
    }

    #[test]
    fn sync_index_states() {
        let (store, chain, shared) = setup("sync_index_states");
        let script1 = Script::new(Vec::new(), CODE_HASH_DAO);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash(), None);
        store.insert_lock_hash(&script2.hash(), None);

        let tx11 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(1000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx12 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(2000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(1u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        let tx21 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(3000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx22 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(4000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21.clone())
            .transaction(tx22.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(2u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(5000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx12.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(6000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block2_fork = BlockBuilder::default()
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(20u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        let block3 = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(21u64))
                    .number(3)
                    .parent_hash(block2_fork.header().hash().to_owned()),
            )
            .build();

        chain.process_block(Arc::new(block1), false).unwrap();
        chain.process_block(Arc::new(block2), false).unwrap();

        store.sync_index_states();

        let transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx21.hash().to_owned(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx22.hash().to_owned(), transactions[1].created_by.tx_hash);

        chain.process_block(Arc::new(block2_fork), false).unwrap();
        chain.process_block(Arc::new(block3), false).unwrap();

        store.sync_index_states();
        let transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(
            Some(tx31.hash().to_owned()),
            transactions[0]
                .consumed_by
                .as_ref()
                .map(|transaction_point| transaction_point.tx_hash.clone())
        );
        assert_eq!(tx31.hash().to_owned(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx32.hash().to_owned(), transactions[1].created_by.tx_hash);
    }

    #[test]
    fn consume_txs_in_same_block() {
        let (store, chain, shared) = setup("consume_txs_in_same_block");
        let script1 = Script::new(Vec::new(), CODE_HASH_DAO);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash(), None);
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());

        let tx11 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(1000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx12 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(900),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx13 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx12.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(800),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11)
            .transaction(tx12)
            .transaction(tx13)
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(1u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        let block1_fork = BlockBuilder::default()
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(20u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        chain.process_block(Arc::new(block1), false).unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(2, cell_transactions.len());

        chain.process_block(Arc::new(block1_fork), false).unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(0, cell_transactions.len());
    }

    #[test]
    fn detach_blocks() {
        let (store, chain, shared) = setup("detach_blocks");
        let script1 = Script::new(Vec::new(), CODE_HASH_DAO);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash(), None);
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());

        let tx11 = TransactionBuilder::default()
            .output(CellOutput::new(
                capacity_bytes!(1000),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx12 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(900),
                Bytes::new(),
                script1.clone(),
                None,
            ))
            .build();

        let tx21 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx12.hash().to_owned(), 0),
                0,
            ))
            .output(CellOutput::new(
                capacity_bytes!(800),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11)
            .transaction(tx12)
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(1u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21)
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(2u64))
                    .number(2)
                    .parent_hash(block1.header().hash().to_owned()),
            )
            .build();

        let block1_fork = BlockBuilder::default()
            .header_builder(
                HeaderBuilder::default()
                    .difficulty(U256::from(20u64))
                    .number(1)
                    .parent_hash(shared.genesis_hash().to_owned()),
            )
            .build();

        chain.process_block(Arc::new(block1), false).unwrap();
        chain.process_block(Arc::new(block2), false).unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(2, cell_transactions.len());

        chain.process_block(Arc::new(block1_fork), false).unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.hash(), 0, 100, false);
        assert_eq!(0, cell_transactions.len());
    }
}
