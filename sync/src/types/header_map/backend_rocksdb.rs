use std::path;

use ckb_db::internal::{
    ops::{Delete as _, GetPinned as _, Open as _, Put as _},
    DB,
};
use ckb_logger::{debug, warn};
use ckb_types::{packed::Byte32, prelude::*};
use tempfile::TempDir;

use super::KeyValueBackend;
use crate::types::HeaderView;

pub(crate) struct RocksDBBackend {
    tmpdir: Option<path::PathBuf>,
    resource: Option<(TempDir, DB)>,
    count: usize,
}

impl KeyValueBackend for RocksDBBackend {
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
                if let Ok(db) = DB::open_default(cache_dir.path()) {
                    debug!(
                        "open a key-value database({}) to save header map into disk",
                        cache_dir.path().to_str().unwrap_or("")
                    );
                    self.resource.replace((cache_dir, db));
                } else {
                    panic!("failed to open a key-value database to save header map into disk");
                }
            } else {
                panic!("failed to create a tempdir to save header map into disk");
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

    fn contains_key(&self, key: &Byte32) -> bool {
        if let Some((_, ref db)) = self.resource {
            db.get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read header map from disk should be ok, but {}", err))
                .is_some()
        } else {
            false
        }
    }

    fn get(&self, key: &Byte32) -> Option<HeaderView> {
        if let Some((_, ref db)) = self.resource {
            db.get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read header map from disk should be ok, but {}", err))
                .map(|slice| HeaderView::from_slice_should_be_ok(&slice))
        } else {
            None
        }
    }

    fn insert(&mut self, value: &HeaderView) -> Option<HeaderView> {
        if let Some((_, ref db)) = self.resource {
            let key = value.hash();
            let old_value_opt = db
                .get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read header map from disk should be ok, but {}", err))
                .map(|slice| HeaderView::from_slice_should_be_ok(&slice));
            if db.put(key.as_slice(), &value.to_vec()).is_err() {
                panic!("failed to insert item into header map");
            }
            if old_value_opt.is_none() {
                self.count += 1;
            }
            old_value_opt
        } else {
            None
        }
    }

    fn remove(&mut self, key: &Byte32) -> Option<HeaderView> {
        let mut do_count = false;
        let value_opt = if let Some((_, ref db)) = self.resource {
            let value_opt = db
                .get_pinned(key.as_slice())
                .unwrap_or_else(|err| panic!("read header map from disk should be ok, but {}", err))
                .map(|slice| HeaderView::from_slice_should_be_ok(&slice));
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
