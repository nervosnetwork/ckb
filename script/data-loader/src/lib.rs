use ckb_core::cell::CellMeta;
use ckb_core::extras::BlockExt;
use ckb_core::transaction::CellOutput;
use numext_fixed_hash::H256;

/// Script DataLoader
/// abstract the data access layer
pub trait DataLoader {
    // load CellOutput
    fn lazy_load_cell_output(&self, cell: &CellMeta) -> CellOutput;
    // load BlockExt
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;
}
