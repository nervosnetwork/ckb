use crate::cell_set::{CellSet, CellSetDiff};
use crate::{
    shared::{Shared, SharedBuilder},
    store::ChainKVStore,
};
use ckb_core::cell::{resolve_transaction, CellStatus, LiveCell};
use ckb_core::transaction::Transaction;
use ckb_db::memorydb::MemoryKeyValueDB;
use fnv::FnvHashSet;
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
    let cell_set_overlay = chain_state.new_cell_set_overlay(&cell_set_diff);

    let transcations = transcations();
    // let cell_provider = OverlayCellProvider::new(&overlay, &cell_set);

    let mut seen_inputs = FnvHashSet::default();

    //Outpoint::null should be live
    let rtx0 = resolve_transaction(&transcations[0], &mut seen_inputs, &cell_set_overlay);
    assert_eq!(rtx0.input_cells[0], CellStatus::Live(LiveCell::Null));

    assert_eq!(
        cell_set_overlay
            .overlay
            .is_dead_cell(&transcations[1].inputs()[0].previous_output),
        Some(false)
    );
}
