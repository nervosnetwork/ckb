use ckb_types::{
    bytes::Bytes,
    core::{Capacity, Cycle, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
};

pub(crate) const DEFAULT_MAX_ANCESTORS_SIZE: usize = 25;
pub(crate) const MOCK_CYCLES: Cycle = 0;
pub(crate) const MOCK_FEE: Capacity = Capacity::zero();
pub(crate) const MOCK_SIZE: usize = 0;

pub(crate) fn build_tx(inputs: Vec<(&Byte32, u32)>, outputs_len: usize) -> TransactionView {
    TransactionBuilder::default()
        .inputs(
            inputs
                .into_iter()
                .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
        )
        .outputs((0..outputs_len).map(|i| {
            CellOutput::new_builder()
                .capacity(Capacity::bytes(i + 1).unwrap().pack())
                .build()
        }))
        .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
        .build()
}

pub(crate) fn build_tx_with_dep(
    inputs: Vec<(&Byte32, u32)>,
    deps: Vec<(&Byte32, u32)>,
    outputs_len: usize,
) -> TransactionView {
    TransactionBuilder::default()
        .inputs(
            inputs
                .into_iter()
                .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
        )
        .cell_deps(deps.into_iter().map(|(txid, index)| {
            CellDep::new_builder()
                .out_point(OutPoint::new(txid.to_owned(), index))
                .build()
        }))
        .outputs((0..outputs_len).map(|i| {
            CellOutput::new_builder()
                .capacity(Capacity::bytes(i + 1).unwrap().pack())
                .build()
        }))
        .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
        .build()
}

pub(crate) fn build_tx_with_header_dep(
    inputs: Vec<(&Byte32, u32)>,
    header_deps: Vec<Byte32>,
    outputs_len: usize,
) -> TransactionView {
    TransactionBuilder::default()
        .inputs(
            inputs
                .into_iter()
                .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
        )
        .set_header_deps(header_deps)
        .outputs((0..outputs_len).map(|i| {
            CellOutput::new_builder()
                .capacity(Capacity::bytes(i + 1).unwrap().pack())
                .build()
        }))
        .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
        .build()
}
