use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use rocksdb::{Options, DB};

use crate::{
    Error, ROCKSDB_COL_CELL, ROCKSDB_COL_CELL_ALIAS, ROCKSDB_COL_CELL_INPUT, ROCKSDB_COL_SCRIPT,
    ROCKSDB_COL_TX,
};

pub fn with_rocksdb<P, T, F>(path: P, timeout: Option<Duration>, func: F) -> Result<T, Error>
where
    P: AsRef<Path>,
    F: FnOnce(&DB) -> Result<T, Error>,
{
    let path = path.as_ref().to_path_buf();
    let start = Instant::now();
    let timeout = timeout.unwrap_or(Duration::from_secs(6));
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    let columns = vec![
        // TODO: remove this later
        "key",
        ROCKSDB_COL_CELL,
        ROCKSDB_COL_CELL_ALIAS,
        ROCKSDB_COL_CELL_INPUT,
        ROCKSDB_COL_SCRIPT,
        ROCKSDB_COL_TX,
    ];
    loop {
        match DB::open_cf(&options, &path, &columns) {
            Ok(db) => break func(&db),
            Err(err) => {
                if start.elapsed() >= timeout {
                    log::warn!(
                        "Open rocksdb failed with error={}, timeout={:?}",
                        err,
                        timeout
                    );
                    break Err(err.into());
                }
                log::debug!("Failed open rocksdb: path={:?}, error={}", path, err);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
