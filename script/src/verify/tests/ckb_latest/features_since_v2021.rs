use super::SCRIPT_VERSION;
use crate::syscalls::SOURCE_GROUP_FLAG;
use crate::{
    ScriptError,
    verify::{tests::utils::*, *},
};
use ckb_error::assert_error_eq;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{Capacity, ScriptHashType, TransactionBuilder, capacity_bytes, cell::CellMetaBuilder},
    h256,
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint, Script},
    prelude::*,
};
use ckb_vm::Error as VmError;
use std::io::Read;

#[test]
fn test_hint_instructions() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_data_hash) =
        load_cell_from_path("testdata/cadd_hint_lock");

    let always_success_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(always_success_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(always_success_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![always_success_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1,);
    if script_version < ScriptVersion::V1 {
        let vm_error = VmError::InvalidInstruction {
            pc: 65_656,
            instruction: 36_906,
        };
        let script_error = ScriptError::VMInternalError(vm_error);
        assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
    } else {
        assert_eq!(result.ok(), Some(540));
    }
}

#[test]
fn test_b_extension() {
    let script_version = SCRIPT_VERSION;

    let args: packed::Bytes = {
        let num0 = 0x0102030405060708u64; // a random value
        let num1 = u64::from(num0.count_ones());

        let mut vec = Vec::with_capacity(8 * 2);
        vec.extend_from_slice(&num0.to_le_bytes());
        vec.extend_from_slice(&num1.to_le_bytes());
        vec.into()
    };

    let (cpop_lock_cell, cpop_lock_data_hash) = load_cell_from_path("testdata/cpop_lock");

    let cpop_lock_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(cpop_lock_data_hash)
        .args(args)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(cpop_lock_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![cpop_lock_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1,);
    match script_version {
        ScriptVersion::V0 => {
            let vm_error = VmError::InvalidInstruction {
                pc: 65866,
                instruction: 0x60291913,
            };
            let script_error = ScriptError::VMInternalError(vm_error);
            assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
        }
        ScriptVersion::V1 => {
            assert_eq!(result.ok(), Some(1876));
        }
        ScriptVersion::V2 => {
            assert_eq!(result.ok(), Some(1875));
        }
    }
}

#[test]
fn test_cycles_difference() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_data_hash) =
        load_cell_from_path("testdata/mop_adc_lock");

    let always_success_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(always_success_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(always_success_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![always_success_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_ok());
    let cycles_actual = result.unwrap();
    let cycles_expected = if script_version >= ScriptVersion::V1 {
        686
    } else {
        696
    };
    assert_eq!(cycles_actual, cycles_expected);
}

#[test]
fn check_current_cycles() {
    let script_version = SCRIPT_VERSION;

    let (current_cycles_cell, current_cycles_data_hash) =
        load_cell_from_path("testdata/current_cycles");

    let current_cycles_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(current_cycles_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(current_cycles_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![current_cycles_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
}

#[test]
fn check_vm_version() {
    let script_version = SCRIPT_VERSION;

    let (vm_version_cell, vm_version_data_hash) = load_cell_from_path("testdata/vm_version");

    let vm_version_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(vm_version_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(vm_version_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![vm_version_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version == ScriptVersion::V1);
}

#[test]
fn check_exec_from_cell_data() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_from_cell_data");
    let (exec_callee_cell, _exec_callee_data_hash) = load_cell_from_path("testdata/exec_callee");

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
}

#[test]
fn check_exec_from_witness() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_from_witness");
    let (exec_callee_cell, _exec_caller_data_hash) = load_cell_from_path("testdata/exec_callee");

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
    let transaction = TransactionBuilder::default()
        .input(input)
        .set_witnesses(vec![exec_callee_cell_data.into()])
        .build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![exec_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
    if script_version == ScriptVersion::V1 {
        assert_eq!(result.ok(), Some(1200));
    } else if script_version == ScriptVersion::V2 {
        assert_eq!(result.ok(), Some(76198));
    }
}

#[test]
fn check_exec_wrong_callee_format() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_from_cell_data");
    let (exec_callee_cell, _exec_caller_data_hash) =
        load_cell_from_slice(&[0x00, 0x01, 0x02, 0x03]);

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert!(result.is_err());
}

#[tokio::test]
async fn async_check_exec_wrong_callee_format() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_from_cell_data");
    let (exec_callee_cell, _exec_caller_data_hash) =
        load_cell_from_slice(&[0x00, 0x01, 0x02, 0x03]);

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier
        .verify_without_limit_async(script_version, &rtx)
        .await;
    assert!(result.is_err());
}

#[test]
fn check_exec_big_offset_length() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_big_offset_length");
    let (exec_callee_cell, _exec_caller_data_hash) =
        load_cell_from_slice(&[0x00, 0x01, 0x02, 0x03]);

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    match script_version {
        ScriptVersion::V0 => {}
        ScriptVersion::V1 => {
            assert!(result.unwrap_err().to_string().contains("error code 3"));
        }
        _ => {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("VM Internal Error: ElfParseError")
            );
        }
    }
}

#[test]
fn load_code_into_global() {
    let script_version = SCRIPT_VERSION;

    let (dyn_lib_cell, dyn_lib_data_hash) = load_cell_from_path("testdata/is_even.lib");

    let rtx = {
        let args: packed::Bytes = {
            let number = 0x01u64; // a random odd value

            let data_hash = dyn_lib_data_hash.raw_data();
            let mut vec = Vec::with_capacity(8 + data_hash.len());
            vec.extend_from_slice(&number.to_le_bytes());
            vec.extend_from_slice(&data_hash);
            vec.into()
        };

        let (dyn_lock_cell, dyn_lock_data_hash) =
            load_cell_from_path("testdata/load_is_even_into_global");

        let dyn_lock_script = Script::new_builder()
            .hash_type(script_version.data_hash_type())
            .code_hash(dyn_lock_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(dyn_lock_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input).build();
        let dummy_cell = create_dummy_cell(output);

        ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dyn_lock_cell, dyn_lib_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        }
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1,);
    if script_version < ScriptVersion::V0 {
        let vm_error = VmError::MemWriteOnFreezedPage;
        let script_error = ScriptError::VMInternalError(vm_error);
        assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
    } else if script_version == ScriptVersion::V1 {
        assert_eq!(result.ok(), Some(10529));
    } else if script_version == ScriptVersion::V2 {
        assert_eq!(result.ok(), Some(10525));
    }
}

#[derive(Clone, Copy)]
enum ExecFrom {
    TxInputWitness,
    GroupInputWitness,
    TxOutputWitness,
    GroupOutputWitness,
    TxCellDep,
    TxInputCell,
    TxOutputCell,
    GroupInputCell,
    GroupOutputCell,
    OutOfBound(u64, u64, u64, u64),
    Slice(u64),
    OutOfSlice(u64),
}

// Args:
// - flag: Control if loading code to update the number before and after exec.
// - recursion: Recursively invoke exec how many times.
// - number: A input number.
// - expected: The expected number after all invocations.
// - result: The expected result of the script for `>= ScriptVersion::V1`.
// See "exec_configurable_callee.c" for more details.
fn test_exec(
    flag: u8,
    recursion: u64,
    number: u64,
    expected: u64,
    exec_from: ExecFrom,
    expected_result: Result<(), String>,
) {
    let script_version = SCRIPT_VERSION;

    let (dyn_lib_cell, dyn_lib_data_hash) = load_cell_from_path("testdata/mul2.lib");

    let args: packed::Bytes = {
        // The args for invoke exec.
        let (index, source, place, bounds): (u64, u64, u64, u64) = match exec_from {
            ExecFrom::TxInputWitness => (0, 1, 1, 0),
            ExecFrom::TxOutputWitness => (0, 2, 1, 0),
            ExecFrom::GroupInputWitness => (0, SOURCE_GROUP_FLAG | 1, 1, 0),
            ExecFrom::GroupOutputWitness => (0, SOURCE_GROUP_FLAG | 2, 1, 0),
            ExecFrom::TxCellDep => (1, 3, 0, 0),
            ExecFrom::TxInputCell => (1, 1, 0, 0),
            ExecFrom::TxOutputCell => (0, 2, 0, 0),
            ExecFrom::GroupInputCell => (0, SOURCE_GROUP_FLAG | 1, 0, 0),
            ExecFrom::GroupOutputCell => (0, SOURCE_GROUP_FLAG | 2, 0, 0),
            ExecFrom::OutOfBound(index, source, place, bounds) => (index, source, place, bounds),
            ExecFrom::Slice(bounds) => (0, 1, 1, bounds),
            ExecFrom::OutOfSlice(bounds) => (0, 1, 1, bounds),
        };
        // Load data as code at last exec.
        let data_hash = dyn_lib_data_hash.raw_data();

        let mut vec = Vec::new();
        vec.extend_from_slice(&flag.to_le_bytes());
        vec.extend_from_slice(&recursion.to_le_bytes());
        vec.extend_from_slice(&number.to_le_bytes());
        vec.extend_from_slice(&expected.to_le_bytes());
        vec.extend_from_slice(&index.to_le_bytes());
        vec.extend_from_slice(&source.to_le_bytes());
        vec.extend_from_slice(&place.to_le_bytes());
        vec.extend_from_slice(&bounds.to_le_bytes());
        vec.extend_from_slice(&data_hash);
        vec.into()
    };

    let rtx = {
        let (exec_caller_cell, exec_caller_data_hash) =
            load_cell_from_path("testdata/exec_configurable_caller");
        let (exec_callee_cell, _exec_callee_data_hash) =
            load_cell_from_path("testdata/exec_configurable_callee");

        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();

        let exec_caller_script = Script::new_builder()
            .hash_type(script_version.data_hash_type())
            .code_hash(exec_caller_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100))
            .lock(exec_caller_script.clone())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);
        let (transaction, resolved_inputs) = match exec_from {
            ExecFrom::TxOutputWitness
            | ExecFrom::TxInputWitness
            | ExecFrom::GroupInputWitness
            | ExecFrom::OutOfSlice(..) => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let tx = TransactionBuilder::default()
                    .input(input)
                    .set_witnesses(vec![exec_callee_cell_data.into()])
                    .build();
                (tx, vec![create_dummy_cell(output)])
            }
            ExecFrom::Slice(bounds) => {
                let offset = (bounds >> 32) as usize;
                let mut data = vec![0; offset];
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                data.extend(exec_callee_cell_data);

                let tx = TransactionBuilder::default()
                    .input(input)
                    .set_witnesses(vec![data.into()])
                    .build();
                (tx, vec![create_dummy_cell(output)])
            }
            ExecFrom::TxCellDep => {
                let tx = TransactionBuilder::default().input(input).build();
                (tx, vec![create_dummy_cell(output)])
            }
            ExecFrom::GroupOutputWitness => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .type_(Some(exec_caller_script))
                    .build();
                let tx = TransactionBuilder::default()
                    .output(output)
                    .set_witnesses(vec![exec_callee_cell_data.into()])
                    .build();
                (tx, vec![])
            }
            ExecFrom::TxInputCell => {
                let callee_output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000))
                    .lock(always_success_script.clone())
                    .build();
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let callee_cell =
                    CellMetaBuilder::from_cell_output(callee_output, exec_callee_cell_data.clone())
                        .build();
                let tx = TransactionBuilder::default().input(input).build();

                (tx, vec![create_dummy_cell(output), callee_cell])
            }
            ExecFrom::GroupInputCell => {
                let caller_output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .lock(exec_caller_script)
                    .type_(Some(always_success_script.clone()))
                    .build();
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let caller_cell =
                    CellMetaBuilder::from_cell_output(caller_output, exec_callee_cell_data.clone())
                        .build();
                let tx = TransactionBuilder::default().input(input).build();

                (tx, vec![caller_cell])
            }
            ExecFrom::TxOutputCell => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let callee_output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .lock(always_success_script.clone())
                    .build();
                let tx = TransactionBuilder::default()
                    .input(input)
                    .output(callee_output)
                    .output_data(exec_callee_cell_data)
                    .build();
                (tx, vec![create_dummy_cell(output)])
            }
            ExecFrom::GroupOutputCell => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let callee_output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100))
                    .type_(Some(exec_caller_script))
                    .build();
                let tx = TransactionBuilder::default()
                    .output(callee_output)
                    .output_data(exec_callee_cell_data)
                    .build();
                (tx, vec![])
            }
            ExecFrom::OutOfBound(..) => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let tx = TransactionBuilder::default()
                    .set_witnesses(vec![exec_callee_cell_data.into()])
                    .build();
                (tx, vec![create_dummy_cell(output)])
            }
        };

        let always_success_out_point = OutPoint::new(h256!("0x11").into(), 0);
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![
                exec_caller_cell,
                exec_callee_cell,
                dyn_lib_cell,
                resolved_always_success_cell,
            ],
            resolved_inputs,
            resolved_dep_groups: vec![],
        }
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let max_cycles = Cycle::MAX;
    let result = verifier.verify_without_pause(script_version, &rtx, max_cycles);
    match expected_result {
        Ok(()) => {
            assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
        }
        Err(e) => {
            assert!(result.is_err());
            if script_version < ScriptVersion::V1 {
                return;
            }
            let err_string = format!("{}", result.unwrap_err());
            assert!(err_string.contains(&e), "{}", err_string);
        }
    }
}

