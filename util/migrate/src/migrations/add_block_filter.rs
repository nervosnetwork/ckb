use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar};
use std::sync::Arc;

pub struct AddBlockFilterColumnFamily;

const VERSION: &str = "20220228115700";

impl Migration for AddBlockFilterColumnFamily {
    fn migrate(
        &self,
        db: RocksDB,
        _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB> {
        Ok(db)
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn expensive(&self) -> bool {
        false
    }
}
