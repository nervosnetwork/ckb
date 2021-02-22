use crate::ChainStore;
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::HeaderView,
    packed::{Byte32, OutPoint},
    prelude::*,
};

pub struct DataLoaderWrapper<'a, T>(&'a T);
impl<'a, T: ChainStore<'a>> DataLoaderWrapper<'a, T> {
    pub fn new(source: &'a T) -> Self {
        DataLoaderWrapper(source)
    }
}

impl<'a, T: ChainStore<'a>> CellDataProvider for DataLoaderWrapper<'a, T> {
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<Bytes> {
        self.0
            .get_cell_data(&out_point.tx_hash(), out_point.index().unpack())
            .map(|(data, _)| data)
    }

    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<Byte32> {
        self.0
            .get_cell_data(&out_point.tx_hash(), out_point.index().unpack())
            .map(|(_, data_hash)| data_hash)
    }
}

impl<'a, T: ChainStore<'a>> HeaderProvider for DataLoaderWrapper<'a, T> {
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.0.get_block_header(block_hash)
    }
}