#[test]
fn exec_from_cell_data_1times_no_load() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0000, 1, 2, 1, *from, res);
    }
}

#[test]
fn exec_from_cell_data_100times_no_load() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0000, 100, 101, 1, *from, res);
    }
}

#[test]
fn exec_from_cell_data_1times_and_load_before() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0001, 1, 1, 1, *from, res);
    }
}

#[test]
fn exec_from_cell_data_100times_and_load_before() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0001, 100, 51, 2, *from, res);
    }
}

#[test]
fn exec_from_cell_data_1times_and_load_after() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0100, 1, 2, 2, *from, res);
    }
}

#[test]
fn exec_from_cell_data_100times_and_load_after() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0100, 100, 101, 2, *from, res);
    }
}

#[test]
fn exec_from_cell_data_1times_and_load_both_and_write() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0111, 1, 1, 2, *from, res);
    }
}

#[test]
fn exec_from_cell_data_100times_and_load_both_and_write() {
    for from in &[
        ExecFrom::TxCellDep,
        ExecFrom::TxInputCell,
        ExecFrom::TxOutputCell,
        ExecFrom::GroupInputCell,
        ExecFrom::GroupOutputCell,
    ] {
        let res = Ok(());
        test_exec(0b0111, 100, 51, 4, *from, res);
    }
}

