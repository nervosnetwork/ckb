use crate::migrations;
use crate::types::{
    CellTransaction, LiveCell, LockHashCapacity, LockHashCellOutput, LockHashIndex,
    LockHashIndexState, TransactionPoint,
};
use ckb_app_config::IndexerConfig;
use ckb_db::{db::RocksDB, DBIterator, Direction, IteratorMode, RocksDBTransaction};
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_db_schema::Col;
use ckb_logger::{debug, error, trace};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_types::{
    core::{self, BlockNumber, Capacity},
    packed::{self, Byte32, LiveCellOutput, OutPoint},
    prelude::*,
};
use ckb_util::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const COLUMNS: u32 = 4;

/// +---------------------------------+---------------+--------------------------+
/// |             Column              |      Key      |          Value           |
/// +---------------------------------+---------------+--------------------------+
/// | COLUMN_LOCK_HASH_INDEX_STATE    | Byte32        | LockHashIndexState       |
/// | COLUMN_LOCK_HASH_LIVE_CELL      | LockHashIndex | LiveCellOutput           |
/// | COLUMN_LOCK_HASH_TRANSACTION    | LockHashIndex | Option<TransactionPoint> |
/// | COLUMN_OUT_POINT_LOCK_HASH      | OutPoint      | LockHashCellOutput       |
/// +---------------------------------+---------------+--------------------------+

const COLUMN_LOCK_HASH_INDEX_STATE: Col = "0";
const COLUMN_LOCK_HASH_LIVE_CELL: Col = "1";
const COLUMN_LOCK_HASH_TRANSACTION: Col = "2";
const COLUMN_OUT_POINT_LOCK_HASH: Col = "3";

