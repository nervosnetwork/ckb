use crate::cell_set::{CellSet, CellSetDiff};
use crate::{
    shared::{Shared, SharedBuilder},
    store::ChainKVStore,
};
use ckb_core::cell::{CellProvider, CellStatus, UnresolvableError};
use ckb_core::transaction::Transaction;
use ckb_db::memorydb::MemoryKeyValueDB;
use fnv::FnvHashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

fn cell_set() -> CellSet {
    let file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("./src/tests/data/no1/cell_set.json"),
    )
    .unwrap();
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).unwrap()
}

fn cell_set_diff() -> CellSetDiff {
    let file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("./src/tests/data/no1/cell_set_diff.json"),
    )
    .unwrap();
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).unwrap()
}

fn transcations() -> Vec<Transaction> {
    let file =
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("./src/tests/data/no1/txs.json"))
            .unwrap();
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).unwrap()
}

fn new_shared() -> Shared<ChainKVStore<MemoryKeyValueDB>> {
    SharedBuilder::<MemoryKeyValueDB>::new().build()
}

#[test]
fn case_no1() {
    let shared = new_shared();
    let mut chain_state = shared.chain_state().lock();
    chain_state.cell_set = cell_set();

    let cell_set_diff = cell_set_diff();
    let outputs = FnvHashMap::default();

    let cell_set_overlay = chain_state.new_cell_set_overlay(&cell_set_diff, &outputs);
    let transcations = transcations();

    //Outpoint::null should be live
    let rtx0 = cell_set_overlay
        .resolve_transaction(&transcations[0])
        .unwrap();
    assert_eq!(rtx0.input_cells[0], LiveCell::Null);

    // cell A (0x8aa8799cd6ad56dd6929fd6ac05f5cab6a5339562297abb619839ab2da519f35, 0)
    // A is dead in old fork
    let rtx1 = chain_state.resolve_transaction(&transcations[1]);
    assert_eq!(rtx1.unwrap_err(), UnresolvableError::Dead);
    // assert_eq!(rtx1.input_cells[0], CellStatus::Dead);

    // A include in cell_set_diff old_inputs
    // A is live in cell_set_overlay
    let rtx1_overlay = cell_set_overlay
        .resolve_transaction(&transcations[1])
        .unwrap();
    assert_eq!(rtx1_overlay.input_cells[0], LiveCell::Null);

    // assert_eq!(
    //     rtx1_overlay.input_cells[0],
    //     CellStatus::Live(LiveCell::Null)
    // );

    // assert_eq!(
    //     rtx1_overlay.dep_cells[0],
    //     CellStatus::Unknown,
    // );
}
