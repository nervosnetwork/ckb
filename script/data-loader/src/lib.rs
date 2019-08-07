use ckb_core::extras::BlockExt;
use ckb_core::{cell::CellMeta, header::Header, Bytes};
use numext_fixed_hash::H256;

/// Script DataLoader
/// abstract the data access layer
pub trait DataLoader {
    // load cell data
    fn load_cell_data(&self, cell: &CellMeta) -> Option<Bytes>;
    // load BlockExt
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;
    // load header
    fn get_header(&self, block_hash: &H256) -> Option<Header>;
}
