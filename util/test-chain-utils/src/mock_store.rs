use ckb_core::block::Block;
use ckb_core::cell::{CellMetaBuilder, CellProvider, CellStatus, HeaderProvider, HeaderStatus};
use ckb_core::extras::EpochExt;
use ckb_core::header::Header;
use ckb_core::transaction::OutPoint;
use ckb_db::RocksDB;
use ckb_store::{ChainDB, ChainStore, COLUMNS};
use std::sync::Arc;

#[derive(Clone)]
pub struct MockStore(pub Arc<ChainDB>);

impl Default for MockStore {
    fn default() -> Self {
        let db = RocksDB::open_tmp(COLUMNS);
        MockStore(Arc::new(ChainDB::new(db)))
    }
}

impl MockStore {
    pub fn new(parent: &Header, chain_store: &ChainDB) -> Self {
        // Insert parent block into current mock store for referencing
        let block = chain_store.get_block(parent.hash()).unwrap();
        let epoch_ext = chain_store
            .get_block_epoch_index(parent.hash())
            .and_then(|index| chain_store.get_epoch_ext(&index))
            .unwrap();
        let store = Self::default();
        store.insert_block(&block, &epoch_ext);
        store
    }

    pub fn store(&self) -> &ChainDB {
        &self.0
    }

    pub fn insert_block(&self, block: &Block, epoch_ext: &EpochExt) {
        let db_txn = self.0.begin_transaction();
        db_txn.insert_block(&block).unwrap();
        db_txn.attach_block(&block).unwrap();
        db_txn
            .insert_block_epoch_index(
                &block.header().hash(),
                epoch_ext.last_block_hash_in_previous_epoch(),
            )
            .unwrap();
        db_txn
            .insert_epoch_ext(epoch_ext.last_block_hash_in_previous_epoch(), &epoch_ext)
            .unwrap();
        db_txn.commit().unwrap();
    }
}

impl CellProvider for MockStore {
    fn cell(&self, out_point: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &out_point.cell {
            match self.0.get_transaction(&cell_out_point.tx_hash) {
                Some((tx, _)) => tx
                    .outputs()
                    .get(cell_out_point.index as usize)
                    .map(|cell| {
                        let data = tx
                            .outputs_data()
                            .get(cell_out_point.index as usize)
                            .expect("output data");
                        let cell = cell.to_owned();
                        CellStatus::live_cell(
                            CellMetaBuilder::from_cell_output(cell, data.to_owned())
                                .out_point(cell_out_point.to_owned())
                                .build(),
                        )
                    })
                    .unwrap_or(CellStatus::Unknown),
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unspecified
        }
    }
}

impl HeaderProvider for MockStore {
    fn header(&self, out_point: &OutPoint) -> HeaderStatus {
        if let Some(block_hash) = &out_point.block_hash {
            match self.0.get_block_header(block_hash) {
                Some(header) => HeaderStatus::Live(Box::new(header)),
                None => HeaderStatus::Unknown,
            }
        } else {
            HeaderStatus::Unspecified
        }
    }
}
