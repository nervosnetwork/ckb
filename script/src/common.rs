use ckb_core::{cell::CellMeta, transaction::CellOutput};
use ckb_store::ChainStore;

/// Extend ChainStore
/// Lazy load cell output from chain store
pub trait LazyLoadCellOutput {
    fn lazy_load_cell_output(&self, cell_meta: &CellMeta) -> CellOutput;
}

impl<CS: ChainStore> LazyLoadCellOutput for CS {
    fn lazy_load_cell_output(&self, cell: &CellMeta) -> CellOutput {
        match cell.cell_output.as_ref() {
            Some(output) => output.to_owned(),
            None => self
                .get_cell_output(&cell.out_point.tx_hash, cell.out_point.index)
                .expect("lazy load cell output from store"),
        }
    }
}
