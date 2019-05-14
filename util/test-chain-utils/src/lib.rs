use ckb_core::script::Script;
use ckb_core::transaction::CellOutput;
use ckb_core::Capacity;
use hash::blake2b_256;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub fn create_always_success_cell() -> (CellOutput, Script) {
    let mut file = File::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../script/testdata/always_success"),
    )
    .unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let cell = CellOutput::new(
        Capacity::bytes(buffer.len()).unwrap(),
        buffer.into(),
        Script::default(),
        None,
    );

    let script = Script::new(vec![], blake2b_256(&cell.data).into());

    (cell, script)
}
