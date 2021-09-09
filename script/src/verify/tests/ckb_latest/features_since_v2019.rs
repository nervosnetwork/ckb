use byteorder::{ByteOrder, LittleEndian};
use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_CYCLES, TYPE_ID_CODE_HASH};
use ckb_crypto::secp::Privkey;
use ckb_error::assert_error_eq;
use ckb_hash::{blake2b_256, new_blake2b};
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{capacity_bytes, cell::CellMetaBuilder, Capacity, ScriptHashType, TransactionBuilder},
    h256,
    packed::{CellDep, CellInput, CellOutputBuilder, OutPoint, Script},
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

    let rtx = random_2_in_2_out_rtx();

    let max_cycles = TWO_IN_TWO_OUT_CYCLES;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, max_cycles);
    assert!(result.is_ok());
    let cycle = result.unwrap();
    assert!(cycle <= TWO_IN_TWO_OUT_CYCLES);
    assert!(cycle >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
}