#[test]
fn exec_from_witness_1times_no_load() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0000, 1, 2, 1, *from, res);
    }
}

#[test]
fn exec_from_witness_100times_no_load() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0000, 100, 101, 1, *from, res);
    }
}

#[test]
fn exec_from_witness_1times_and_load_before() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0001, 1, 1, 1, *from, res);
    }
}

#[test]
fn exec_from_witness_100times_and_load_before() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0001, 100, 51, 2, *from, res);
    }
}

#[test]
fn exec_from_witness_1times_and_load_after() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0100, 1, 2, 2, *from, res);
    }
}

#[test]
fn exec_from_witness_100times_and_load_after() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0100, 100, 101, 2, *from, res);
    }
}

#[test]
fn exec_from_witness_1times_and_load_both_and_write() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0111, 1, 1, 2, *from, res);
    }
}

#[test]
fn exec_from_witness_100times_and_load_both_and_write() {
    for from in &[
        ExecFrom::TxInputWitness,
        ExecFrom::TxOutputWitness,
        ExecFrom::GroupInputWitness,
        ExecFrom::GroupOutputWitness,
    ] {
        let res = Ok(());
        test_exec(0b0111, 100, 51, 4, *from, res);
    }
}

#[test]
fn exec_from_witness_source_out_bound() {
    for from in &[
        ExecFrom::OutOfBound(0, 3, 1, 0),
        ExecFrom::OutOfBound(0, 4, 1, 0),
        ExecFrom::OutOfBound(0, SOURCE_GROUP_FLAG | 3, 0, 0),
        ExecFrom::OutOfBound(0, SOURCE_GROUP_FLAG | 4, 0, 0),
    ] {
        let res = Err("error code 1".to_string());
        test_exec(0b0000, 1, 2, 1, *from, res);
    }
}

