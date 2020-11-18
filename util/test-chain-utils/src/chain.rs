use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_chain_spec::{
    build_genesis_type_id_script, ChainSpec, OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL,
    OUTPUT_INDEX_SECP256K1_DATA,
};
use ckb_dao_utils::genesis_dao_data;
use ckb_resource::Resource;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, Capacity, EpochNumberWithFraction, ScriptHashType,
        TransactionBuilder, TransactionView,
    },
    packed::{CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    utilities::difficulty_to_compact,
    H256, U256,
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
            .hash_type(ScriptHashType::Data.into())
            .code_hash(CellOutput::calc_data_hash(&data))
            .build();

        (cell, data, script)
    };
}

// #include "ckb_syscalls.h"

// #define HASH_SIZE 32

// int main() {
//   int ret;
//   uint64_t hash_len = HASH_SIZE;
//   unsigned char data_hash[HASH_SIZE];

//   ret = ckb_load_cell_by_field(data_hash, &hash_len, 0, 0, CKB_SOURCE_INPUT, CKB_CELL_FIELD_DATA_HASH);
//   if (ret != CKB_SUCCESS) {
//     return ret;
//   }

//   return 0;
// }
lazy_static! {
    static ref LOAD_INPUT_DATA_HASH: (CellOutput, Bytes, Script) = {
        let mut file =
            File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/load_input_data_hash"))
                .unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();
        let data: Bytes = buffer.into();

        let cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();

        let script = Script::new_builder()
            .hash_type(ScriptHashType::Data.into())
            .code_hash(CellOutput::calc_data_hash(&data))
            .build();

        (cell, data, script)
    };
}

/// TODO(doc): @chuijiaolianying
pub fn load_input_data_hash_cell() -> &'static (CellOutput, Bytes, Script) {
    &LOAD_INPUT_DATA_HASH
}

/// TODO(doc): @chuijiaolianying
pub fn always_success_cell() -> &'static (CellOutput, Bytes, Script) {
    &SUCCESS_CELL
}

/// TODO(doc): @chuijiaolianying
pub fn always_success_consensus() -> Consensus {
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(always_success_cell.clone())
        .output_data(always_success_cell_data.pack())
        .witness(always_success_script.clone().into_witness())
        .build();
    let dao = genesis_dao_data(vec![&always_success_tx]).unwrap();
    let genesis = BlockBuilder::default()
        .timestamp(unix_time_as_millis().pack())
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();
    ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build()
}

/// TODO(doc): @chuijiaolianying
pub fn always_success_cellbase(
    block_number: BlockNumber,
    reward: Capacity,
    consensus: &Consensus,
) -> TransactionView {
    let (_, _, always_success_script) = always_success_cell();
    let input = CellInput::new_cellbase_input(block_number);

    let witness = always_success_script.to_owned().into_witness();
    let data = Bytes::new();

    let builder = TransactionBuilder::default().input(input).witness(witness);

    if block_number <= consensus.finalization_delay_length() {
        builder.build()
    } else {
        let output = CellOutput::new_builder()
            .capacity(reward.pack())
            .lock(always_success_script.to_owned())
            .build();

        builder.output(output).output_data(data.pack()).build()
    }
}

fn load_spec_by_name(name: &str) -> ChainSpec {
    // remove "ckb_" prefix
    let base_name = &name[4..];
    let res = Resource::bundled(format!("specs/{}.toml", base_name));
    ChainSpec::load_from(&res).expect("load spec by name")
}

/// TODO(doc): @chuijiaolianying
pub fn ckb_testnet_consensus() -> Consensus {
    let name = "ckb_testnet";
    let spec = load_spec_by_name(name);
    spec.build_consensus().unwrap()
}

/// TODO(doc): @chuijiaolianying
pub fn type_lock_script_code_hash() -> H256 {
    build_genesis_type_id_script(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL)
        .calc_script_hash()
        .unpack()
}

/// TODO(doc): @chuijiaolianying
pub fn secp256k1_blake160_sighash_cell(consensus: Consensus) -> (CellOutput, Bytes) {
    let genesis_block = consensus.genesis_block();
    let tx = genesis_block.transactions()[0].clone();
    let (cell_output, data) = tx
        .output_with_data(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL as usize)
        .unwrap();

    (cell_output, data)
}

/// TODO(doc): @chuijiaolianying
pub fn secp256k1_data_cell(consensus: Consensus) -> (CellOutput, Bytes) {
    let genesis_block = consensus.genesis_block();
    let tx = genesis_block.transactions()[0].clone();
    let (cell_output, data) = tx
        .output_with_data(OUTPUT_INDEX_SECP256K1_DATA as usize)
        .unwrap();

    (cell_output, data)
}
