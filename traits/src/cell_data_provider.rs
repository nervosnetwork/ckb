use ckb_types::{
    bytes::Bytes,
    core::cell::CellMeta,
    packed::{Byte32, OutPoint},
};

/// Trait for cell_data storage
pub trait CellDataProvider {
    /// Load cell_data from memory, fallback to storage access
    fn load_cell_data(&self, cell: &CellMeta) -> Option<Bytes> {
        cell.mem_cell_data
            .as_ref()
            .map(ToOwned::to_owned)
            .or_else(|| self.get_cell_data(&cell.out_point))
    }

    /// Load cell_data_hash from memory, fallback to storage access
    fn load_cell_data_hash(&self, cell: &CellMeta) -> Option<Byte32> {
        cell.mem_cell_data_hash
            .as_ref()
            .map(ToOwned::to_owned)
            .or_else(|| self.get_cell_data_hash(&cell.out_point))
    }

    /// Fetch cell_data from storage
    fn get_cell_data(&self, out_point: &OutPoint) -> Option<Bytes>;

    /// Fetch cell_data_hash from storage, please note that loading a large amount of cell data
    /// and calculating hash may be a performance bottleneck, so here is a separate fn designed
    /// to facilitate caching.
    ///
    /// In unit test or other scenarios that are not performance bottlenecks, you may use the
    /// results of `get_cell_data` to calculate hash as a default implementation:
    /// self.get_cell_data(out_point).map(|data| CellOutput::calc_data_hash(&data))
    fn get_cell_data_hash(&self, out_point: &OutPoint) -> Option<Byte32>;
}
