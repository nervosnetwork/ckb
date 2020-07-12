use crate::ChainStore;
use ckb_script_data_loader::DataLoader;
use ckb_types::{
    bytes::Bytes,
    core::{cell::CellMeta, HeaderView},
    packed::Byte32,
    prelude::*,
};

pub struct DataLoaderWrapper<'a, T>(&'a T);
impl<'a, T: ChainStore<'a>> DataLoaderWrapper<'a, T> {
    pub fn new(source: &'a T) -> Self {
        DataLoaderWrapper(source)
    }
}

impl<'a, T: ChainStore<'a>> DataLoader for DataLoaderWrapper<'a, T> {
    fn load_cell_data(&self, cell: &CellMeta) -> Option<(Bytes, Byte32)> {
        cell.mem_cell_data
            .as_ref()
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.0
                    .get_cell_data(&cell.out_point.tx_hash(), cell.out_point.index().unpack())
            })
    }

    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
        self.0.get_block_header(block_hash)
    }
}
