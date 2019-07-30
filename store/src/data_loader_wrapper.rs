use crate::ChainStore;
use ckb_core::{cell::CellMeta, extras::BlockExt, Bytes};
use ckb_script_data_loader::DataLoader;
use numext_fixed_hash::H256;
use std::sync::Arc;

pub struct DataLoaderWrapper<CS>(Arc<CS>);
impl<CS> DataLoaderWrapper<CS> {
    pub fn new(source: Arc<CS>) -> Self {
        DataLoaderWrapper(source)
    }
}

impl<CS: ChainStore> DataLoader for DataLoaderWrapper<CS> {
    // load CellOutput
    fn load_cell_data(&self, cell: &CellMeta) -> Option<Bytes> {
        cell.mem_cell_data
            .as_ref()
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.0
                    .get_cell_data(&cell.out_point.tx_hash, cell.out_point.index)
            })
    }
    // load BlockExt
    #[inline]
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.0.get_block_ext(block_hash)
    }
}
