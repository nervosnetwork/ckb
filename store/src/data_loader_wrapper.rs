use crate::ChainStore;
use ckb_core::transaction::CellOutput;
use ckb_core::{cell::CellMeta, extras::BlockExt};
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
    fn lazy_load_cell_output(&self, cell: &CellMeta) -> CellOutput {
        match cell.cell_output.as_ref() {
            Some(output) => output.to_owned(),
            None => self
                .0
                .get_cell_output(&cell.out_point.tx_hash, cell.out_point.index)
                .expect("lazy load cell output from store"),
        }
    }
    // load BlockExt
    #[inline]
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.0.get_block_ext(block_hash)
    }
}
