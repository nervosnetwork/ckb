use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar};
use std::sync::Arc;

pub struct AddChainRootMMR;

// TODO(light-client) update the version number of this db migration.
const VERSION: &str = "20220214100000";

impl Migration for AddChainRootMMR {
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
