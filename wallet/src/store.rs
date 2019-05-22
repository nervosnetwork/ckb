use crate::types::{
    CellTransaction, LiveCell, LockHashCellOutput, LockHashIndex, LockHashIndexState,
    TransactionPoint,
};
use bincode::{deserialize, serialize};
use ckb_core::block::Block;
use ckb_core::transaction::{CellOutPoint, CellOutput};
use ckb_db::{
    rocksdb::{RocksDB, RocksdbBatch},
    Col, DBConfig, DbBatch, IterableKeyValueDB, KeyValueDB,
};
use ckb_notify::NotifyController;
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use crossbeam_channel::{self, select};
use log::error;
use numext_fixed_hash::H256;
use std::collections::HashMap;
use std::thread;

const WALLET_STORE_SUBSCRIBER: &str = "wallet_store";

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

pub trait WalletStore: Sync + Send {
    fn get_live_cells(&self, lock_hash: &H256, skip_num: usize, take_num: usize) -> Vec<LiveCell>;

    fn get_transactions(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
    ) -> Vec<CellTransaction>;

    fn get_lock_hash_index_states(&self) -> HashMap<H256, LockHashIndexState>;
}

pub struct DefaultWalletStore<CS> {
    db: RocksDB,
    shared: Shared<CS>,
}

impl<CS: ChainStore + 'static> WalletStore for DefaultWalletStore<CS> {
    fn get_live_cells(&self, lock_hash: &H256, skip_num: usize, take_num: usize) -> Vec<LiveCell> {
        let iter = self
            .db
            .iter(COLUMN_LOCK_HASH_LIVE_CELL, lock_hash.as_bytes())
            .expect("wallet db iter should be ok");
        iter.skip(skip_num)
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
    ) -> Vec<CellTransaction> {
        let iter = self
            .db
            .iter(COLUMN_LOCK_HASH_TRANSACTION, lock_hash.as_bytes())
            .expect("wallet db iter should be ok");
        iter.skip(skip_num)
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
            .iter(COLUMN_LOCK_HASH_INDEX_STATE, &Vec::new())
            .expect("wallet db iter should be ok")
            .map(|(key, value)| {
                (
                    H256::from_slice(&key).expect("db safe access"),
                    deserialize(&value).expect("deserialize LockHashIndexState should be ok"),
                )
            })
            .collect()
    }
}

