use std::path;

use ckb_db::internal::{
    ops::{Delete as _, GetPinned as _, Open as _, Put as _},
    BlockBasedOptions, Options, DB,
};
use ckb_logger::{debug, warn};
use tempfile::TempDir;

use super::{Key, KeyValueBackend, StorageBackend, Value};

pub(crate) struct RocksDBBackend {
    tmpdir: Option<path::PathBuf>,
    resource: Option<(TempDir, DB)>,
    count: usize,
}

impl StorageBackend for RocksDBBackend {
    fn new<P>(tmpdir: Option<P>) -> Self
    where
        P: AsRef<path::Path>,
    {
        Self {
            tmpdir: tmpdir.map(|p| p.as_ref().to_path_buf()),
            resource: None,
            count: 0,
        }
    }

    fn len(&self) -> usize {
        self.count
    }

    fn is_opened(&self) -> bool {
        self.resource.is_some()
    }

    fn open(&mut self) {
        if !self.is_opened() {
            let mut builder = tempfile::Builder::new();
            builder.prefix("ckb-tmp-");
            let cache_dir_res = if let Some(ref tmpdir) = self.tmpdir {
                builder.tempdir_in(tmpdir)
            } else {
                builder.tempdir()
            };
            if let Ok(cache_dir) = cache_dir_res {
                // We minimize memory usage at all costs here.
                // If we want to use more memory, we should increase the limit of KeyValueMemory.
                let opts = {
                    let mut block_opts = BlockBasedOptions::default();
                    block_opts.disable_cache();
                    let mut opts = Options::default();
                    opts.create_if_missing(true);
                    opts.set_block_based_table_factory(&block_opts);
                    opts.set_write_buffer_size(4 * 1024 * 1024);
                    opts.set_max_write_buffer_number(2);
                    opts.set_min_write_buffer_number_to_merge(1);
                    opts
                };
                if let Ok(db) = DB::open(&opts, cache_dir.path()) {
                    debug!(
                        "open a key-value database({}) to save hashmap into disk",
                        cache_dir.path().to_str().unwrap_or("")
                    );
                    self.resource.replace((cache_dir, db));
                } else {
                    panic!("failed to open a key-value database to save hashmap into disk");
                }
            } else {
                panic!("failed to create a tempdir to save hashmap into disk");
            }
        }
    }

    fn try_close(&mut self) -> bool {
        if self.is_opened() {
            if self.is_empty() {
                if let Some((cache_dir, db)) = self.resource.take() {
                    drop(db);
                    let _ignore = cache_dir.close();
                }
                true
            } else {
                false
            }
        } else {
            true
        }
    }
}

impl<K, V> KeyValueBackend<K, V> for RocksDBBackend
where
    K: Key,
    V: Value,
{
    fn contains_key(&self, key: &K) -> bool {
        if let Some((_, ref db)) = self.resource {
            db.get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err))
                .is_some()
        } else {
            false
        }
    }

    fn get(&self, key: &K) -> Option<V> {
        if let Some((_, ref db)) = self.resource {
            db.get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err))
                .map(|slice| V::from_slice(&slice))
        } else {
            None
        }
    }

    fn insert(&mut self, key: &K, value: &V) -> Option<V> {
        if let Some((_, ref db)) = self.resource {
            let old_value_opt = db
                .get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err))
                .map(|slice| V::from_slice(&slice));
            if db.put(key.as_slice(), &value.to_vec()).is_err() {
                panic!("failed to insert item into hashmap");
            }
            if old_value_opt.is_none() {
                self.count += 1;
            }
            old_value_opt
        } else {
            None
        }
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let mut do_count = false;
        let value_opt = if let Some((_, ref db)) = self.resource {
            let value_opt = db
                .get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err))
                .map(|slice| V::from_slice(&slice));
            if value_opt.is_some() {
                if db.delete(key.as_slice()).is_ok() {
                    do_count = true;
                    value_opt
                } else {
                    warn!("failed to delete a value from database");
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        if do_count {
            self.count -= 1;
            self.try_close();
        }
        value_opt
    }
}
