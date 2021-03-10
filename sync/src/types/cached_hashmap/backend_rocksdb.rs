use std::{collections::HashSet, path, sync::Arc};

use ckb_db::internal::{
    ops::{Delete as _, GetPinned as _, Open as _, Put as _},
    BlockBasedOptions, DBPinnableSlice, Options, DB,
};
use ckb_logger::{debug, trace, warn};
use ckb_util::RwLock;
use tempfile::TempDir;

use super::{Key, KeyValueBackend, StorageBackend, Value};

pub(crate) struct RocksDB {
    tmpdir: Option<path::PathBuf>,
    resource: Option<(TempDir, DB)>,
    references: HashSet<u8>,
}

pub(crate) struct RocksDBBackend {
    id: u8,
    rocksdb: Arc<RwLock<RocksDB>>,
    count: usize,
}

impl RocksDB {
    pub(crate) fn new<P>(tmpdir: Option<P>) -> Self
    where
        P: AsRef<path::Path>,
    {
        Self {
            tmpdir: tmpdir.map(|p| p.as_ref().to_path_buf()),
            resource: None,
            references: HashSet::new(),
        }
    }

    fn is_opened_by(&self, id: u8) -> bool {
        self.references.contains(&id)
    }

    fn open_by(&mut self, id: u8) {
        if self.resource.is_none() {
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
                        "open a key-value database({}) to cache hashmap into disk",
                        cache_dir.path().to_str().unwrap_or("")
                    );
                    self.resource.replace((cache_dir, db));
                } else {
                    panic!("failed to open a key-value database to cache hashmap into disk");
                }
            } else {
                panic!("failed to create a tempdir to cache hashmap into disk");
            }
        }
        self.references.insert(id);
    }

    fn try_close_by(&mut self, id: u8) {
        self.references.remove(&id);
        if self.references.is_empty() {
            debug!("close the cached hashmap by {}", id);
            if let Some((cache_dir, db)) = self.resource.take() {
                drop(db);
                let _ignore = cache_dir.close();
            }
        }
    }

    fn contains_key(&self, key: &[u8]) -> bool {
        if let Some((_, ref db)) = self.resource {
            db.get_pinned(key)
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err))
                .is_some()
        } else {
            false
        }
    }

    fn get(&self, key: &[u8]) -> Option<DBPinnableSlice> {
        if let Some((_, ref db)) = self.resource {
            db.get_pinned(key)
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err))
        } else {
            None
        }
    }

    fn insert(&self, key: &[u8], value: &[u8]) -> Option<DBPinnableSlice> {
        if let Some((_, ref db)) = self.resource {
            let old_value_opt = db
                .get_pinned(key)
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err));
            if db.put(key, value).is_err() {
                panic!("failed to insert item into hashmap");
            }
            old_value_opt
        } else {
            None
        }
    }

    fn remove(&self, key: &[u8]) -> Option<DBPinnableSlice> {
        if let Some((_, ref db)) = self.resource {
            let value_opt = db
                .get_pinned(key)
                .unwrap_or_else(|err| panic!("read hashmap from disk should be ok, but {}", err));
            if value_opt.is_some() {
                if db.delete(key).is_ok() {
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
        }
    }
}

impl RocksDBBackend {
    pub(crate) fn new(rocksdb: Arc<RwLock<RocksDB>>, id: u8) -> Self {
        Self {
            id,
            rocksdb,
            count: 0,
        }
    }
}

impl StorageBackend for RocksDBBackend {
    fn len(&self) -> usize {
        self.count
    }

    fn is_opened(&self) -> bool {
        self.rocksdb.read().is_opened_by(self.id)
    }

    fn open(&self) {
        self.rocksdb.write().open_by(self.id);
    }

    fn try_close(&self) -> bool {
        if self.is_opened() {
            if self.is_empty() {
                trace!("try close the cached hashmap by {}", self.id);
                self.rocksdb.write().try_close_by(self.id);
                true
            } else {
                false
            }
        } else {
            true
        }
    }
}

fn key_with_prefix<K: Key>(prefix: u8, key: &K) -> Vec<u8> {
    let s = key.as_slice();
    let mut k = Vec::with_capacity(s.len() + 1);
    k.push(prefix);
    k.extend_from_slice(s);
    k
}

impl<K, V> KeyValueBackend<K, V> for RocksDBBackend
where
    K: Key,
    V: Value,
{
    fn contains_key(&self, key: &K) -> bool {
        let k = key_with_prefix(self.id, key);
        self.rocksdb.read().contains_key(&k)
    }

    fn get(&self, key: &K) -> Option<V> {
        let k = key_with_prefix(self.id, key);
        self.rocksdb
            .read()
            .get(&k)
            .map(|slice| V::from_slice(&slice))
    }

    fn insert(&mut self, key: &K, value: &V) -> Option<V> {
        let k = key_with_prefix(self.id, key);
        let v = value.to_vec();
        let old_value_opt = self
            .rocksdb
            .read()
            .insert(&k, &v)
            .map(|slice| V::from_slice(&slice));
        if old_value_opt.is_none() {
            self.count += 1;
        }
        old_value_opt
    }

    fn remove(&mut self, key: &K) -> Option<V> {
        let k = key_with_prefix(self.id, key);
        let value_opt = self
            .rocksdb
            .read()
            .remove(&k)
            .map(|slice| V::from_slice(&slice));
        if value_opt.is_some() {
            self.count -= 1;
            self.try_close();
        }
        value_opt
    }
}