impl<CS: ChainStore + 'static> DefaultWalletStore<CS> {
    pub fn new(db: RocksDB, shared: Shared<CS>) -> Self {
        DefaultWalletStore { db, shared }
    }

    pub fn start<S: ToString>(self, thread_name: Option<S>, notify: &NotifyController) {
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let new_tip_receiver = notify.subscribe_new_tip(WALLET_STORE_SUBSCRIBER);
        thread_builder
            .spawn(move || loop {
                select! {
                    recv(new_tip_receiver) -> msg => match msg {
                        Ok(tip_changes) => self.update(&tip_changes.detached_blocks, &tip_changes.attached_blocks),
                        _ => {
                            error!(target: "wallet", "new_tip_receiver closed");
                            break;
                        }
                    },
                }
            })
            .expect("start DefaultWalletStore failed");
    }

    // helper function
    fn commit_batch<F>(&self, process: F)
    where
        F: FnOnce(&mut WalletStoreBatch),
    {
        match self.db.batch() {
            Ok(batch) => {
                let mut batch = WalletStoreBatch { batch };
                process(&mut batch);
                batch.commit();
            }
            Err(err) => {
                error!(target: "wallet", "wallet db failed to create new batch, error: {:?}", err);
            }
        }
    }

    pub fn insert_lock_hash(&self, lock_hash: &H256) {
        let index_state = LockHashIndexState {
            block_number: 0,
            block_hash: self.shared.genesis_hash().to_owned(),
        };
        self.commit_batch(|batch| {
            batch.insert_lock_hash_index_state(&lock_hash, &index_state);
        })
    }

    pub fn remove_lock_hash(&self, lock_hash: &H256) {
        self.commit_batch(|batch| {
            let iter = self
                .db
                .iter(COLUMN_LOCK_HASH_LIVE_CELL, lock_hash.as_bytes())
                .expect("wallet db iter should be ok");

            iter.take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
                .for_each(|(key, value)| {
                    let lock_hash_index = LockHashIndex::from_slice(&key);
                    batch.delete_lock_hash_live_cell(&lock_hash_index);
                    batch.delete_cell_out_point_lock_hash(&lock_hash_index.cell_out_point);
                });

            let iter = self
                .db
                .iter(COLUMN_LOCK_HASH_TRANSACTION, lock_hash.as_bytes())
                .expect("wallet db iter should be ok");

            iter.take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
                .for_each(|(key, value)| {
                    let lock_hash_index = LockHashIndex::from_slice(&key);
                    batch.delete_lock_hash_transaction(&lock_hash_index);
                });

            batch.delete_lock_hash_index_state(&lock_hash);
        });
    }

    pub(crate) fn update(&self, detached_blocks: &[Block], attached_blocks: &[Block]) {
        let lock_hash_index_states = self.get_lock_hash_index_states();
        if !lock_hash_index_states.is_empty() {
            self.commit_batch(|batch| {
                detached_blocks
                    .iter()
                    .for_each(|block| self.detach_block(batch, &lock_hash_index_states, block));
                // rocksdb rust binding doesn't support transactional batch read, have to use a batch buffer here.
                let mut batch_buffer = HashMap::<CellOutPoint, LockHashCellOutput>::new();
                attached_blocks.iter().for_each(|block| {
                    self.attach_block(batch, &mut batch_buffer, &lock_hash_index_states, block)
                });
                if let Some(block) = attached_blocks.last() {
                    let index_state = LockHashIndexState {
                        block_number: block.header().number(),
                        block_hash: block.header().hash().to_owned(),
                    };
                    lock_hash_index_states.keys().for_each(|lock_hash| {
                        batch.insert_lock_hash_index_state(lock_hash, &index_state);
                    })
                }
            });
        }
    }

    fn detach_block(
        &self,
        batch: &mut WalletStoreBatch,
        index_states: &HashMap<H256, LockHashIndexState>,
        block: &Block,
    ) {
        let block_number = block.header().number();
        block.transactions().iter().for_each(|tx| {
            let tx_hash = tx.hash();
            if !tx.is_cellbase() {
                tx.inputs().iter().enumerate().for_each(|(index, input)| {
                    let index = index as u32;
                    let cell_out_point = input.previous_output.cell.clone().expect("cell exists");
                    if let Some(mut lock_hash_cell_output) =
                        self.get_lock_hash_cell_output(&cell_out_point)
                    {
                        if index_states.contains_key(&lock_hash_cell_output.lock_hash) {
                            let lock_hash_index = LockHashIndex::new(
                                lock_hash_cell_output.lock_hash.clone(),
                                block_number,
                                tx_hash.clone(),
                                index,
                            );
                            batch.insert_lock_hash_live_cell(
                                &lock_hash_index,
                                &lock_hash_cell_output
                                    .cell_output
                                    .expect("inconsistent state"),
                            );
                            batch.insert_lock_hash_transaction(&lock_hash_index, &None);

                            lock_hash_cell_output.cell_output = None;
                            batch.insert_cell_out_point_lock_hash(
                                &cell_out_point,
                                &lock_hash_cell_output,
                            );
                        }
                    }
                });
            }

            tx.outputs().iter().enumerate().for_each(|(index, output)| {
                let index = index as u32;
                let lock_hash = output.lock.hash();
                if index_states.contains_key(&lock_hash) {
                    let lock_hash_index =
                        LockHashIndex::new(lock_hash, block_number, tx_hash.clone(), index);

                    batch.delete_lock_hash_live_cell(&lock_hash_index);
                    batch.delete_lock_hash_transaction(&lock_hash_index);
                    batch.delete_cell_out_point_lock_hash(&lock_hash_index.cell_out_point);
                }
            });
        })
    }

    fn attach_block(
        &self,
        batch: &mut WalletStoreBatch,
        batch_buffer: &mut HashMap<CellOutPoint, LockHashCellOutput>,
        index_states: &HashMap<H256, LockHashIndexState>,
        block: &Block,
    ) {
        let block_number = block.header().number();
        block.transactions().iter().for_each(|tx| {
            let tx_hash = tx.hash();
            tx.outputs().iter().enumerate().for_each(|(index, output)| {
                let index = index as u32;
                let lock_hash = output.lock.hash();
                if index_states.contains_key(&lock_hash) {
                    let lock_hash_index =
                        LockHashIndex::new(lock_hash.clone(), block_number, tx_hash.clone(), index);
                    batch.insert_lock_hash_live_cell(&lock_hash_index, output);
                    batch.insert_lock_hash_transaction(&lock_hash_index, &None);

                    let mut lock_hash_cell_output = LockHashCellOutput {
                        lock_hash,
                        block_number,
                        cell_output: None,
                    };
                    let cell_out_point = CellOutPoint {
                        tx_hash: tx_hash.clone(),
                        index,
                    };
                    batch.insert_cell_out_point_lock_hash(&cell_out_point, &lock_hash_cell_output);

                    // insert lock_hash_cell_output as a cached value
                    lock_hash_cell_output.cell_output = Some(output.clone());
                    batch_buffer.insert(cell_out_point, lock_hash_cell_output);
                }
            });

            if !tx.is_cellbase() {
                tx.inputs().iter().enumerate().for_each(|(index, input)| {
                    // lookup lock_hash in the batch buffer and store
                    let index = index as u32;
                    let cell_out_point = input.previous_output.cell.clone().expect("cell exists");
                    if let Some(lock_hash_cell_output) = batch_buffer
                        .get(&cell_out_point)
                        .cloned()
                        .or_else(|| self.get_lock_hash_cell_output(&cell_out_point))
                    {
                        if index_states.contains_key(&lock_hash_cell_output.lock_hash) {
                            batch.insert_cell_out_point_lock_hash(
                                &cell_out_point,
                                &lock_hash_cell_output,
                            );
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
                            batch.delete_lock_hash_live_cell(&lock_hash_index);
                            batch
                                .insert_lock_hash_transaction(&lock_hash_index, &Some(consumed_by));
                        }
                    }
                });
            }
        })
    }

    fn get_lock_hash_cell_output(
        &self,
        cell_out_point: &CellOutPoint,
    ) -> Option<LockHashCellOutput> {
        self.db
            .read(
                COLUMN_CELL_OUT_POINT_LOCK_HASH,
                &serialize(cell_out_point).expect("serialize OutPoint should be ok"),
            )
            .expect("wallet db read should be ok")
            .map(|value| deserialize(&value).expect("deserialize LockHashCellOutput should be ok"))
    }
}

