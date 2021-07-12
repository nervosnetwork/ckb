use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_BYTES, TWO_IN_TWO_OUT_CYCLES, TYPE_ID_CODE_HASH};
use ckb_crypto::secp::{Generator, Privkey};
use ckb_error::assert_error_eq;
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_test_chain_utils::{
    always_success_cell, ckb_testnet_consensus, secp256k1_blake160_sighash_cell,
    secp256k1_data_cell, type_lock_script_code_hash,
};
use ckb_types::{
    core::{
        capacity_bytes, cell::CellMetaBuilder, Capacity, DepType, ScriptHashType,
        TransactionBuilder,
    },
    h256,
    packed::{CellDep, CellInput, CellOutputBuilder, OutPoint, Script, WitnessArgs},
    H256,
};
use std::io::Read;

use super::SCRIPT_VERSION;
use crate::{
    type_id::TYPE_ID_CYCLES,
    verify::{tests::utils::*, *},
};

#[test]
fn check_always_success_hash() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(always_success_script.clone())
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .transaction_info(default_transaction_info())
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![always_success_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
}

#[test]
fn check_signature() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let code_hash = blake2b_256(&buffer);
    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder()
        .out_point(dep_out_point.clone())
        .build();
    let data = Bytes::from(buffer);
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .build();
    let dep_cell = CellMetaBuilder::from_cell_output(output, data)
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point)
        .build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(code_hash.pack())
        .hash_type(ScriptHashType::Data.into())
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .cell_dep(cell_dep)
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());

    // Not enough cycles
    let max_cycles = ALWAYS_SUCCESS_SCRIPT_CYCLE - 1;
    let result = verifier.verify(script_version, &rtx, max_cycles);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::ExceededMaximumCycles(max_cycles).input_lock_script(0),
    );

    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
}

#[test]
fn check_signature_referenced_via_type_hash() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder()
        .out_point(dep_out_point.clone())
        .build();
    let data = Bytes::from(buffer);
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .type_(
            Some(
                Script::new_builder()
                    .code_hash(h256!("0x123456abcd90").pack())
                    .hash_type(ScriptHashType::Data.into())
                    .build(),
            )
            .pack(),
        )
        .build();
    let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
    let dep_cell = CellMetaBuilder::from_cell_output(output, data)
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point)
        .build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(type_hash)
        .hash_type(ScriptHashType::Type.into())
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .cell_dep(cell_dep)
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
}

#[test]
fn check_signature_referenced_via_type_hash_failure_with_multiple_matches() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();
    let data = Bytes::from(buffer);

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder()
        .out_point(dep_out_point.clone())
        .build();
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .type_(
            Some(
                Script::new_builder()
                    .code_hash(h256!("0x123456abcd90").pack())
                    .hash_type(ScriptHashType::Data.into())
                    .build(),
            )
            .pack(),
        )
        .build();
    let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
    let dep_cell = CellMetaBuilder::from_cell_output(output, data.clone())
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point)
        .build();

    let dep_out_point2 = OutPoint::new(h256!("0x1234").pack(), 8);
    let cell_dep2 = CellDep::new_builder()
        .out_point(dep_out_point2.clone())
        .build();
    let output2 = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .type_(
            Some(
                Script::new_builder()
                    .code_hash(h256!("0x123456abcd90").pack())
                    .hash_type(ScriptHashType::Data.into())
                    .build(),
            )
            .pack(),
        )
        .build();
    let dep_cell2 = CellMetaBuilder::from_cell_output(output2, data)
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point2)
        .build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(type_hash)
        .hash_type(ScriptHashType::Type.into())
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .cell_dep(cell_dep)
        .cell_dep(cell_dep2)
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell, dep_cell2],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::MultipleMatches.input_lock_script(0),
    );
}

#[test]
fn check_invalid_signature() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_failure();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);

    // This line makes the verification invalid
    args.extend(&b"extrastring".to_vec());
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let code_hash = blake2b_256(&buffer);
    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();
    let data = Bytes::from(buffer);
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .build();
    let dep_cell = CellMetaBuilder::from_cell_output(output, data)
        .transaction_info(default_transaction_info())
        .build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(code_hash.pack())
        .hash_type(ScriptHashType::Data.into())
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .cell_dep(cell_dep)
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script.clone())
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::validation_failure(&script, -1).input_lock_script(0),
    );
}

