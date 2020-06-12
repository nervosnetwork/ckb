use ckb_app_config::StoreConfig;
use ckb_chain_iter::ChainIterator;
use ckb_db::RocksDB;
use ckb_db_migration::Migration;
use ckb_error::Error;
use ckb_store::{attach_block_cell, ChainDB, COLUMN_CELL};

const FREEZER_VERSION: &str = "20200603184756";
const BATCH: u64 = 1000;

pub struct FreezerMigration;

impl Migration for FreezerMigration {
    fn migrate(&self, mut db: RocksDB) -> Result<RocksDB, Error> {
        clean_cell_column(&mut db)?;
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let iter = ChainIterator::new(&chain_db);
        let mut count = 0;
        let mut txn = chain_db.begin_transaction();
        for block in iter {
            attach_block_cell(&txn, &block)?;
            count += 1;
            if count == BATCH {
                txn.commit()?;
                txn = chain_db.begin_transaction();
                count = 0;
            }
        }
        if count != 0 {
            txn.commit()?;
        }
        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        FREEZER_VERSION
    }
}

// https://github.com/facebook/rocksdb/issues/1295
fn clean_cell_column(db: &mut RocksDB) -> Result<(), Error> {
    db.drop_cf(COLUMN_CELL)?;
    db.create_cf(COLUMN_CELL)?;
    Ok(())
}