#[test]
fn exec_from_cell_data_source_out_bound() {
    for from in &[
        ExecFrom::OutOfBound(1, 4, 0, 0),
        ExecFrom::OutOfBound(1, SOURCE_GROUP_FLAG | 3, 0, 0),
        ExecFrom::OutOfBound(1, SOURCE_GROUP_FLAG | 4, 0, 0),
    ] {
        let res = Err("error code 1".to_string());
        test_exec(0b0000, 1, 2, 1, *from, res);
    }
}

#[test]
fn exec_from_witness_place_error() {
    let script_version = SCRIPT_VERSION;

    let from = ExecFrom::OutOfBound(0, 1, 3, 0);
    let res = if script_version <= ScriptVersion::V1 {
        Err("Place parse_from_u64".to_string())
    } else {
        Err("error code 1".to_string())
    };
    test_exec(0b0000, 1, 2, 1, from, res);
}

#[test]
fn exec_slice() {
    let script_version = SCRIPT_VERSION;

    let (exec_callee_cell, _exec_callee_data_hash) =
        load_cell_from_path("testdata/exec_configurable_callee");
    let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
    let length = exec_callee_cell_data.len() as u64;

    let from = ExecFrom::OutOfSlice(length);
    let res = Ok(());
    test_exec(0b0000, 1, 2, 1, from, res);

    let from = ExecFrom::OutOfSlice(length + 1);
    let res = if script_version >= ScriptVersion::V2 {
        Ok(())
    } else {
        Err("error code 3".to_string())
    };
    test_exec(0b0000, 1, 2, 1, from, res);

    let from = ExecFrom::OutOfSlice(((length - 1) << 32) | 1);
    let res = if script_version >= ScriptVersion::V2 {
        Err("Malformed entity: Too small".to_string())
    } else {
        Err("MemWriteOnExecutablePage".to_string())
    };
    test_exec(0b0000, 1, 2, 1, from, res);

    let from = ExecFrom::Slice((10 << 32) | length);
    let res = Ok(());
    test_exec(0b0000, 1, 2, 1, from, res);
}

