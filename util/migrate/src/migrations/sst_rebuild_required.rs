use ckb_db::RocksDB;
use ckb_db_migration::{Migration, ProgressBar};
use ckb_error::{Error, InternalErrorKind};
use std::sync::Arc;

pub const VERSION: &str = "20260421000000";

pub struct SstRebuildRequired;

impl Migration for SstRebuildRequired {
    fn migrate(
        &self,
        _db: RocksDB,
        _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error> {
        Err(InternalErrorKind::Database
            .other("RocksDB key schema migration requires `ckb migrate --sst-rebuild`")
            .into())
    }

    fn version(&self) -> &str {
        VERSION
    }
}
