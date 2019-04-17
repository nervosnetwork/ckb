use bincode::{deserialize, serialize};
use ckb_core::block::Block;
use ckb_core::header::BlockNumber;
use ckb_core::transaction::{CellOutput, OutPoint as CoreOutPoint};
use ckb_db::{Col, DbBatch, IterableKeyValueDB};
use ckb_db::{DBConfig, RocksDB};
use ckb_notify::NotifyController;
use crossbeam_channel::{self, select};
use jsonrpc_core::Result;
use jsonrpc_derive::rpc;
use jsonrpc_types::{CellOutputWithOutPoint, OutPoint, Transaction};
use log::error;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::TryInto;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

#[rpc]
pub trait WalletRpc {
    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_live_cells","params": ["0xcb7bce98a778f130d34da522623d7e56705bddfe0dc4781bd2331211134a19a5", 0, 50]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_live_cells")]
    fn get_live_cells(
        &self,
        _lock_hash: H256,
        _page: usize,
        _per_page: usize,
    ) -> Result<Vec<CellOutputWithOutPoint>>;

    // curl -d '{"id": 2, "jsonrpc": "2.0", "method":"get_transactions","params": ["0xcb7bce98a778f130d34da522623d7e56705bddfe0dc4781bd2331211134a19a5"]}' -H 'content-type:application/json' 'http://localhost:8114'
    #[rpc(name = "get_transactions")]
    fn get_transactions(&self, _lock_hash: H256) -> Result<Vec<Transaction>>;
}

pub(crate) struct WalletRpcImpl<WS> {
    pub store: WS,
}

pub(crate) fn new_default_wallet_rpc(
    path: PathBuf,
    notify: &NotifyController,
) -> WalletRpcImpl<DefaultWalletStore<RocksDB>> {
    let config = DBConfig {
        path,
        ..Default::default()
    };
    let db = Arc::new(RocksDB::open(&config, COLUMNS));
    DefaultWalletStore::new(Arc::clone(&db)).start(Some("wallet_store"), notify);

    WalletRpcImpl {
        store: DefaultWalletStore::new(db),
    }
}

impl<WS: WalletStore + 'static> WalletRpc for WalletRpcImpl<WS> {
    fn get_live_cells(
        &self,
        lock_hash: H256,
        page: usize,
        per_page: usize,
    ) -> Result<Vec<CellOutputWithOutPoint>> {
        let per_page = per_page.min(50);
        Ok(self
            .store
            .get_live_cells(&lock_hash, page.saturating_mul(per_page), per_page))
    }

    fn get_transactions(&self, _lock_hash: H256) -> Result<Vec<Transaction>> {
        unimplemented!()
    }
}

pub trait WalletStore: Sync + Send {
    fn get_live_cells(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
    ) -> Vec<CellOutputWithOutPoint>;
}

pub const WALLET_SUBSCRIBER: &str = "wallet";
pub const COLUMNS: u32 = 3;

const COLUMN_LOCK_HASH_LIVE_CELL: Col = 0;
const COLUMN_LOCK_HASH_TRANSACTION: Col = 1;
const COLUMN_OUT_POINT_LOCK_HASH: Col = 2;

#[derive(Serialize, Deserialize)]
struct InPoint(H256, u32);

#[derive(Serialize, Deserialize, Clone)]
struct LockHashCellOutput(H256, BlockNumber, CellOutput);

fn serialize_lock_hash_key(
    lock_hash: &H256,
    block_number: BlockNumber,
    tx_hash: &H256,
    output_index: u32,
) -> Vec<u8> {
    let mut key = lock_hash.to_vec();
    key.extend_from_slice(&block_number.to_le_bytes());
    key.extend_from_slice(tx_hash.as_bytes());
    key.extend_from_slice(&output_index.to_le_bytes());
    key
}