#[test]
fn check_signature_referenced_via_type_hash_ok_with_multiple_matches() {
    let script_version = SCRIPT_VERSION;
    if script_version < ScriptVersion::V1 {
        // This transaction is restricted by rfc_0029 and not supported in the 2019 version
        return;
    }

    let mut file = open_cell_always_success();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();
    let data = Bytes::from(buffer);

    let (privkey, pubkey) = random_keypair();
    let mut args = b"foobar".to_vec();

    let signature = sign_args(&args, &privkey);
    args.extend(&to_hex_pubkey(&pubkey));
    args.extend(&to_hex_signature(&signature));

    let dep_out_point = OutPoint::new(h256!("0x123").into(), 8);
    let cell_dep = CellDep::new_builder()
        .out_point(dep_out_point.clone())
        .build();
    let output = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .type_(Some(
            Script::new_builder()
                .code_hash(h256!("0x123456abcd90"))
                .hash_type(ScriptHashType::Data)
                .build(),
        ))
        .build();
    let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
    let dep_cell = CellMetaBuilder::from_cell_output(output, data.clone())
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point)
        .build();

    let dep_out_point2 = OutPoint::new(h256!("0x1234").into(), 8);
    let cell_dep2 = CellDep::new_builder()
        .out_point(dep_out_point2.clone())
        .build();
    let output2 = CellOutputBuilder::default()
        .capacity(Capacity::bytes(data.len()).unwrap())
        .type_(Some(
            Script::new_builder()
                .code_hash(h256!("0x123456abcd90"))
                .hash_type(ScriptHashType::Data)
                .build(),
        ))
        .build();
    let dep_cell2 = CellMetaBuilder::from_cell_output(output2, data)
        .transaction_info(default_transaction_info())
        .out_point(dep_out_point2)
        .build();

    let script = Script::new_builder()
        .args(Bytes::from(args))
        .code_hash(type_hash)
        .hash_type(ScriptHashType::Type)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .cell_dep(cell_dep)
        .cell_dep(cell_dep2)
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100))
        .lock(script)
        .build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![dep_cell, dep_cell2],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.ok(), Some(539));
}
