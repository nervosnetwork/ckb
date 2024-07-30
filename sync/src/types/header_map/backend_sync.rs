use super::KeyValueBackend;
use crate::types::HeaderIndexView;
use ckb_types::packed::Byte32;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(crate) struct SyncBackend {
    db: Arc<Mutex<UnsafeCell<HashMap<Byte32, HeaderIndexView>>>>,
}

impl SyncBackend {
    fn get_mut_unsafe(&self) -> &mut HashMap<Byte32, HeaderIndexView> {
        unsafe { &mut *self.db.lock().unwrap().get() }
    }
    fn get(&self) -> &HashMap<Byte32, HeaderIndexView> {
        unsafe { &*self.db.lock().unwrap().get() }
    }
}

impl KeyValueBackend for SyncBackend {
    fn new<P>(_tmpdir: Option<P>) -> Self
    where
        P: AsRef<std::path::Path>,
    {
        Self {
            db: Arc::new(Mutex::new(UnsafeCell::new(HashMap::new()))),
        }
    }

    fn len(&self) -> usize {
        self.get().len()
    }
    fn is_empty(&self) -> bool {
        self.get().is_empty()
    }

    fn contains_key(&self, key: &Byte32) -> bool {
        self.get().contains_key(key)
    }
    fn get(&self, key: &Byte32) -> Option<HeaderIndexView> {
        self.get().get(key).map(|f| f.clone())
    }
    fn insert(&self, value: &HeaderIndexView) -> Option<()> {
        let key = value.hash();
        self.get_mut_unsafe()
            .insert(key, value.clone())
            .map(|_f| ())
    }
    fn insert_batch(&self, values: &[HeaderIndexView]) {
        for value in values {
            let key = value.hash();
            self.get_mut_unsafe()
                .insert(key, value.clone())
                .expect("failed to insert item to sled");
        }
    }
    fn remove(&self, key: &Byte32) -> Option<HeaderIndexView> {
        self.get_mut_unsafe().remove(key)
    }
    fn remove_no_return(&self, key: &Byte32) {
        self.get_mut_unsafe().remove(key);
    }
}
