use crate::cell_set::{CellSet, CellSetDiff};
use crate::{
    shared::{Shared, SharedBuilder},
    store::ChainKVStore,
};
use ckb_core::transaction::Transaction;
use ckb_db::memorydb::MemoryKeyValueDB;
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

    let out_point = transcations[1].inputs()[0].previous_output.clone();

    // cell A (0x8aa8799cd6ad56dd6929fd6ac05f5cab6a5339562297abb619839ab2da519f35, 0)
    // A is dead in old fork
    assert_eq!(
        cell_set()
            .get(&out_point.tx_hash)
            .map(|mate| mate.is_dead(out_point.index as usize)),
        Some(true)
    );

    // A include in cell_set_diff old_inputs
    // A is live in cell_set_overlay
    assert_eq!(
        cell_set_overlay
            .overlay
            .get(&out_point.tx_hash)
            .map(|mate| mate.is_dead(out_point.index as usize)),
        Some(false)
    );
}