#[test]
fn check_invalid_dep_reference() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();
    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(blake2b_256(&buffer).pack())
        .hash_type(ScriptHashType::Data.into())
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .cell_dep(cell_dep)
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::InvalidCodeHash.input_lock_script(0),
    );
}

#[test]
fn check_output_contract() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();
    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let input = CellInput::new(OutPoint::null(), 0);
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(always_success_script.clone())
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .transaction_info(default_transaction_info())
    .build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(blake2b_256(&buffer).pack())
        .hash_type(ScriptHashType::Data.into())
        .build();
    let output_data = Bytes::default();
    let output = CellOutputBuilder::default()
        .lock(
            Script::new_builder()
                .hash_type(ScriptHashType::Data.into())
                .build(),
        )
        .type_(Some(script).pack())
        .build();

    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder()
        .out_point(dep_out_point.clone())
        .build();
    let dep_cell = {
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
            .build()
    };

    let transaction = TransactionBuilder::default()
        .input(input)
        .output(output)
        .output_data(output_data.pack())
        .cell_dep(cell_dep)
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell, always_success_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
}

#[test]
fn check_invalid_output_contract() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_failure();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);
    // This line makes the verification invalid
    args.extend(&b"extrastring".to_vec());
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let input = CellInput::new(OutPoint::null(), 0);
    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(always_success_script.clone())
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.to_owned(),
        always_success_cell_data.to_owned(),
    )
    .transaction_info(default_transaction_info())
    .build();

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(blake2b_256(&buffer).pack())
        .hash_type(ScriptHashType::Data.into())
        .build();
    let output = CellOutputBuilder::default()
        .type_(Some(script.clone()).pack())
        .build();

    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();
    let dep_cell = {
        let dep_cell_data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(dep_cell_data.len()).unwrap().pack())
            .build();
        CellMetaBuilder::from_cell_output(output, dep_cell_data)
            .transaction_info(default_transaction_info())
            .build()
    };

    let transaction = TransactionBuilder::default()
        .input(input)
        .output(output)
        .output_data(Bytes::new().pack())
        .cell_dep(cell_dep)
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell, always_success_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::validation_failure(&script, -1).output_type_script(0),
    );
}

#[test]
fn check_same_lock_and_type_script_are_executed_twice() {
    let script_version = SCRIPT_VERSION;

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();

    let privkey = Privkey::from_slice(&[1; 32][..]);
    let pubkey = privkey.pubkey().unwrap();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let script = Script::new_builder()
        .args(Bytes::from(args).pack())
        .code_hash(blake2b_256(&buffer).pack())
        .hash_type(ScriptHashType::Data.into())
        .build();

    let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
    let cell_dep = CellDep::new_builder()
        .out_point(dep_out_point.clone())
        .build();
    let data = Bytes::from(buffer);
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap().pack())
        .build();
    let dep_cell = CellMetaBuilder::from_cell_output(output, data)
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point)
        .build();

    let transaction = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .cell_dep(cell_dep)
        .build();

    // The lock and type scripts here are both executed.
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script.clone())
        .type_(Some(script).pack())
        .build();
    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    // Cycles can tell that both lock and type scripts are executed
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.ok(), Some(ALWAYS_SUCCESS_SCRIPT_CYCLE * 2));
}

#[test]
fn check_type_id_one_in_one_out() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let type_id_script = Script::new_builder()
        .args(Bytes::from(h256!("0x1111").as_ref()).pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script).pack())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let max_cycles = TYPE_ID_CYCLES * 2;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, max_cycles);
    assert!(
        result.is_ok(),
        "expect ok, but got {:?}",
        result.unwrap_err()
    );
}

#[test]
fn check_type_id_one_in_one_out_not_enough_cycles() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let type_id_script = Script::new_builder()
        .args(Bytes::from(h256!("0x1111").as_ref()).pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script).pack())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let max_cycles = TYPE_ID_CYCLES - 1;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    // two groups need exec, so cycles not TYPE_ID_CYCLES - 1
    let result = verifier.verify(script_version, &rtx, max_cycles);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::ExceededMaximumCycles(TYPE_ID_CYCLES - ALWAYS_SUCCESS_SCRIPT_CYCLE - 1)
            .input_type_script(0),
    );
}

