use std::path;

use ckb_types::packed::Byte32;

use crate::types::HeaderView;

pub(crate) trait KeyValueBackend {
    fn new<P>(tmpdir: Option<P>) -> Self
    where
        P: AsRef<path::Path>;

    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn contains_key(&self, key: &Byte32) -> bool;
    fn get(&self, key: &Byte32) -> Option<HeaderView>;
    fn insert(&self, value: &HeaderView) -> Option<()>;
    fn insert_batch(&self, values: &[HeaderView]);
    fn remove(&self, key: &Byte32) -> Option<HeaderView>;
    fn remove_no_return(&self, key: &Byte32);
}