fn deserialize_lock_hash_key(slice: &[u8]) -> (H256, BlockNumber, H256, u32) {
    let lock_hash = H256::from_slice(&slice[0..32]).unwrap();
    let block_number = BlockNumber::from_le_bytes(slice[32..40].try_into().unwrap());
    let tx_hash = H256::from_slice(&slice[40..72]).unwrap();
    let index = u32::from_le_bytes(slice[72..76].try_into().unwrap());
    (lock_hash, block_number, tx_hash, index)
}

pub struct DefaultWalletStore<T> {
    db: Arc<T>,
}

impl<T: IterableKeyValueDB + 'static> DefaultWalletStore<T> {
    pub fn new(db: Arc<T>) -> Self {
        DefaultWalletStore { db }
    }

    pub fn start<S: ToString>(self, thread_name: Option<S>, notify: &NotifyController) {
        let mut thread_builder = thread::Builder::new();
        if let Some(name) = thread_name {
            thread_builder = thread_builder.name(name.to_string());
        }

        let new_tip_receiver = notify.subscribe_new_tip(WALLET_SUBSCRIBER);
        thread_builder
            .spawn(move || loop {
                select! {
                    recv(new_tip_receiver) -> msg => match msg {
                        Ok(tip_changes) => self.update(&tip_changes.detached_blocks, &tip_changes.attached_blocks),
                        _ => {
                            error!(target: "rpc", "new_tip_receiver closed");
                            break;
                        }
                    },
                }
            })
            .expect("Start DefaultWalletStore failed");
    }

    pub(crate) fn update(&self, detached_blocks: &[Block], attached_blocks: &[Block]) {
        match self.db.batch() {
            Ok(mut batch) => {
                detached_blocks
                    .iter()
                    .for_each(|block| self.detach_block(&mut batch, block));
                // rocksdb rust binding doesn't support transactional batch read, have to use a batch buffer here.
                let mut batch_buffer = HashMap::<CoreOutPoint, LockHashCellOutput>::new();
                attached_blocks
                    .iter()
                    .for_each(|block| self.attach_block(&mut batch, &mut batch_buffer, block));
                if let Err(err) = batch.commit() {
                    error!(target: "rpc", "wallet db failed to commit batch, error: {:?}", err)
                }
            }
            Err(err) => {
                error!(target: "rpc", "wallet db failed to create new batch, error: {:?}", err);
            }
        }
    }

    fn detach_block(&self, batch: &mut DbBatch, block: &Block) {
        let block_number = block.header().number();
        block.commit_transactions().iter().for_each(|tx| {
            let tx_hash = tx.hash();
            if !tx.is_cellbase() {
                tx.inputs().iter().enumerate().for_each(|(index, input)| {
                    let index = index as u32;
                    let lock_hash_cell_output = self.get_lock_hash_cell_output(&input.previous_output);
                    let lock_hash_key = serialize_lock_hash_key(
                        &lock_hash_cell_output.0,
                        block_number,
                        &tx_hash,
                        index,
                    );

                    batch
                        .insert(
                            COLUMN_LOCK_HASH_LIVE_CELL,
                            &lock_hash_key,
                            &serialize(&lock_hash_cell_output.2)
                                .expect("serialize CellOutput should be ok"),
                        )
                        .expect("batch insert lock_hash_live_cell failed");

                    batch
                        .insert(
                            COLUMN_LOCK_HASH_TRANSACTION,
                            &lock_hash_key,
                            &serialize(&None::<InPoint>).expect("serialize None should be ok"),
                        )
                        .expect("batch insert lock_hash_transaction failed");
                });
            }

            tx.outputs().iter().enumerate().for_each(|(index, output)| {
                let index = index as u32;
                let lock_hash = output.lock.hash();
                let lock_hash_key =
                    serialize_lock_hash_key(&lock_hash, block_number, &tx_hash, index);
                let out_point = CoreOutPoint::new(tx_hash.clone(), index);

                batch
                    .delete(COLUMN_LOCK_HASH_LIVE_CELL, &lock_hash_key)
                    .expect("batch delete lock_hash_live_cell failed");
                batch
                    .delete(COLUMN_LOCK_HASH_TRANSACTION, &lock_hash_key)
                    .expect("batch delete lock_hash_transaction failed");
                batch
                    .delete(
                        COLUMN_OUT_POINT_LOCK_HASH,
                        &serialize(&out_point).expect("serialize OutPoint should be ok"),
                    )
                    .expect("batch delete out_point_lock_hash failed");
            });
        })
    }

    fn attach_block(
        &self,
        batch: &mut DbBatch,
        batch_buffer: &mut HashMap<CoreOutPoint, LockHashCellOutput>,
        block: &Block,
    ) {
        let block_number = block.header().number();
        block.commit_transactions().iter().for_each(|tx| {
            let tx_hash = tx.hash();
            tx.outputs().iter().enumerate().for_each(|(index, output)| {
                let index = index as u32;
                let lock_hash = output.lock.hash();
                let lock_hash_key =
                    serialize_lock_hash_key(&lock_hash, block_number, &tx_hash, index);
                let out_point = CoreOutPoint::new(tx_hash.clone(), index);
                let lock_hash_cell_output =
                    LockHashCellOutput(lock_hash.clone(), block_number, output.clone());

                batch
                    .insert(
                        COLUMN_LOCK_HASH_LIVE_CELL,
                        &lock_hash_key,
                        &serialize(&lock_hash_cell_output.2)
                            .expect("serialize CellOutput should be ok"),
                    )
                    .expect("batch insert lock_hash_live_cell failed");

                batch
                    .insert(
                        COLUMN_LOCK_HASH_TRANSACTION,
                        &lock_hash_key,
                        &serialize(&None::<InPoint>).expect("serialize None should be ok"),
                    )
                    .expect("batch insert lock_hash_transaction failed");

                batch
                    .insert(
                        COLUMN_OUT_POINT_LOCK_HASH,
                        &serialize(&out_point).expect("serialize OutPoint should be ok"),
                        &serialize(&lock_hash_cell_output)
                            .expect("serialize LockHashCellOutput should be ok"),
                    )
                    .expect("batch insert out_point_lock_hash failed");

                batch_buffer.insert(out_point, lock_hash_cell_output);
            });

            if !tx.is_cellbase() {
                tx.inputs().iter().enumerate().for_each(|(index, input)| {
                    // lookup lock_hash in the batch map and db
                    let index = index as u32;
                    let lock_hash_cell_output = batch_buffer
                        .get(&input.previous_output)
                        .cloned()
                        .unwrap_or_else(|| self.get_lock_hash_cell_output(&input.previous_output));
                    let lock_hash_key = serialize_lock_hash_key(
                        &lock_hash_cell_output.0,
                        lock_hash_cell_output.1,
                        &input.previous_output.hash,
                        input.previous_output.index,
                    );
                    let in_point = InPoint(tx_hash.clone(), index);

                    batch
                        .delete(COLUMN_LOCK_HASH_LIVE_CELL, &lock_hash_key)
                        .expect("batch delete lock_hash_live_cell failed");
                    batch
                        .insert(
                            COLUMN_LOCK_HASH_TRANSACTION,
                            &lock_hash_key,
                            &serialize(&in_point).expect("serializing InPoint be ok"),
                        )
                        .expect("batch insert lock_hash_outputs failed");
                });
            }
        })
    }

    fn get_lock_hash_cell_output(&self, out_point: &CoreOutPoint) -> LockHashCellOutput {
        deserialize(
            &self
                .db
                .read(
                    COLUMN_OUT_POINT_LOCK_HASH,
                    &serialize(out_point).expect("serialize OutPoint should be ok"),
                )
                .expect("wallet db read should be ok")
                .expect("inconsistent wallet db state"),
        )
        .expect("db safe access")
    }
}

