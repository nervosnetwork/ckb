use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::script::{Script, ScriptHashType};
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder};
use ckb_core::Capacity;
use ckb_core::{BlockNumber, Bytes};
use ckb_dao_utils::genesis_dao_data;
use faketime::unix_time_as_millis;
use lazy_static::lazy_static;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
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

        let cell = CellOutput::new(
            Capacity::bytes(data.len()).unwrap(),
            CellOutput::calculate_data_hash(&data),
            Script::default(),
            None,
        );

        let script = Script::new(vec![], cell.data_hash().to_owned(), ScriptHashType::Data);

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
        .output_data(always_success_cell_data.to_owned())
        .witness(always_success_script.clone().into_witness())
        .build();
    let dao = genesis_dao_data(&always_success_tx).unwrap();
    let genesis = BlockBuilder::from_header_builder(
        HeaderBuilder::default()
            .timestamp(unix_time_as_millis())
            .difficulty(U256::from(1000u64))
            .dao(dao),
    )
    .transaction(always_success_tx)
    .build();
    Consensus::default()
        .set_genesis_block(genesis)
        .set_cellbase_maturity(0)
}

pub fn always_success_cellbase(block_number: BlockNumber, reward: Capacity) -> Transaction {
    let (_, _, always_success_script) = always_success_cell();
    TransactionBuilder::default()
        .input(CellInput::new_cellbase_input(block_number))
        .output(CellOutput::new(
            reward,
            H256::zero(),
            always_success_script.to_owned(),
            None,
        ))
        .output_data(Bytes::new())
        .witness(always_success_script.to_owned().into_witness())
        .build()
}
