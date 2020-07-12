use ckb_types::{
    bytes::Bytes,
    core::{cell::CellMeta, HeaderView},
    packed::Byte32,
};

/// Script DataLoader
/// abstract the data access layer
pub trait DataLoader {
    // load cell data and its hash
    fn load_cell_data(&self, cell: &CellMeta) -> Option<(Bytes, Byte32)>;

    // load Header
    fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView>;
}
