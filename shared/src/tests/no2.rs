use crate::cell_set::CellSet;
use crate::{
    shared::{Shared, SharedBuilder},
    store::ChainKVStore,
};
use ckb_core::block::Block;
use ckb_core::cell::resolve_transaction;
use ckb_db::memorydb::MemoryKeyValueDB;
use fnv::FnvHashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

fn block() -> Block {
    let file =
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("./src/tests/data/no2/block.json"))
            .unwrap();
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).unwrap()
}

fn cell_set() -> CellSet {
    let file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("./src/tests/data/no2/cell_set.json"),
    )
    .unwrap();
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).unwrap()
}
fn new_shared() -> Shared<ChainKVStore<MemoryKeyValueDB>> {
    SharedBuilder::<MemoryKeyValueDB>::new().build()
}

#[test]
fn case_no2() {
    let block = block();
    let shared = new_shared();
    let mut chain_state = shared.chain_state().lock();
    chain_state.cell_set = cell_set();
    let mut seen_inputs = FnvHashSet::default();

    // dep status
    assert!(block
        .transactions()
        .iter()
        .map(|tx| resolve_transaction(tx, &mut seen_inputs, &*chain_state).dep_cells)
        .flatten()
        .all(|status| status.is_live()));
}
