use ckb_db::RocksDB;
use ckb_db_migration::{Migration, ProgressBar};
use ckb_db_schema::{COLUMN_BLOCK_FILTER, COLUMN_META, META_LATEST_BUILT_FILTER_DATA_KEY};
use ckb_error::Error;
use std::sync::Arc;

pub struct RebuildBlockFilter;

const VERSION: &str = "20220803143236";

/// TODO: this migration can be archived when release a new version of ckb
impl Migration for RebuildBlockFilter {
    fn migrate(
        &self,
        mut db: RocksDB,
        _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error> {
        clean_block_filter_column(&mut db)?;
        let mut wb = db.new_write_batch();
        wb.delete(COLUMN_META, META_LATEST_BUILT_FILTER_DATA_KEY)?;
        db.write(&wb)?;
        Ok(db)
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn expensive(&self) -> bool {
        false
    }
}

fn clean_block_filter_column(db: &mut RocksDB) -> Result<(), Error> {
    db.drop_cf(COLUMN_BLOCK_FILTER)?;
    db.create_cf(COLUMN_BLOCK_FILTER)?;
    Ok(())
}
