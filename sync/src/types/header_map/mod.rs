use std::path;

use ckb_types::packed::Byte32;
use ckb_util::Mutex;

use crate::types::HeaderView;

mod backend;
mod backend_rocksdb;
mod kernel_lru;
mod memory;

pub(crate) use self::{
    backend::KeyValueBackend, backend_rocksdb::RocksDBBackend, kernel_lru::HeaderMapLruKernel,
    memory::KeyValueMemory,
};

pub struct HeaderMapLru(Mutex<HeaderMapLruKernel<RocksDBBackend>>);

impl HeaderMapLru {
    pub(crate) fn new<P>(
        tmpdir: Option<P>,
        primary_limit: usize,
        backend_close_threshold: usize,
    ) -> Self
    where
        P: AsRef<path::Path>,
    {
        let inner = HeaderMapLruKernel::new(tmpdir, primary_limit, backend_close_threshold);
        Self(Mutex::new(inner))
    }

    pub(crate) fn contains_key(&self, hash: &Byte32) -> bool {
        self.0.lock().contains_key(hash)
    }

    pub(crate) fn get(&self, hash: &Byte32) -> Option<HeaderView> {
        self.0.lock().get(hash)
    }

    pub(crate) fn insert(&self, view: HeaderView) -> Option<HeaderView> {
        self.0.lock().insert(view)
    }

    pub(crate) fn remove(&self, hash: &Byte32) -> Option<HeaderView> {
        self.0.lock().remove(hash)
    }
}
