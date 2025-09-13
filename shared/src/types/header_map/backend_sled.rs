use crate::types::HeaderIndexView;
use ckb_types::{packed::Byte32, prelude::*};
use sled::{Config, Db, Mode};
use std::path;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::TempDir;

pub(crate) struct SledBackend {
    count: AtomicUsize,
    db: Db,
    _tmpdir: Option<TempDir>,
}

impl SledBackend {
    pub fn new<P>(header_map_base_path: Option<P>) -> Self
    where
        P: AsRef<path::Path>,
    {
        let mut _tmpdir = None;
        let header_map_base_path: PathBuf = header_map_base_path
            .map(|p| p.as_ref().to_path_buf())
            .unwrap_or_else(|| {
                let mut builder = tempfile::Builder::new();
                builder.prefix("ckb-tmp-");
                let tmpdir = builder.tempdir().expect("create a temporary directory");
                let path = tmpdir.path().to_path_buf();
                _tmpdir = Some(tmpdir);
                path
            });
        let header_map_path = header_map_base_path.join("header_map");

        // use a smaller system page cache here since we are using sled as a temporary storage,
        // most of the time we will only read header from memory.
        let db: Db = Config::new()
            .mode(Mode::HighThroughput)
            .cache_capacity(64 * 1024 * 1024)
            .path(header_map_path)
            .open()
            .expect("failed to open a key-value database to save header map into disk");

        Self {
            db,
            _tmpdir,
            count: AtomicUsize::new(0),
        }
    }

    fn len(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains_key(&self, key: &Byte32) -> bool {
        self.db
            .contains_key(key.as_slice())
            .expect("sled contains_key")
    }

    fn get(&self, key: &Byte32) -> Option<HeaderIndexView> {
        self.db
            .get(key.as_slice())
            .unwrap_or_else(|err| panic!("read header map from disk should be ok, but {err}"))
            .map(|slice| HeaderIndexView::from_slice_should_be_ok(key.as_slice(), slice.as_ref()))
    }

    fn insert(&self, value: &HeaderIndexView) -> Option<()> {
        let key = value.hash();
        let last_value = self
            .db
            .insert(key.as_slice(), value.to_vec())
            .expect("failed to insert item to sled");
        if last_value.is_none() {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        last_value.map(|_| ())
    }

    pub fn insert_batch(&self, values: &[HeaderIndexView]) {
        let mut count = 0;
        for value in values {
            let key = value.hash();
            let last_value = self
                .db
                .insert(key.as_slice(), value.to_vec())
                .expect("failed to insert item to sled");
            if last_value.is_none() {
                count += 1;
            }
        }
        self.count.fetch_add(count, Ordering::SeqCst);
    }

    pub fn remove(&self, key: &Byte32) -> Option<HeaderIndexView> {
        let old_value = self
            .db
            .remove(key.as_slice())
            .expect("failed to remove item from sled");

        old_value.map(|slice| {
            self.count.fetch_sub(1, Ordering::SeqCst);
            HeaderIndexView::from_slice_should_be_ok(key.as_slice(), &slice)
        })
    }

    pub fn remove_no_return(&self, key: &Byte32) {
        let old_value = self
            .db
            .remove(key.as_slice())
            .expect("failed to remove item from sled");
        if old_value.is_some() {
            self.count.fetch_sub(1, Ordering::SeqCst);
        }
    }
}
