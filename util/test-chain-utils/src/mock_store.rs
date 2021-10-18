use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::core::error::OutPointError;
use ckb_types::{
    core::{
        cell::{CellMetaBuilder, CellProvider, CellStatus, HeaderChecker},
        BlockExt, BlockView, EpochExt, HeaderView,
    },
    packed::{Byte32, OutPoint},
    prelude::*,
};
use faketime::unix_time_as_millis;
use std::sync::Arc;

/// A temporary RocksDB for mocking chain storage.
#[doc(hidden)]
#[derive(Clone)]
pub struct MockStore(pub Arc<ChainDB>);

impl Default for MockStore {
    fn default() -> Self {
        let db = RocksDB::open_tmp(COLUMNS);
        MockStore(Arc::new(ChainDB::new(db, Default::default())))
    }
}

impl MockStore {
    /// Create a new `MockStore` with insert parent block into the temporary database for reference.
    #[doc(hidden)]
    pub fn new(parent: &HeaderView, chain_store: &ChainDB) -> Self {
        let block = chain_store.get_block(&parent.hash()).unwrap();
        let epoch_ext = chain_store
            .get_block_epoch_index(&parent.hash())
            .and_then(|index| chain_store.get_epoch_ext(&index))
            .unwrap();
        let parent_block_ext = BlockExt {
            received_at: unix_time_as_millis(),
            total_difficulty: Default::default(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
        };
        let store = Self::default();
        {
            let db_txn = store.0.begin_transaction();
            db_txn
                .insert_block_ext(&block.parent_hash(), &parent_block_ext)
                .unwrap();
            db_txn.commit().unwrap();
        }
        store.insert_block(&block, &epoch_ext);
        store
    }

    /// Return the mock chainDB.
    #[doc(hidden)]
    pub fn store(&self) -> &ChainDB {
        &self.0
    }

    /// Insert a block into mock chainDB.
    #[doc(hidden)]
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
        {
            let parent_block_ext = self.0.get_block_ext(&block.parent_hash()).unwrap();
            let block_ext = BlockExt {
                received_at: unix_time_as_millis(),
                total_difficulty: parent_block_ext.total_difficulty.to_owned()
                    + block.header().difficulty(),
                total_uncles_count: parent_block_ext.total_uncles_count
                    + block.data().uncles().len() as u64,
                verified: Some(true),
                txs_fees: vec![],
            };
            db_txn.insert_block_ext(&block.hash(), &block_ext).unwrap();
        }
        db_txn.commit().unwrap();
    }

    /// Remove a block from mock chainDB.
    #[doc(hidden)]
    pub fn remove_block(&self, block: &BlockView) {
        let db_txn = self.0.begin_transaction();
        db_txn.delete_block(&block).unwrap();
        db_txn.detach_block(&block).unwrap();
        db_txn.commit().unwrap();
    }
}

impl CellProvider for MockStore {
    fn cell(&self, out_point: &OutPoint, _eager_load: bool) -> CellStatus {
        match self.0.get_transaction(&out_point.tx_hash()) {
            Some((tx, _)) => tx
                .outputs()
                .get(out_point.index().unpack())
                .map(|cell| {
                    let data = tx
                        .outputs_data()
                        .get(out_point.index().unpack())
                        .expect("output data");

                    let cell_meta = CellMetaBuilder::from_cell_output(cell, data.unpack())
                        .out_point(out_point.to_owned())
                        .build();

                    CellStatus::live_cell(cell_meta)
                })
                .unwrap_or(CellStatus::Unknown),
            None => CellStatus::Unknown,
        }
    }
}

impl HeaderChecker for MockStore {
    fn check_valid(&self, block_hash: &Byte32) -> Result<(), OutPointError> {
        if self.0.get_block_number(block_hash).is_some() {
            Ok(())
        } else {
            Err(OutPointError::InvalidHeader(block_hash.clone()))
        }
    }
}