struct WalletStoreBatch {
    pub batch: RocksdbBatch,
}

impl WalletStoreBatch {
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
    }

    fn commit(self) {
        // only log the error, wallet store commit failure should not causing the thread to panic entirely.
        if let Err(err) = self.batch.commit() {
            error!(target: "wallet", "wallet db failed to commit batch, error: {:?}", err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::{Script, DAO_CODE_HASH};
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, BlockNumber, Bytes, Capacity};
    use ckb_db::MemoryKeyValueDB;
    use ckb_shared::shared::SharedBuilder;
    use ckb_store::ChainKVStore;
    use tempfile;

    fn setup_store(prefix: &str) -> DefaultWalletStore<ChainKVStore<MemoryKeyValueDB>> {
        let builder = SharedBuilder::<MemoryKeyValueDB>::new();
        let shared = builder.consensus(Consensus::default()).build().unwrap();

        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let db = RocksDB::open(&config, COLUMNS);
        DefaultWalletStore::new(db, shared)
    }

    #[test]
    fn lock_hash_index() {
        let store = setup_store("lock_hash_index");
        store.insert_lock_hash(&DAO_CODE_HASH);
        store.insert_lock_hash(&H256::zero());

        assert_eq!(2, store.get_lock_hash_index_states().len());

        store.remove_lock_hash(&DAO_CODE_HASH);
        assert_eq!(1, store.get_lock_hash_index_states().len());
    }

    #[test]
    fn get_live_cells() {
        let store = setup_store("get_live_cells");
        let script1 = Script::new(Vec::new(), DAO_CODE_HASH);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash());
        store.insert_lock_hash(&script2.hash());

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
            .header_builder(HeaderBuilder::default().number(1))
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
            .header_builder(HeaderBuilder::default().number(2))
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
                vec![],
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
                vec![],
            ))
            .output(CellOutput::new(
                capacity_bytes!(6000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block3 = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header_builder(HeaderBuilder::default().number(3))
            .build();

        store.update(&[], &[block1, block2.clone()]);
        let cells = store.get_live_cells(&script1.hash(), 0, 100);
        assert_eq!(2, cells.len());
        assert_eq!(capacity_bytes!(1000), cells[0].cell_output.capacity);
        assert_eq!(capacity_bytes!(3000), cells[1].cell_output.capacity);

        let cells = store.get_live_cells(&script2.hash(), 0, 100);
        assert_eq!(2, cells.len());
        assert_eq!(capacity_bytes!(2000), cells[0].cell_output.capacity);
        assert_eq!(capacity_bytes!(4000), cells[1].cell_output.capacity);

        store.update(&[block2], &[block3]);
        let cells = store.get_live_cells(&script1.hash(), 0, 100);
        assert_eq!(1, cells.len());
        assert_eq!(capacity_bytes!(5000), cells[0].cell_output.capacity);

        let cells = store.get_live_cells(&script2.hash(), 0, 100);
        assert_eq!(1, cells.len());
        assert_eq!(capacity_bytes!(6000), cells[0].cell_output.capacity);

        // remove script1's lock hash should remove its indexed data also
        store.remove_lock_hash(&script1.hash());
        let cells = store.get_live_cells(&script1.hash(), 0, 100);
        assert_eq!(0, cells.len());
        let cells = store.get_live_cells(&script2.hash(), 0, 100);
        assert_eq!(1, cells.len());
    }

    #[test]
    fn get_transactions() {
        let store = setup_store("get_transactions");
        let script1 = Script::new(Vec::new(), DAO_CODE_HASH);
        let script2 = Script::default();
        store.insert_lock_hash(&script1.hash());
        store.insert_lock_hash(&script2.hash());

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
            .header_builder(HeaderBuilder::default().number(1))
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
            .header_builder(HeaderBuilder::default().number(2))
            .build();

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(
                OutPoint::new_cell(tx11.hash().to_owned(), 0),
                0,
                vec![],
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
                vec![],
            ))
            .output(CellOutput::new(
                capacity_bytes!(6000),
                Bytes::new(),
                script2.clone(),
                None,
            ))
            .build();

        let block3 = BlockBuilder::default()
            .transaction(tx31.clone())
            .transaction(tx32.clone())
            .header_builder(HeaderBuilder::default().number(3))
            .build();

        store.update(&[], &[block1, block2.clone()]);
        let transactions = store.get_transactions(&script1.hash(), 0, 100);
        assert_eq!(2, transactions.len());
        assert_eq!(tx11.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx21.hash().to_owned(), transactions[1].created_by.tx_hash);

        let transactions = store.get_transactions(&script2.hash(), 0, 100);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx22.hash().to_owned(), transactions[1].created_by.tx_hash);

        store.update(&[block2], &[block3]);
        let transactions = store.get_transactions(&script1.hash(), 0, 100);
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

        let transactions = store.get_transactions(&script2.hash(), 0, 100);
        assert_eq!(2, transactions.len());
        assert_eq!(tx12.hash().to_owned(), transactions[0].created_by.tx_hash);
        assert_eq!(tx32.hash().to_owned(), transactions[1].created_by.tx_hash);

        // remove script1's lock hash should remove its indexed data also
        store.remove_lock_hash(&script1.hash());
        let transactions = store.get_transactions(&script1.hash(), 0, 100);
        assert_eq!(0, transactions.len());
        let transactions = store.get_transactions(&script2.hash(), 0, 100);
        assert_eq!(2, transactions.len());
    }
}