#[test]
fn check_type_id_creation() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000).pack())
        .lock(always_success_script.clone())
        .build();

    let input_hash = {
        let mut blake2b = new_blake2b();
        blake2b.update(input.as_slice());
        blake2b.update(&0u64.to_le_bytes());
        let mut ret = [0; 32];
        blake2b.finalize(&mut ret);
        Bytes::from(ret.to_vec())
    };

    let type_id_script = Script::new_builder()
        .args(input_hash.pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script).pack())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
}

#[test]
fn check_type_id_termination() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let type_id_script = Script::new_builder()
        .args(Bytes::from(h256!("0x1111").as_ref()).pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script).pack())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
}

#[test]
fn check_type_id_invalid_creation() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000).pack())
        .lock(always_success_script.clone())
        .build();

    let input_hash = {
        let mut blake2b = new_blake2b();
        blake2b.update(&input.previous_output().tx_hash().as_bytes());
        let mut buf = [0; 4];
        LittleEndian::write_u32(&mut buf, input.previous_output().index().unpack());
        blake2b.update(&buf[..]);
        let mut buf = [0; 8];
        LittleEndian::write_u64(&mut buf, 0);
        blake2b.update(&buf[..]);
        blake2b.update(b"unnecessary data");
        let mut ret = [0; 32];
        blake2b.finalize(&mut ret);
        Bytes::from(ret.to_vec())
    };

    let type_id_script = Script::new_builder()
        .args(input_hash.pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::validation_failure(&type_id_script, -3).output_type_script(0),
    );
}

#[test]
fn check_type_id_invalid_creation_length() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(1000).pack())
        .lock(always_success_script.clone())
        .build();

    let input_hash = {
        let mut blake2b = new_blake2b();
        blake2b.update(&input.previous_output().tx_hash().as_bytes());
        let mut buf = [0; 4];
        LittleEndian::write_u32(&mut buf, input.previous_output().index().unpack());
        blake2b.update(&buf[..]);
        let mut buf = [0; 8];
        LittleEndian::write_u64(&mut buf, 0);
        blake2b.update(&buf[..]);
        let mut ret = [0; 32];
        blake2b.finalize(&mut ret);

        let mut buf = vec![];
        buf.extend_from_slice(&ret[..]);
        buf.extend_from_slice(b"abc");
        Bytes::from(buf)
    };

    let type_id_script = Script::new_builder()
        .args(input_hash.pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::validation_failure(&type_id_script, -1).output_type_script(0),
    );
}

