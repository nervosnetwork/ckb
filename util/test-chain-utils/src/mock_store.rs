use ckb_db::RocksDB;
use ckb_store::{ChainDB, ChainStore, COLUMNS};
use ckb_types::core::error::OutPointError;
use ckb_types::{
    core::{
        cell::{CellMetaBuilder, CellProvider, CellStatus, HeaderChecker},
        BlockView, EpochExt, HeaderView,
    },
    packed::{Byte32, OutPoint},
    prelude::*,
};
use std::sync::Arc;

/// TODO(doc): @chuijiaolianying
#[derive(Clone)]
pub struct MockStore(pub Arc<ChainDB>);

impl Default for MockStore {
    fn default() -> Self {
        let db = RocksDB::open_tmp(COLUMNS);
        MockStore(Arc::new(ChainDB::new(db, Default::default())))
    }
}

impl MockStore {
    /// TODO(doc): @chuijiaolianying
    pub fn new(parent: &HeaderView, chain_store: &ChainDB) -> Self {
        // Insert parent block into current mock store for referencing
        let block = chain_store.get_block(&parent.hash()).unwrap();
        let epoch_ext = chain_store
            .get_block_epoch_index(&parent.hash())
            .and_then(|index| chain_store.get_epoch_ext(&index))
            .unwrap();
        let store = Self::default();
        store.insert_block(&block, &epoch_ext);
        store
    }

    /// TODO(doc): @chuijiaolianying
    pub fn store(&self) -> &ChainDB {
        &self.0
    }

    /// TODO(doc): @chuijiaolianying
    pub fn insert_block(&self, block: &BlockView, epoch_ext: &EpochExt) {
        let db_txn = self.0.begin_transaction();
        let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn.insert_block(&block).unwrap();
        db_txn.attach_block(&block).unwrap();
        db_txn
            .insert_block_epoch_index(&block.hash(), &last_block_hash_in_previous_epoch)
            .unwrap();
        db_txn
            .insert_epoch_ext(&last_block_hash_in_previous_epoch, epoch_ext)
            .unwrap();
        db_txn.commit().unwrap();
    }

    /// TODO(doc): @chuijiaolianying
    pub fn remove_block(&self, block: &BlockView) {
        let db_txn = self.0.begin_transaction();
        db_txn
            .delete_block(&block.header().hash(), block.transactions().len())
            .unwrap();
        db_txn.detach_block(&block).unwrap();
        db_txn.commit().unwrap();
    }
}

impl CellProvider for MockStore {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        match self.0.get_transaction(&out_point.tx_hash()) {
            Some((tx, _)) => tx
                .outputs()
                .get(out_point.index().unpack())
                .map(|cell| {
                    let data = tx
                        .outputs_data()
                        .get(out_point.index().unpack())
                        .expect("output data");

                    let mut cell_meta = CellMetaBuilder::from_cell_output(cell, data.unpack())
                        .out_point(out_point.to_owned())
                        .build();
                    if !with_data {
                        cell_meta.mem_cell_data = None;
                    }

                    CellStatus::live_cell(cell_meta)
                })
                .unwrap_or(CellStatus::Unknown),
            None => CellStatus::Unknown,
        }
    }
}

impl HeaderChecker for MockStore {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), ckb_error::Error> {
        if self.0.get_block_number(block_hash).is_some() {
            Ok(())
        } else {
            Err(OutPointError::InvalidHeader(block_hash.clone()).into())
        }
    }
}
