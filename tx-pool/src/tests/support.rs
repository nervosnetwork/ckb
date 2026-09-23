use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_snapshot::Snapshot;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    U256,
    core::{Capacity, TransactionBuilder, TransactionView},
    packed::{self, Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::*,
};
use std::sync::Arc;

pub(crate) fn genesis_snapshot() -> Arc<Snapshot> {
    let consensus = Arc::new(ConsensusBuilder::default().build());
    let store = MockStore::default();
    let genesis = consensus.genesis_block();
    Arc::new(Snapshot::new(
        genesis.header(),
        U256::zero(),
        consensus.genesis_epoch_ext().clone(),
        store.store().get_snapshot(),
        Default::default(),
        consensus,
    ))
}

pub(crate) fn build_tx(inputs: Vec<(&Byte32, u32)>, outputs_len: usize) -> TransactionView {
    TransactionBuilder::default()
        .inputs(
            inputs
                .into_iter()
                .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
        )
        .outputs((0..outputs_len).map(|i| {
            CellOutput::new_builder()
                .capacity(Capacity::bytes(i + 1).unwrap())
                .build()
        }))
        .outputs_data((0..outputs_len).map(|_| packed::Bytes::default()))
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
                .capacity(Capacity::bytes(i + 1).unwrap())
                .build()
        }))
        .outputs_data((0..outputs_len).map(|_| packed::Bytes::default()))
        .build()
}
