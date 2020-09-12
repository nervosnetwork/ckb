use crate::types::LockHashIndex;
use ckb_db::{Col, Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar};
use ckb_shared::shared::Shared;
use ckb_store::ChainStore;

use ckb_types::{
    packed::{CellOutput, LiveCellOutput, LockHashIndexReader},
    prelude::*,
};

pub struct AddFieldsToLiveCell {
    shared: Shared,
}

impl AddFieldsToLiveCell {
    pub fn new(shared: Shared) -> Self {
        Self { shared }
    }
}

impl Migration for AddFieldsToLiveCell {
    fn migrate(&self, db: RocksDB, _pb: Box<dyn FnMut(u64) -> ProgressBar>) -> Result<RocksDB> {
        const COLUMN_LOCK_HASH_LIVE_CELL: Col = "1";

        let snapshot = self.shared.snapshot();
        let txn = db.transaction();
        // Update `CellOutput` to `LiveCellOutput`
        let migration = |key: &[u8], value: &[u8]| -> Result<()> {
            let lock_hash_index = LockHashIndex::from_packed(
                LockHashIndexReader::from_slice(&key)
                    .expect("LockHashIndex in storage should be ok"),
            );
            let cell_output =
                CellOutput::from_slice(&value).expect("CellOutput in storage should be ok");
            let tx = snapshot
                .get_transaction(&lock_hash_index.out_point.tx_hash())
                .expect("Get tx from snapshot should be ok")
                .0;

            let live_cell_output = LiveCellOutput::new_builder()
                .cell_output(cell_output)
                .output_data_len(
                    (tx.outputs_data()
                        .get(lock_hash_index.out_point.index().unpack())
                        .expect("verified tx")
                        .len() as u64)
                        .pack(),
                )
                .cellbase(tx.is_cellbase().pack())
                .build();
            txn.put(COLUMN_LOCK_HASH_LIVE_CELL, key, live_cell_output.as_slice())?;
            Ok(())
        };
        db.traverse(COLUMN_LOCK_HASH_LIVE_CELL, migration)?;
        txn.commit()?;
        Ok(db)
    }

    fn version(&self) -> &str {
        "20191201091330"
    }
}
