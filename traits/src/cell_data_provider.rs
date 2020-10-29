use ckb_types::{
    bytes::Bytes,
    core::cell::CellMeta,
    packed::{Byte32, OutPoint},
};

/// TODO(doc): @quake
pub trait CellDataProvider {
    /// TODO(doc): @quake
    fn load_cell_data(&self, cell: &CellMeta) -> Option<(Bytes, Byte32)> {
        cell.mem_cell_data
            .as_ref()
            .map(ToOwned::to_owned)
            .or_else(|| self.get_cell_data(&cell.out_point))
    }

    /// TODO(doc): @quake
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<(Bytes, Byte32)>;
}
