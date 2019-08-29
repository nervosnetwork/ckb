use ckb_types::{
    bytes::Bytes,
    core::{cell::CellMeta, BlockExt, EpochExt, HeaderView},
    packed::Byte32,
};

/// Script DataLoader
/// abstract the data access layer
pub trait DataLoader {
    // load cell data and its hash
    fn load_cell_data(&self, cell: &CellMeta) -> Option<(Bytes, Byte32)>;
    // load BlockExt
    fn get_block_ext(&self, block_hash: &Byte32) -> Option<BlockExt>;
    // load block EpochExt
    fn get_block_epoch(&self, block_hash: &Byte32) -> Option<EpochExt>;
    // load Header
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView>;
}
