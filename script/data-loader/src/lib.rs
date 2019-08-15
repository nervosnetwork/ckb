use ckb_types::{
    bytes::Bytes,
    core::{cell::CellMeta, BlockExt, HeaderView},
    packed::Byte32,
};

/// Script DataLoader
/// abstract the data access layer
pub trait DataLoader {
    // load cell data
    fn load_cell_data(&self, cell: &CellMeta) -> Option<Bytes>;
    // load BlockExt
    fn get_block_ext(&self, block_hash: &Byte32) -> Option<BlockExt>;
    // load Header
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView>;
}
