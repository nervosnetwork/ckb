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

    fn is_opened(&self) -> bool;
    fn open(&mut self);
    fn try_close(&mut self) -> bool;

    fn contains_key(&self, key: &Byte32) -> bool;
    fn get(&self, key: &Byte32) -> Option<HeaderView>;
    fn insert(&mut self, value: &HeaderView) -> Option<HeaderView>;
    fn remove(&mut self, key: &Byte32) -> Option<HeaderView>;
}
