use crate::util::cell::{as_input, as_output};
use crate::Node;
use ckb_types::core::cell::CellMeta;
use ckb_types::core::{TransactionBuilder, TransactionView};

pub fn always_success_transactions(node: &Node, cells: &[CellMeta]) -> Vec<TransactionView> {
    cells
        .iter()
        .map(|cell| always_success_transaction(node, cell))
        .collect()
}

pub fn always_success_transaction(node: &Node, cell: &CellMeta) -> TransactionView {
    TransactionBuilder::default()
        .input(as_input(cell))
        .output(as_output(cell))
        .output_data(Default::default())
        .cell_dep(node.always_success_cell_dep())
        .build()
}