impl<T: IterableKeyValueDB + 'static> WalletStore for DefaultWalletStore<T> {
    fn get_live_cells(
        &self,
        lock_hash: &H256,
        skip_num: usize,
        take_num: usize,
    ) -> Vec<CellOutputWithOutPoint> {
        let iter = self
            .db
            .iter(COLUMN_LOCK_HASH_LIVE_CELL, lock_hash.as_bytes())
            .expect("wallet db iter should be ok");
        iter.take_while(|(key, _)| key.starts_with(lock_hash.as_bytes()))
            .skip(skip_num)
            .take(take_num)
            .map(|(key, value)| {
                let output: CellOutput = deserialize(&value).expect("deserialize should be ok");
                let (_, _, hash, index) = deserialize_lock_hash_key(&key);
                CellOutputWithOutPoint {
                    out_point: OutPoint { hash, index },
                    lock: output.lock,
                    capacity: output.capacity,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, TransactionBuilder};
    use tempfile;

    fn setup_store(prefix: &str) -> DefaultWalletStore<RocksDB> {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let db = RocksDB::open(&config, COLUMNS);
        DefaultWalletStore::new(Arc::new(db))
    }

    #[test]
    fn get_live_cells() {
        let store = setup_store("get_live_cells");
        let script1 = Script::always_success();
        let script2 = Script::default();

        let tx11 = TransactionBuilder::default()
            .output(CellOutput::new(1000, vec![11], script1.clone(), None))
            .build();

        let tx12 = TransactionBuilder::default()
            .output(CellOutput::new(2000, vec![12], script2.clone(), None))
            .build();

        let block1 = BlockBuilder::default()
            .commit_transaction(tx11.clone())
            .commit_transaction(tx12.clone())
            .with_header_builder(HeaderBuilder::default().number(1));

        let tx21 = TransactionBuilder::default()
            .output(CellOutput::new(3000, vec![21], script1.clone(), None))
            .build();

        let tx22 = TransactionBuilder::default()
            .output(CellOutput::new(4000, vec![22], script2.clone(), None))
            .build();

        let block2 = BlockBuilder::default()
            .commit_transaction(tx21)
            .commit_transaction(tx22)
            .with_header_builder(HeaderBuilder::default().number(2));

        let tx31 = TransactionBuilder::default()
            .input(CellInput::new(CoreOutPoint::new(tx11.hash(), 0), 0, vec![]))
            .output(CellOutput::new(5000, vec![31], script1.clone(), None))
            .build();

        let tx32 = TransactionBuilder::default()
            .input(CellInput::new(CoreOutPoint::new(tx12.hash(), 0), 0, vec![]))
            .output(CellOutput::new(6000, vec![32], script2.clone(), None))
            .build();

        let block3 = BlockBuilder::default()
            .commit_transaction(tx31)
            .commit_transaction(tx32)
            .with_header_builder(HeaderBuilder::default().number(3));

        store.update(&[], &[block1, block2.clone()]);
        let cells = store.get_live_cells(&script1.hash(), 0, 100);
        assert_eq!(2, cells.len());
        assert_eq!(1000, cells[0].capacity);
        assert_eq!(3000, cells[1].capacity);

        let cells = store.get_live_cells(&script2.hash(), 0, 100);
        assert_eq!(2, cells.len());
        assert_eq!(2000, cells[0].capacity);
        assert_eq!(4000, cells[1].capacity);

        store.update(&[block2], &[block3]);
        let cells = store.get_live_cells(&script1.hash(), 0, 100);
        assert_eq!(1, cells.len());
        assert_eq!(5000, cells[0].capacity);

        let cells = store.get_live_cells(&script2.hash(), 0, 100);
        assert_eq!(1, cells.len());
        assert_eq!(6000, cells[0].capacity);
    }
}
