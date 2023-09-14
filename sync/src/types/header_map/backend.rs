use std::path;

use ckb_types::packed::Byte32;

use crate::types::HeaderIndexView;

pub(crate) trait KeyValueBackend {
    fn new<P>(tmpdir: Option<P>) -> Self
    where
        P: AsRef<path::Path>;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn contains_key(&self, key: &Byte32) -> bool;
    fn get(&self, key: &Byte32) -> Option<HeaderIndexView>;
    fn insert(&self, value: &HeaderIndexView) -> Option<()>;
    fn insert_batch(&self, values: &[HeaderIndexView]);
    fn remove(&self, key: &Byte32) -> Option<HeaderIndexView>;
    fn remove_no_return(&self, key: &Byte32);
}