pub trait IndexerStore: Sync + Send {
    fn get_live_cells(
        &self,
        lock_hash: &Byte32,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<LiveCell>;

    fn get_transactions(
        &self,
        lock_hash: &Byte32,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<CellTransaction>;

    fn get_capacity(&self, lock_hash: &Byte32) -> Option<LockHashCapacity>;

    fn get_lock_hash_index_states(&self) -> HashMap<Byte32, LockHashIndexState>;

    fn insert_lock_hash(
        &self,
        lock_hash: &Byte32,
        index_from: Option<BlockNumber>,
    ) -> LockHashIndexState;

    fn remove_lock_hash(&self, lock_hash: &Byte32);
}

#[derive(Clone)]
pub struct DefaultIndexerStore {
    db: Arc<RocksDB>,
    shared: Shared,
    batch_interval: Duration,
    batch_size: usize,
    sync_lock: Arc<Mutex<()>>,
}

impl IndexerStore for DefaultIndexerStore {
    fn get_live_cells(
        &self,
        lock_hash: &Byte32,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<LiveCell> {
        let mut from_key = lock_hash.as_slice().to_owned();
        let iter = if reverse_order {
            from_key.extend_from_slice(&BlockNumber::max_value().to_be_bytes());
            self.db.iter(
                COLUMN_LOCK_HASH_LIVE_CELL,
                IteratorMode::From(&from_key, Direction::Reverse),
            )
        } else {
            self.db.iter(
                COLUMN_LOCK_HASH_LIVE_CELL,
                IteratorMode::From(&from_key, Direction::Forward),
            )
        };
        iter.expect("indexer db iter should be ok")
            .skip(skip_num)
            .take(take_num)
            .take_while(|(key, _)| key.starts_with(lock_hash.as_slice()))
            .map(|(key, value)| {
                let live_cell_output = LiveCellOutput::from_slice(&value)
                    .expect("verify LiveCellOutput in storage should be ok");
                let lock_hash_index = LockHashIndex::from_packed(
                    packed::LockHashIndexReader::from_slice(&key).unwrap(),
                );
                LiveCell {
                    created_by: lock_hash_index.into(),
                    cell_output: live_cell_output.cell_output(),
                    output_data_len: live_cell_output.output_data_len().unpack(),
                    cellbase: live_cell_output.cellbase().unpack(),
                }
            })
            .collect()
    }

    fn get_transactions(
        &self,
        lock_hash: &Byte32,
        skip_num: usize,
        take_num: usize,
        reverse_order: bool,
    ) -> Vec<CellTransaction> {
        let mut from_key = lock_hash.as_slice().to_owned();
        let iter = if reverse_order {
            from_key.extend_from_slice(&BlockNumber::max_value().to_be_bytes());
            self.db.iter(
                COLUMN_LOCK_HASH_TRANSACTION,
                IteratorMode::From(&from_key, Direction::Reverse),
            )
        } else {
            self.db.iter(
                COLUMN_LOCK_HASH_TRANSACTION,
                IteratorMode::From(&from_key, Direction::Forward),
            )
        };
        iter.expect("indexer db iter should be ok")
            .skip(skip_num)
            .take(take_num)
            .take_while(|(key, _)| key.starts_with(lock_hash.as_slice()))
            .map(|(key, value)| {
                let consumed_by = packed::TransactionPointOptReader::from_slice(&value)
                    .expect("verify TransactionPointOpt in storage should be ok")
                    .to_opt()
                    .map(TransactionPoint::from_packed);
                let lock_hash_index = LockHashIndex::from_packed(
                    packed::LockHashIndexReader::from_slice(&key).unwrap(),
                );
                CellTransaction {
                    created_by: lock_hash_index.into(),
                    consumed_by,
                }
            })
            .collect()
    }

    fn get_lock_hash_index_states(&self) -> HashMap<Byte32, LockHashIndexState> {
        self.db
            .iter(COLUMN_LOCK_HASH_INDEX_STATE, IteratorMode::Start)
            .expect("indexer db iter should be ok")
            .map(|(key, value)| {
                (
                    Byte32::from_slice(&key).expect("db safe access"),
                    LockHashIndexState::from_packed(
                        packed::LockHashIndexStateReader::from_slice(&value)
                            .expect("verify LockHashIndexState in storage should be ok"),
                    ),
                )
            })
            .collect()
    }

    fn get_capacity(&self, lock_hash: &Byte32) -> Option<LockHashCapacity> {
        let snapshot = self.db.get_snapshot();
        let from_key = lock_hash.as_slice();
        let iter = snapshot
            .iter(
                COLUMN_LOCK_HASH_LIVE_CELL,
                IteratorMode::From(from_key, Direction::Forward),
            )
            .expect("indexer db snapshot iter should be ok");
        snapshot
            .get_pinned(COLUMN_LOCK_HASH_INDEX_STATE, lock_hash.as_slice())
            .expect("indexer db snapshot get should be ok")
            .map(|value| {
                let index_state = LockHashIndexState::from_packed(
                    packed::LockHashIndexStateReader::from_slice(&value)
                        .expect("verify LockHashIndexState in storage should be ok"),
                );

                let (capacity, cells_count) =
                    iter.take_while(|(key, _)| key.starts_with(from_key)).fold(
                        (Capacity::zero(), 0),
                        |(capacity, cells_count), (_key, value)| {
                            let cell_output_capacity: Capacity = LiveCellOutput::from_slice(&value)
                                .expect("verify LiveCellOutput in storage should be ok")
                                .cell_output()
                                .capacity()
                                .unpack();
                            (
                                capacity
                                    .safe_add(cell_output_capacity)
                                    .expect("capacity should not overflow"),
                                cells_count + 1,
                            )
                        },
                    );

                LockHashCapacity {
                    capacity,
                    cells_count,
                    block_number: index_state.block_number,
                }
            })
    }

    fn insert_lock_hash(
        &self,
        lock_hash: &Byte32,
        index_from: Option<BlockNumber>,
    ) -> LockHashIndexState {
        let index_state = {
            let snapshot = self.shared.snapshot();
            let tip_number = snapshot.tip_header().number();
            let block_number = index_from.unwrap_or_else(|| tip_number).min(tip_number);
            LockHashIndexState {
                block_number,
                block_hash: snapshot.get_block_hash(block_number).expect("block exists"),
            }
        };
        let sync_lock = self.sync_lock.lock();
        self.commit_txn(|txn| {
            txn.insert_lock_hash_index_state(lock_hash, &index_state);
        });
        drop(sync_lock);
        index_state
    }

    fn remove_lock_hash(&self, lock_hash: &Byte32) {
        let sync_lock = self.sync_lock.lock();
        self.commit_txn(|txn| {
            let iter = self
                .db
                .iter(
                    COLUMN_LOCK_HASH_LIVE_CELL,
                    IteratorMode::From(lock_hash.as_slice(), Direction::Forward),
                )
                .expect("indexer db iter should be ok");

            iter.take_while(|(key, _)| key.starts_with(lock_hash.as_slice()))
                .for_each(|(key, _)| {
                    let lock_hash_index = LockHashIndex::from_packed(
                        packed::LockHashIndexReader::from_slice(&key).unwrap(),
                    );
                    txn.delete_lock_hash_live_cell(&lock_hash_index);
                    txn.delete_cell_out_point_lock_hash(&lock_hash_index.out_point);
                });

            let iter = self
                .db
                .iter(
                    COLUMN_LOCK_HASH_TRANSACTION,
                    IteratorMode::From(lock_hash.as_slice(), Direction::Forward),
                )
                .expect("indexer db iter should be ok");

            iter.take_while(|(key, _)| key.starts_with(lock_hash.as_slice()))
                .for_each(|(key, _)| {
                    let lock_hash_index = LockHashIndex::from_packed(
                        packed::LockHashIndexReader::from_slice(&key).unwrap(),
                    );
                    txn.delete_lock_hash_transaction(&lock_hash_index);
                });

            txn.delete_lock_hash_index_state(&lock_hash);
        });
        drop(sync_lock);
    }
}

const INIT_DB_VERSION: &str = "20191127135521";

impl DefaultIndexerStore {
    pub fn new(config: &IndexerConfig, shared: Shared) -> Self {
        let mut migrations = Migrations::default();
        migrations.add_migration(Box::new(DefaultMigration::new(INIT_DB_VERSION)));
        migrations.add_migration(Box::new(migrations::AddFieldsToLiveCell::new(
            shared.clone(),
        )));

        let db = migrations
            .migrate(RocksDB::open(&config.db, COLUMNS))
            .unwrap_or_else(|err| panic!("Indexer migrate failed {}", err));

        DefaultIndexerStore {
            db: Arc::new(db),
            shared,
            batch_interval: Duration::from_millis(config.batch_interval),
            batch_size: config.batch_size,
            sync_lock: Arc::new(Mutex::new(())),
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
                thread::sleep(self.batch_interval);
            })
            .expect("start DefaultIndexerStore failed");
    }

    // helper function
    fn commit_txn<F>(&self, process: F)
    where
        F: FnOnce(&IndexerStoreTransaction),
    {
        let db_txn = self.db.transaction();
        let mut txn = IndexerStoreTransaction { txn: db_txn };
        process(&mut txn);
        txn.commit();
    }

    pub fn sync_index_states(&self) {
        let sync_lock = self.sync_lock.lock();
        debug!("Start sync index states with chain store");
        let mut lock_hash_index_states = self.get_lock_hash_index_states();
        if lock_hash_index_states.is_empty() {
            return;
        }
        let snapshot = self.shared.snapshot();
        // retains the lock hashes on fork chain and detach blocks
        lock_hash_index_states.retain(|_, index_state| {
            snapshot.get_block_number(&index_state.block_hash.clone())
                != Some(index_state.block_number)
        });
        lock_hash_index_states
            .iter()
            .for_each(|(lock_hash, index_state)| {
                let mut index_lock_hashes = HashSet::new();
                index_lock_hashes.insert(lock_hash.to_owned());

                let mut block = snapshot
                    .get_block(&index_state.block_hash.clone())
                    .expect("block exists");
                // detach blocks until reach a block on main chain
                self.commit_txn(|txn| {
                    self.detach_block(txn, &index_lock_hashes, &block);
                    while snapshot.get_block_hash(block.header().number() - 1)
                        != Some(block.data().header().raw().parent_hash())
                    {
                        block = snapshot
                            .get_block(&block.data().header().raw().parent_hash())
                            .expect("block exists");
                        self.detach_block(txn, &index_lock_hashes, &block);
                    }
                    let index_state = LockHashIndexState {
                        block_number: block.header().number() - 1,
                        block_hash: block.header().parent_hash(),
                    };
                    txn.insert_lock_hash_index_state(lock_hash, &index_state);
                });
            });

        // attach blocks until reach tip or txn limit
        let mut lock_hash_index_states = self.get_lock_hash_index_states();

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

        let tip_number = snapshot.tip_header().number();
        self.commit_txn(|txn| {
            (start_number..=tip_number)
                .take(self.batch_size)
                .for_each(|block_number| {
                    let index_lock_hashes = lock_hash_index_states
                        .iter()
                        .filter(|(_, index_state)| index_state.block_number <= block_number)
                        .map(|(lock_hash, _)| lock_hash)
                        .cloned()
                        .collect();
                    let block = snapshot
                        .get_block_hash(block_number)
                        .as_ref()
                        .and_then(|hash| snapshot.get_block(hash))
                        .expect("block exists");
                    self.attach_block(txn, &index_lock_hashes, &block);
                    let index_state = LockHashIndexState {
                        block_number,
                        block_hash: block.hash(),
                    };
                    index_lock_hashes.into_iter().for_each(|lock_hash| {
                        lock_hash_index_states.insert(lock_hash, index_state.clone());
                    })
                });

            lock_hash_index_states
                .iter()
                .for_each(|(lock_hash, index_state)| {
                    txn.insert_lock_hash_index_state(lock_hash, index_state);
                })
        });

        drop(sync_lock);
        debug!("End sync index states with chain store");
    }

    fn detach_block(
        &self,
        txn: &IndexerStoreTransaction,
        index_lock_hashes: &HashSet<Byte32>,
        block: &core::BlockView,
    ) {
        trace!("detach block {}", block.header().hash());
        let snapshot = self.shared.snapshot();
        let block_number = block.header().number();
        block.transactions().iter().rev().for_each(|tx| {
            let tx_hash = tx.hash();
            tx.outputs()
                .into_iter()
                .enumerate()
                .for_each(|(index, output)| {
                    let index = index as u32;
                    let lock_hash = output.calc_lock_hash();
                    if index_lock_hashes.contains(&lock_hash) {
                        let lock_hash_index =
                            LockHashIndex::new(lock_hash, block_number, tx_hash.clone(), index);
                        txn.delete_lock_hash_live_cell(&lock_hash_index);
                        txn.delete_lock_hash_transaction(&lock_hash_index);
                        txn.delete_cell_out_point_lock_hash(&lock_hash_index.out_point);
                    }
                });

            if !tx.is_cellbase() {
                tx.inputs().into_iter().for_each(|input| {
                    let out_point = input.previous_output();
                    if let Some(lock_hash_cell_output) = txn.get_lock_hash_cell_output(&out_point) {
                        if index_lock_hashes.contains(&lock_hash_cell_output.lock_hash) {
                            if let Some(cell_output) = lock_hash_cell_output.cell_output {
                                if let Some((out_point_tx, _)) =
                                    snapshot.get_transaction(&out_point.tx_hash())
                                {
                                    let lock_hash_index = LockHashIndex::new(
                                        lock_hash_cell_output.lock_hash.clone(),
                                        lock_hash_cell_output.block_number,
                                        out_point.tx_hash(),
                                        out_point.index().unpack(),
                                    );

                                    let live_cell_output = LiveCellOutput::new_builder()
                                        .cell_output(cell_output)
                                        .output_data_len(
                                            (out_point_tx
                                                .outputs_data()
                                                .get(lock_hash_index.out_point.index().unpack())
                                                .expect("verified tx")
                                                .len()
                                                as u64)
                                                .pack(),
                                        )
                                        .cellbase(out_point_tx.is_cellbase().pack())
                                        .build();
                                    txn.generate_live_cell(lock_hash_index, live_cell_output);
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
        txn: &IndexerStoreTransaction,
        index_lock_hashes: &HashSet<Byte32>,
        block: &core::BlockView,
    ) {
        trace!("attach block {}", block.hash());
        let block_number = block.header().number();
        block.transactions().iter().for_each(|tx| {
            let tx_hash = tx.hash();
            if !tx.is_cellbase() {
                tx.inputs()
                    .into_iter()
                    .enumerate()
                    .for_each(|(index, input)| {
                        let index = index as u32;
                        let out_point = input.previous_output();
                        if let Some(lock_hash_cell_output) =
                            txn.get_lock_hash_cell_output(&out_point)
                        {
                            if index_lock_hashes.contains(&lock_hash_cell_output.lock_hash) {
                                let lock_hash_index = LockHashIndex::new(
                                    lock_hash_cell_output.lock_hash,
                                    lock_hash_cell_output.block_number,
                                    out_point.tx_hash(),
                                    out_point.index().unpack(),
                                );
                                let consumed_by = TransactionPoint {
                                    block_number,
                                    tx_hash: tx_hash.clone(),
                                    index,
                                };
                                txn.consume_live_cell(lock_hash_index, consumed_by);
                            }
                        }
                    });
            }

            tx.outputs()
                .into_iter()
                .enumerate()
                .for_each(|(index, output)| {
                    let lock_hash = output.calc_lock_hash();
                    if index_lock_hashes.contains(&lock_hash) {
                        let lock_hash_index = LockHashIndex::new(
                            lock_hash,
                            block_number,
                            tx_hash.clone(),
                            index as u32,
                        );
                        let live_cell_output = LiveCellOutput::new_builder()
                            .cell_output(output)
                            .output_data_len(
                                (tx.outputs_data().get(index).expect("verified tx").len() as u64)
                                    .pack(),
                            )
                            .cellbase(tx.is_cellbase().pack())
                            .build();
                        txn.generate_live_cell(lock_hash_index, live_cell_output);
                    }
                });
        })
    }
}

struct IndexerStoreTransaction {
    pub txn: RocksDBTransaction,
}

impl IndexerStoreTransaction {
    fn generate_live_cell(&self, lock_hash_index: LockHashIndex, live_cell_output: LiveCellOutput) {
        self.insert_lock_hash_live_cell(&lock_hash_index, &live_cell_output);
        self.insert_lock_hash_transaction(&lock_hash_index, &None);

        let lock_hash_cell_output = LockHashCellOutput {
            lock_hash: lock_hash_index.lock_hash.clone(),
            block_number: lock_hash_index.block_number,
            cell_output: Some(live_cell_output.cell_output()),
        };
        self.insert_cell_out_point_lock_hash(&lock_hash_index.out_point, &lock_hash_cell_output);
    }

    fn consume_live_cell(&self, lock_hash_index: LockHashIndex, consumed_by: TransactionPoint) {
        if let Some(lock_hash_cell_output) = self
            .txn
            .get(
                COLUMN_LOCK_HASH_LIVE_CELL,
                lock_hash_index.pack().as_slice(),
            )
            .expect("indexer db read should be ok")
            .map(|value| {
                LiveCellOutput::from_slice(&value)
                    .expect("verify CellOutput in storage should be ok")
            })
            .map(|live_cell_output: LiveCellOutput| LockHashCellOutput {
                lock_hash: lock_hash_index.lock_hash.clone(),
                block_number: lock_hash_index.block_number,
                cell_output: Some(live_cell_output.cell_output()),
            })
        {
            self.delete_lock_hash_live_cell(&lock_hash_index);
            self.insert_lock_hash_transaction(&lock_hash_index, &Some(consumed_by));
            self.insert_cell_out_point_lock_hash(
                &lock_hash_index.out_point,
                &lock_hash_cell_output,
            );
        }
    }

    fn insert_lock_hash_index_state(&self, lock_hash: &Byte32, index_state: &LockHashIndexState) {
        let value = index_state.pack();
        self.txn
            .put(
                COLUMN_LOCK_HASH_INDEX_STATE,
                lock_hash.as_slice(),
                value.as_slice(),
            )
            .expect("txn insert COLUMN_LOCK_HASH_INDEX_STATE failed");
    }

    fn insert_lock_hash_live_cell(
        &self,
        lock_hash_index: &LockHashIndex,
        live_cell_output: &LiveCellOutput,
    ) {
        self.txn
            .put(
                COLUMN_LOCK_HASH_LIVE_CELL,
                lock_hash_index.pack().as_slice(),
                live_cell_output.as_slice(),
            )
            .expect("txn insert COLUMN_LOCK_HASH_LIVE_CELL failed");
    }

    fn insert_lock_hash_transaction(
        &self,
        lock_hash_index: &LockHashIndex,
        consumed_by: &Option<TransactionPoint>,
    ) {
        let value = {
            packed::TransactionPointOpt::new_builder()
                .set(consumed_by.as_ref().map(|i| i.pack()))
                .build()
        };
        self.txn
            .put(
                COLUMN_LOCK_HASH_TRANSACTION,
                lock_hash_index.pack().as_slice(),
                value.as_slice(),
            )
            .expect("txn insert COLUMN_LOCK_HASH_TRANSACTION failed");
    }

    fn insert_cell_out_point_lock_hash(
        &self,
        out_point: &OutPoint,
        lock_hash_cell_output: &LockHashCellOutput,
    ) {
        self.txn
            .put(
                COLUMN_OUT_POINT_LOCK_HASH,
                out_point.as_slice(),
                lock_hash_cell_output.pack().as_slice(),
            )
            .expect("txn insert COLUMN_OUT_POINT_LOCK_HASH failed");
    }

    fn delete_lock_hash_index_state(&self, lock_hash: &Byte32) {
        self.txn
            .delete(COLUMN_LOCK_HASH_INDEX_STATE, lock_hash.as_slice())
            .expect("txn delete COLUMN_LOCK_HASH_INDEX_STATE failed");
    }

    fn delete_lock_hash_live_cell(&self, lock_hash_index: &LockHashIndex) {
        self.txn
            .delete(
                COLUMN_LOCK_HASH_LIVE_CELL,
                lock_hash_index.pack().as_slice(),
            )
            .expect("txn delete COLUMN_LOCK_HASH_LIVE_CELL failed");
    }

    fn delete_lock_hash_transaction(&self, lock_hash_index: &LockHashIndex) {
        self.txn
            .delete(
                COLUMN_LOCK_HASH_TRANSACTION,
                lock_hash_index.pack().as_slice(),
            )
            .expect("txn delete COLUMN_LOCK_HASH_TRANSACTION failed");
    }

    fn delete_cell_out_point_lock_hash(&self, out_point: &OutPoint) {
        self.txn
            .delete(COLUMN_OUT_POINT_LOCK_HASH, out_point.as_slice())
            .expect("txn delete COLUMN_OUT_POINT_LOCK_HASH failed");
    }

    fn get_lock_hash_cell_output(&self, out_point: &OutPoint) -> Option<LockHashCellOutput> {
        self.txn
            .get(COLUMN_OUT_POINT_LOCK_HASH, out_point.as_slice())
            .expect("indexer db read should be ok")
            .map(|value| {
                LockHashCellOutput::from_packed(
                    packed::LockHashCellOutputReader::from_slice(&value)
                        .expect("verify LockHashCellOutput in storage should be ok"),
                )
            })
    }

    fn commit(self) {
        // only log the error, indexer store commit failure should not causing the thread to panic entirely.
        if let Err(err) = self.txn.commit() {
            error!("indexer db failed to commit txn, error: {:?}", err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_chain::{
        chain::{ChainController, ChainService},
        switch::Switch,
    };
    use ckb_chain_spec::consensus::Consensus;
    use ckb_resource::CODE_HASH_DAO;
    use ckb_shared::shared::{Shared, SharedBuilder};
    use ckb_types::{
        bytes::Bytes,
        core::{
            capacity_bytes, BlockBuilder, Capacity, HeaderBuilder, ScriptHashType,
            TransactionBuilder,
        },
        packed::{Byte32, CellInput, CellOutputBuilder, OutPoint, ScriptBuilder},
        utilities::{difficulty_to_compact, DIFF_TWO},
        U256,
    };
    use std::sync::Arc;

    fn setup(prefix: &str) -> (DefaultIndexerStore, ChainController, Shared) {
        let builder = SharedBuilder::default();
        let (shared, table) = builder.consensus(Consensus::default()).build().unwrap();

        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let mut config = IndexerConfig::default();
        config.db.path = tmp_dir.as_ref().to_path_buf();
        let chain_service = ChainService::new(shared.clone(), table);
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
        store.insert_lock_hash(&CODE_HASH_DAO.pack(), None);
        store.insert_lock_hash(&Byte32::zero(), None);

        assert_eq!(2, store.get_lock_hash_index_states().len());

        store.remove_lock_hash(&CODE_HASH_DAO.pack());
        assert_eq!(1, store.get_lock_hash_index_states().len());
    }

    #[test]
    fn get_live_cells() {
        let (store, chain, shared) = setup("get_live_cells");
        let script1 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let script2 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"script2".to_vec()).pack())
            .build();
        store.insert_lock_hash(&script1.calc_script_hash(), None);
        store.insert_lock_hash(&script2.calc_script_hash(), None);

        let tx11 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx12 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(DIFF_TWO.pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        let tx21_output_data = Bytes::from(vec![1, 2, 3]);
        let tx21 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(3000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(tx21_output_data.pack())
            .build();

        let tx22 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(4000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21)
            .transaction(tx22)
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(4u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.header().hash())
                    .build(),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx11.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(5000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx12.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(6000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block2_fork = BlockBuilder::default()
            .transaction(tx31)
            .transaction(tx32)
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(20u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.header().hash())
                    .build(),
            )
            .build();

        chain
            .internal_process_block(Arc::new(block1), Switch::DISABLE_ALL)
            .unwrap();
        chain
            .internal_process_block(Arc::new(block2), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();

        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, cells.len());
        assert_eq!(
            capacity_bytes!(1000),
            cells[0].cell_output.capacity().unpack()
        );
        assert_eq!(
            capacity_bytes!(3000),
            cells[1].cell_output.capacity().unpack()
        );
        assert_eq!(tx21_output_data.len() as u64, cells[1].output_data_len);
        assert_eq!(false, cells[1].cellbase,);
        // test reverse order
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, true);
        assert_eq!(2, cells.len());
        assert_eq!(
            capacity_bytes!(3000),
            cells[0].cell_output.capacity().unpack()
        );
        assert_eq!(
            capacity_bytes!(1000),
            cells[1].cell_output.capacity().unpack()
        );
        // test get_capacity
        let lock_hash_capacity = store.get_capacity(&script1.calc_script_hash()).unwrap();
        assert_eq!(capacity_bytes!(4000), lock_hash_capacity.capacity);
        assert_eq!(2, lock_hash_capacity.cells_count);
        assert_eq!(2, lock_hash_capacity.block_number);

        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(2, cells.len());
        assert_eq!(
            capacity_bytes!(2000),
            cells[0].cell_output.capacity().unpack()
        );
        assert_eq!(
            capacity_bytes!(4000),
            cells[1].cell_output.capacity().unpack()
        );

        chain
            .internal_process_block(Arc::new(block2_fork), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(
            capacity_bytes!(5000),
            cells[0].cell_output.capacity().unpack()
        );

        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(
            capacity_bytes!(6000),
            cells[0].cell_output.capacity().unpack()
        );

        // remove script1's lock hash should remove its indexed data also
        store.remove_lock_hash(&script1.calc_script_hash());
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert!(store.get_capacity(&script1.calc_script_hash()).is_none());
    }

    #[test]
    fn get_transactions() {
        let (store, chain, shared) = setup("get_transactions");
        let script1 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let script2 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"script2".to_vec()).pack())
            .build();
        store.insert_lock_hash(&script1.calc_script_hash(), None);
        store.insert_lock_hash(&script2.calc_script_hash(), None);

        let tx11 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx12 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(DIFF_TWO.pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        let tx21 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(3000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx22 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(4000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21.clone())
            .transaction(tx22.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(4u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.header().hash())
                    .build(),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx11.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(5000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx12.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(6000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block2_fork = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(20u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.header().hash())
                    .build(),
            )
            .build();

        chain
            .internal_process_block(Arc::new(block1), Switch::DISABLE_ALL)
            .unwrap();
        chain
            .internal_process_block(Arc::new(block2), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();

        let transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx21.hash(), transactions[1].created_by.tx_hash);

        // test reverse order
        let transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, true);
        assert_eq!(2, transactions.len());
        assert_eq!(tx21.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx11.hash(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx22.hash(), transactions[1].created_by.tx_hash);

        chain
            .internal_process_block(Arc::new(block2_fork), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(
            Some(tx31.hash()),
            transactions[0]
                .consumed_by
                .as_ref()
                .map(|transaction_point| transaction_point.tx_hash.clone())
        );
        assert_eq!(tx31.hash(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx32.hash(), transactions[1].created_by.tx_hash);

        // remove script1's lock hash should remove its indexed data also
        store.remove_lock_hash(&script1.calc_script_hash());
        let transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, transactions.len());
        let transactions = store.get_transactions(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
    }

    #[test]
    fn sync_index_states() {
        let (store, chain, shared) = setup("sync_index_states");
        let script1 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let script2 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"script2".to_vec()).pack())
            .build();
        store.insert_lock_hash(&script1.calc_script_hash(), None);
        store.insert_lock_hash(&script2.calc_script_hash(), None);

        let tx11 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx12_output_data = Bytes::from(vec![1, 2, 3]);
        let tx12 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(2000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(tx12_output_data.pack())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11.clone())
            .transaction(tx12.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(DIFF_TWO.pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        let tx21 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(3000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx22 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(4000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21.clone())
            .transaction(tx22.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(4u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.header().hash())
                    .build(),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx11.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(5000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx32_output_data = Bytes::from(vec![1, 2, 3, 4]);
        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx12.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(6000).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(tx32_output_data.pack())
            .build();

        let block2_fork = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(20u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.header().hash())
                    .build(),
            )
            .build();

        let block3 = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(22u64)).pack())
                    .number(3.pack())
                    .parent_hash(block2_fork.header().hash())
                    .build(),
            )
            .build();

        chain
            .internal_process_block(Arc::new(block1), Switch::DISABLE_ALL)
            .unwrap();
        chain
            .internal_process_block(Arc::new(block2), Switch::DISABLE_ALL)
            .unwrap();

        store.sync_index_states();

        let transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx21.hash(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx22.hash(), transactions[1].created_by.tx_hash);

        chain
            .internal_process_block(Arc::new(block2_fork), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(tx12_output_data.len() as u64, cells[0].output_data_len);
        let transactions = store.get_transactions(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, transactions.len());
        assert_eq!(tx12.hash(), transactions[0].created_by.tx_hash);

        chain
            .internal_process_block(Arc::new(block3), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(tx32_output_data.len() as u64, cells[0].output_data_len);

        let transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(
            Some(tx31.hash()),
            transactions[0]
                .consumed_by
                .as_ref()
                .map(|transaction_point| transaction_point.tx_hash.clone())
        );
        assert_eq!(tx31.hash(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash(), transactions[0].created_by.tx_hash);
        assert_eq!(tx32.hash(), transactions[1].created_by.tx_hash);
    }

    #[test]
    fn consume_txs_in_same_block() {
        let (store, chain, shared) = setup("consume_txs_in_same_block");
        let script1 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let script2 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"script2".to_vec()).pack())
            .build();
        store.insert_lock_hash(&script1.calc_script_hash(), None);
        store.insert_lock_hash(&script2.calc_script_hash(), None);
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());

        let tx11 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx12 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx11.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(900).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx13_output_data = Bytes::from(vec![1, 2, 3]);
        let tx13 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx12.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(800).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(tx13_output_data.pack())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11)
            .transaction(tx12)
            .transaction(tx13)
            .header(
                HeaderBuilder::default()
                    .compact_target(DIFF_TWO.pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        let block1_fork = BlockBuilder::default()
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(20u64)).pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        chain
            .internal_process_block(Arc::new(block1), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, cell_transactions.len());
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(tx13_output_data.len() as u64, cells[0].output_data_len);
        assert_eq!(false, cells[0].cellbase,);

        chain
            .internal_process_block(Arc::new(block1_fork), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cell_transactions.len());
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());
    }

    #[test]
    fn detach_blocks() {
        let (store, chain, shared) = setup("detach_blocks");
        let script1 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let script2 = ScriptBuilder::default()
            .code_hash(CODE_HASH_DAO.pack())
            .hash_type(ScriptHashType::Data.into())
            .args(Bytes::from(b"script2".to_vec()).pack())
            .build();
        store.insert_lock_hash(&script1.calc_script_hash(), None);
        store.insert_lock_hash(&script2.calc_script_hash(), None);
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());

        let tx11 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx12 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx11.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(900).pack())
                    .lock(script1.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let tx21_output_data = Bytes::from(vec![1, 2, 3]);
        let tx21 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(tx12.hash(), 0), 0))
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(800).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(tx21_output_data.pack())
            .build();

        let block1 = BlockBuilder::default()
            .transaction(tx11)
            .transaction(tx12)
            .header(
                HeaderBuilder::default()
                    .compact_target(DIFF_TWO.pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        let block2 = BlockBuilder::default()
            .transaction(tx21)
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(4u64)).pack())
                    .number(2.pack())
                    .parent_hash(block1.hash())
                    .build(),
            )
            .build();

        let tx31 = TransactionBuilder::default()
            .output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1100).pack())
                    .lock(script2.clone())
                    .build(),
            )
            .output_data(Default::default())
            .build();

        let block1_fork = BlockBuilder::default()
            .transaction(tx31)
            .header(
                HeaderBuilder::default()
                    .compact_target(difficulty_to_compact(U256::from(20u64)).pack())
                    .number(1.pack())
                    .parent_hash(shared.genesis_hash())
                    .build(),
            )
            .build();

        chain
            .internal_process_block(Arc::new(block1), Switch::DISABLE_ALL)
            .unwrap();
        chain
            .internal_process_block(Arc::new(block2), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(2, cell_transactions.len());
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(
            capacity_bytes!(800),
            cells[0].cell_output.capacity().unpack()
        );
        assert_eq!(tx21_output_data.len() as u64, cells[0].output_data_len);
        assert_eq! {
            false,
            cells[0].cellbase
        };

        chain
            .internal_process_block(Arc::new(block1_fork), Switch::DISABLE_ALL)
            .unwrap();
        store.sync_index_states();
        let cells = store.get_live_cells(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cells.len());
        let cell_transactions = store.get_transactions(&script1.calc_script_hash(), 0, 100, false);
        assert_eq!(0, cell_transactions.len());
        let cells = store.get_live_cells(&script2.calc_script_hash(), 0, 100, false);
        assert_eq!(1, cells.len());
        assert_eq!(
            capacity_bytes!(1100),
            cells[0].cell_output.capacity().unpack()
        );
        assert_eq!(0, cells[0].output_data_len);
    }
}
