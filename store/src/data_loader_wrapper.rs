//! TODO(doc): @quake
use crate::ChainStore;
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::HeaderView,
    packed::{Byte32, OutPoint},
};

/// TODO(doc): @quake
pub struct DataLoaderWrapper<'a, T>(&'a T);
impl<'a, T: ChainStore<'a>> DataLoaderWrapper<'a, T> {
    /// TODO(doc): @quake
    pub fn new(source: &'a T) -> Self {
        DataLoaderWrapper(source)
    }
}

impl<'a, T: ChainStore<'a>> CellDataProvider for DataLoaderWrapper<'a, T> {
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<(Bytes, Byte32)> {
        self.0.get_cell_data(out_point)
    }
}

impl<'a, T: ChainStore<'a>> HeaderProvider for DataLoaderWrapper<'a, T> {
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.0.get_block_header(block_hash)
    }
}
