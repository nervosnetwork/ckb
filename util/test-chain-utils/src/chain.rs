use ckb_chain_spec::consensus::Consensus;
use ckb_dao_utils::genesis_dao_data;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, Capacity, ScriptHashType, TransactionBuilder, TransactionView,
    },
    packed::{CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    U256,
};
use faketime::unix_time_as_millis;
use lazy_static::lazy_static;
use std::fs::File;
use std::io::Read;
use std::path::Path;

lazy_static! {
    static ref SUCCESS_CELL: (CellOutput, Bytes, Script) = {
        let mut file = File::open(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../script/testdata/always_success"),
        )
        .unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();
        let data: Bytes = buffer.into();

        let cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();

        let script = Script::new_builder()
            .hash_type(ScriptHashType::Data.pack())
            .code_hash(CellOutput::calc_data_hash(&data).pack())
            .build();

        (cell, data, script)
    };
}

pub fn always_success_cell() -> &'static (CellOutput, Bytes, Script) {
    &SUCCESS_CELL
}

pub fn always_success_consensus() -> Consensus {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.pack())
        .witness(always_success_script.clone().into_witness())
        .build();
    let dao = genesis_dao_data(&always_success_tx).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(unix_time_as_millis().pack())
        .difficulty(U256::from(1000u64).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();
    Consensus::default()
        .set_genesis_block(genesis)
        .set_cellbase_maturity(0)
}

pub fn always_success_cellbase(block_number: BlockNumber, reward: Capacity) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let input = CellInput::new_cellbase_input(block_number);
    let output = CellOutput::new_builder()
        .capacity(reward.pack())
        .lock(always_success_script.to_owned())
        .build();
    let witness = always_success_script.to_owned().into_witness();
    let data = Bytes::new();
    TransactionBuilder::default()
        .input(input)
        .output(output)
        .witness(witness)
        .output_data(data.pack())
        .build()
}
