use ckb_chain_spec::consensus::{Consensus, ConsensusBuilder};
use ckb_chain_spec::{
    ChainSpec, OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL, OUTPUT_INDEX_SECP256K1_DATA,
    build_genesis_type_id_script,
};
use ckb_dao_utils::genesis_dao_data;
use ckb_resource::Resource;
use ckb_types::{
    H256, U256,
    bytes::Bytes,
    core::{
        BlockBuilder, BlockNumber, Capacity, EpochNumberWithFraction, ScriptHashType,
        TransactionBuilder, TransactionView,
    },
    packed::{CellInput, CellOutput, OutPoint, Script},
    prelude::*,
    utilities::difficulty_to_compact,
};
use std::fs::File;
use std::io::Read;
use std::path::Path;

fn load_cell_from_path(path: &str) -> (CellOutput, Bytes, Script) {
    let mut file = File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap();
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
}

static SUCCESS_CELL: std::sync::LazyLock<(CellOutput, Bytes, Script)> =
    std::sync::LazyLock::new(|| load_cell_from_path("../../script/testdata/always_success"));

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
static LOAD_INPUT_DATA_HASH: std::sync::LazyLock<(CellOutput, Bytes, Script)> =
    std::sync::LazyLock::new(|| load_cell_from_path("vendor/load_input_data_hash"));

/// Script for loading input data hash from input data.
#[doc(hidden)]
pub fn load_input_data_hash_cell() -> &'static (CellOutput, Bytes, Script) {
    &LOAD_INPUT_DATA_HASH
}

// #include "ckb_syscalls.h"

// int main() {
//   int ret;
//   uint8_t data[1];
//   uint64_t len = 1;

//   ret = ckb_load_cell_data(data, &len, 0, 0, CKB_SOURCE_INPUT);

//   if (ret != CKB_SUCCESS) {
//     return ret;
//   }

//   return 0;
// }
static LOAD_INPUT_ONE_BYTE: std::sync::LazyLock<(CellOutput, Bytes, Script)> =
    std::sync::LazyLock::new(|| load_cell_from_path("vendor/load_input_one_byte"));

/// Script for loading one byte from input data.
#[doc(hidden)]
pub fn load_input_one_byte_cell() -> &'static (CellOutput, Bytes, Script) {
    &LOAD_INPUT_ONE_BYTE
}

/// Script for returning always success cell.
#[doc(hidden)]
pub fn always_success_cell() -> &'static (CellOutput, Bytes, Script) {
    &SUCCESS_CELL
}

static IS_EVEN_LIB: std::sync::LazyLock<(CellOutput, Bytes, Script)> =
    std::sync::LazyLock::new(|| load_cell_from_path("../../script/testdata/is_even.lib"));

#[doc(hidden)]
pub fn is_even_lib() -> &'static (CellOutput, Bytes, Script) {
    &IS_EVEN_LIB
}

// from script/testdata without ty_pause
static LOAD_IS_EVEN: std::sync::LazyLock<(CellOutput, Bytes, Script)> =
    std::sync::LazyLock::new(|| load_cell_from_path("vendor/load_is_even_with_snapshot"));

#[doc(hidden)]
pub fn load_is_even() -> &'static (CellOutput, Bytes, Script) {
    &LOAD_IS_EVEN
}

const GENESIS_TIMESTAMP: u64 = 1_557_310_743;

/// Build and return an always success consensus instance.
#[doc(hidden)]
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
        .timestamp(GENESIS_TIMESTAMP.pack())
        .compact_target(difficulty_to_compact(U256::from(1000u64)).pack())
        .dao(dao)
        .transaction(always_success_tx)
        .build();
    ConsensusBuilder::default()
        .genesis_block(genesis)
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .build()
}

/// Build and return an always success cellbase transaction view.
#[doc(hidden)]
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

/// Return chain spec by name, which could be:
///   - ckb_mainnet
///   - ckb_testnet
///   - ckb_staging
///   - ckb_dev
#[doc(hidden)]
fn load_spec_by_name(name: &str) -> ChainSpec {
    // remove "ckb_" prefix
    let base_name = &name[4..];
    let res = Resource::bundled(format!("specs/{base_name}.toml"));
    ChainSpec::load_from(&res).expect("load spec by name")
}

/// Return testnet consensus instance.
#[doc(hidden)]
pub fn ckb_testnet_consensus() -> Consensus {
    let name = "ckb_testnet";
    let spec = load_spec_by_name(name);
    spec.build_consensus().unwrap()
}

/// Return code hash of genesis type_id script which built with output index of SECP256K1/blake160 script.
#[doc(hidden)]
pub fn type_lock_script_code_hash() -> H256 {
    build_genesis_type_id_script(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL)
        .calc_script_hash()
        .unpack()
}

/// Return cell output and data in genesis block's cellbase transaction with index of SECP256K1/blake160 script,
/// the genesis block depends on the consensus parameter.
#[doc(hidden)]
pub fn secp256k1_blake160_sighash_cell(consensus: Consensus) -> (CellOutput, Bytes) {
    let genesis_block = consensus.genesis_block();
    let tx = genesis_block.transactions()[0].clone();
    let (cell_output, data) = tx
        .output_with_data(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL as usize)
        .unwrap();

    (cell_output, data)
}

/// Return cell output and data in genesis block's cellbase transaction with index of SECP256K1,
/// the genesis block depends on the consensus parameter.
#[doc(hidden)]
pub fn secp256k1_data_cell(consensus: Consensus) -> (CellOutput, Bytes) {
    let genesis_block = consensus.genesis_block();
    let tx = genesis_block.transactions()[0].clone();
    let (cell_output, data) = tx
        .output_with_data(OUTPUT_INDEX_SECP256K1_DATA as usize)
        .unwrap();

    (cell_output, data)
}