#[test]
fn check_type_id_one_in_two_out() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_cell_data, always_success_script) =
        always_success_cell();
    let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

    let type_id_script = Script::new_builder()
        .args(Bytes::from(h256!("0x1111").as_ref()).pack())
        .code_hash(TYPE_ID_CODE_HASH.pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
    let input_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(2000).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();

    let output_cell = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();
    let output_cell2 = CellOutputBuilder::default()
        .capacity(capacity_bytes!(990).pack())
        .lock(always_success_script.clone())
        .type_(Some(type_id_script.clone()).pack())
        .build();

    let transaction = TransactionBuilder::default()
        .input(input.clone())
        .output(output_cell)
        .output(output_cell2)
        .cell_dep(
            CellDep::new_builder()
                .out_point(always_success_out_point.clone())
                .build(),
        )
        .build();

    let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
        .out_point(input.previous_output())
        .build();
    let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
        always_success_cell.clone(),
        always_success_cell_data.to_owned(),
    )
    .out_point(always_success_out_point)
    .build();

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![resolved_always_success_cell],
        resolved_inputs: vec![resolved_input_cell],
        resolved_dep_groups: vec![],
    };

    let max_cycles = TYPE_ID_CYCLES * 2;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, max_cycles);
    assert_error_eq!(
        result.unwrap_err(),
        ScriptError::validation_failure(&type_id_script, -2).input_type_script(0),
    );
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx() {
    let script_version = SCRIPT_VERSION;

    let consensus = ckb_testnet_consensus();
    let dep_group_tx_hash = consensus.genesis_block().transactions()[1].hash();
    let secp_out_point = OutPoint::new(dep_group_tx_hash, 0);

    let cell_dep = CellDep::new_builder()
        .out_point(secp_out_point)
        .dep_type(DepType::DepGroup.into())
        .build();

    let input1 = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 0), 0);
    let input2 = CellInput::new(OutPoint::new(h256!("0x1111").pack(), 0), 0);

    let mut generator = Generator::non_crypto_safe_prng(42);
    let privkey = generator.gen_privkey();
    let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
    let lock_arg = Bytes::from((&blake2b_256(&pubkey_data)[0..20]).to_owned());
    let privkey2 = generator.gen_privkey();
    let pubkey_data2 = privkey2.pubkey().expect("Get pubkey failed").serialize();
    let lock_arg2 = Bytes::from((&blake2b_256(&pubkey_data2)[0..20]).to_owned());

    let lock = Script::new_builder()
        .args(lock_arg.pack())
        .code_hash(type_lock_script_code_hash().pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let lock2 = Script::new_builder()
        .args(lock_arg2.pack())
        .code_hash(type_lock_script_code_hash().pack())
        .hash_type(ScriptHashType::Type.into())
        .build();

    let output1 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock.clone())
        .build();
    let output2 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock2.clone())
        .build();
    let tx = TransactionBuilder::default()
        .cell_dep(cell_dep)
        .input(input1.clone())
        .input(input2.clone())
        .output(output1)
        .output(output2)
        .output_data(Default::default())
        .output_data(Default::default())
        .build();

    let tx_hash: H256 = tx.hash().unpack();
    // sign input1
    let witness = {
        WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build()
    };
    let witness_len: u64 = witness.as_bytes().len() as u64;
    let mut hasher = new_blake2b();
    hasher.update(tx_hash.as_bytes());
    hasher.update(&witness_len.to_le_bytes());
    hasher.update(&witness.as_bytes());
    let message = {
        let mut buf = [0u8; 32];
        hasher.finalize(&mut buf);
        H256::from(buf)
    };
    let sig = privkey.sign_recoverable(&message).expect("sign");
    let witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(sig.serialize())).pack())
        .build();
    // sign input2
    let witness2 = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let witness2_len: u64 = witness2.as_bytes().len() as u64;
    let mut hasher = new_blake2b();
    hasher.update(tx_hash.as_bytes());
    hasher.update(&witness2_len.to_le_bytes());
    hasher.update(&witness2.as_bytes());
    let message2 = {
        let mut buf = [0u8; 32];
        hasher.finalize(&mut buf);
        H256::from(buf)
    };
    let sig2 = privkey2.sign_recoverable(&message2).expect("sign");
    let witness2 = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(sig2.serialize())).pack())
        .build();
    let tx = tx
        .as_advanced_builder()
        .witness(witness.as_bytes().pack())
        .witness(witness2.as_bytes().pack())
        .build();

    let serialized_size = tx.data().as_slice().len() as u64;

    assert_eq!(
        serialized_size, TWO_IN_TWO_OUT_BYTES,
        "2 in 2 out tx serialized size changed, PLEASE UPDATE consensus"
    );

    let (secp256k1_blake160_cell, secp256k1_blake160_cell_data) =
        secp256k1_blake160_sighash_cell(consensus.clone());

    let (secp256k1_data_cell, secp256k1_data_cell_data) = secp256k1_data_cell(consensus);

    let input_cell1 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock)
        .build();

    let resolved_input_cell1 = CellMetaBuilder::from_cell_output(input_cell1, Default::default())
        .out_point(input1.previous_output())
        .build();

    let input_cell2 = CellOutput::new_builder()
        .capacity(capacity_bytes!(100).pack())
        .lock(lock2)
        .build();

    let resolved_input_cell2 = CellMetaBuilder::from_cell_output(input_cell2, Default::default())
        .out_point(input2.previous_output())
        .build();

    let resolved_secp256k1_blake160_cell =
        CellMetaBuilder::from_cell_output(secp256k1_blake160_cell, secp256k1_blake160_cell_data)
            .build();

    let resolved_secp_data_cell =
        CellMetaBuilder::from_cell_output(secp256k1_data_cell, secp256k1_data_cell_data).build();

    let rtx = ResolvedTransaction {
        transaction: tx,
        resolved_cell_deps: vec![resolved_secp256k1_blake160_cell, resolved_secp_data_cell],
        resolved_inputs: vec![resolved_input_cell1, resolved_input_cell2],
        resolved_dep_groups: vec![],
    };

    let max_cycles = TWO_IN_TWO_OUT_CYCLES;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, max_cycles);
    assert!(result.is_ok());
    let cycle = result.unwrap();
    assert!(cycle <= TWO_IN_TWO_OUT_CYCLES);
    assert!(cycle >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
}
