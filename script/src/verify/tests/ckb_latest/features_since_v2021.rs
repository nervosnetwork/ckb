use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_CYCLES, TYPE_ID_CODE_HASH};
use ckb_error::assert_error_eq;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{capacity_bytes, cell::CellMetaBuilder, Capacity, ScriptHashType, TransactionBuilder},
    h256,
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint, Script},
};
use ckb_vm::Error as VmError;
use proptest::{prelude::*, prop_assert_eq, proptest};
use rand::distributions::Uniform;
use rand::{thread_rng, Rng};
use std::{collections::VecDeque, io::Read};

use super::SCRIPT_VERSION;
use crate::syscalls::SOURCE_GROUP_FLAG;
use crate::{
    type_id::TYPE_ID_CYCLES,
    verify::{tests::utils::*, *},
    ScriptError,
};

#[test]
fn test_hint_instructions() {
    let script_version = SCRIPT_VERSION;

    let (always_success_cell, always_success_data_hash) =
        load_cell_from_path("testdata/cadd_hint_lock");

    let always_success_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(always_success_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
        vec.pack()
    };

    let (cpop_lock_cell, cpop_lock_data_hash) = load_cell_from_path("testdata/cpop_lock");

    let cpop_lock_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(cpop_lock_data_hash)
        .args(args)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
        .hash_type(script_version.data_hash_type().into())
        .code_hash(always_success_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
        .hash_type(script_version.data_hash_type().into())
        .code_hash(current_cycles_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
fn check_current_cycles_with_snapshot() {
    let script_version = SCRIPT_VERSION;

    let (current_cycles_cell, current_cycles_data_hash) =
        load_cell_from_path("testdata/current_cycles_with_snapshot");

    let current_cycles_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(current_cycles_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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

    let max_cycles = Cycle::MAX;

    let result = verifier.verify_without_pause(script_version, &rtx, max_cycles);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);

    if script_version < ScriptVersion::V1 {
        return;
    }

    let cycles_once = result.unwrap();
    let (cycles, chunks_count) = verifier
        .verify_until_completed(script_version, &rtx)
        .unwrap();
    assert_eq!(cycles, cycles_once);
    assert!(chunks_count > 0);
}

#[test]
fn check_vm_version() {
    let script_version = SCRIPT_VERSION;

    let (vm_version_cell, vm_version_data_hash) = load_cell_from_path("testdata/vm_version");

    let vm_version_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(vm_version_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
fn check_vm_version_with_snapshot() {
    let script_version = SCRIPT_VERSION;

    let (vm_version_cell, vm_version_data_hash) =
        load_cell_from_path("testdata/vm_version_with_snapshot");

    let vm_version_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(vm_version_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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

    let max_cycles = Cycle::MAX;

    let result = verifier.verify_without_pause(script_version, &rtx, max_cycles);
    assert_eq!(result.is_ok(), script_version == ScriptVersion::V1);

    if script_version != ScriptVersion::V1 {
        return;
    }

    let cycles_once = result.unwrap();
    let (cycles, chunks_count) = verifier
        .verify_until_completed(script_version, &rtx)
        .unwrap();
    assert_eq!(cycles, cycles_once);
    assert!(chunks_count > 0);
}

#[test]
fn check_exec_from_cell_data() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_from_cell_data");
    let (exec_callee_cell, _exec_callee_data_hash) = load_cell_from_path("testdata/exec_callee");

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
        .hash_type(script_version.data_hash_type().into())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
    let transaction = TransactionBuilder::default()
        .input(input)
        .set_witnesses(vec![exec_callee_cell_data.pack()])
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
        .hash_type(script_version.data_hash_type().into())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
        .hash_type(script_version.data_hash_type().into())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
        .hash_type(script_version.data_hash_type().into())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("VM Internal Error: ElfParseError"));
        }
    }
}

fn _check_type_id_one_in_one_out_resume(step_cycles: Cycle) -> Result<(), TestCaseError> {
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

    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();

    verifier.verify_map(script_version, &rtx, |verifier| {
        let mut groups: VecDeque<_> = verifier.groups_with_type().collect();
        let mut tmp: Option<FullSuspendedState> = None;
        let mut current_group = None;
        let mut limit = step_cycles;

        loop {
            if let Some(cur_state) = tmp.take() {
                match verifier.verify_group_with_chunk(
                    current_group.unwrap(),
                    limit,
                    &Some(cur_state),
                ) {
                    Ok(ChunkState::Completed(used_cycles, _consumed_cycles)) => {
                        cycles += used_cycles;
                        groups.pop_front();
                        tmp = None;
                    }
                    Ok(ChunkState::Suspended(suspend_state)) => {
                        tmp = suspend_state;
                        limit += step_cycles;
                        continue;
                    }
                    Err(_error) => {
                        unreachable!();
                    }
                }
            }
            if groups.is_empty() {
                break;
            }

            while let Some((ty, _, group)) = groups.front().cloned() {
                match verifier
                    .verify_group_with_chunk(group, limit, &tmp)
                    .unwrap()
                {
                    ChunkState::Completed(used_cycles, _consumed_cycles) => {
                        cycles += used_cycles;
                        groups.pop_front();
                        tmp = None;
                        if groups.front().is_some() {
                            limit = step_cycles;
                        }
                    }
                    ChunkState::Suspended(suspend_state) => {
                        if suspend_state.is_some() {
                            tmp = suspend_state;
                            current_group = Some(group);
                        } else if ty == ScriptGroupType::Type // fast forward
                            && step_cycles > TYPE_ID_CYCLES
                            && limit < (TYPE_ID_CYCLES - step_cycles)
                        {
                            limit += TYPE_ID_CYCLES - step_cycles;
                        } else if ty == ScriptGroupType::Lock  // fast forward
                            && step_cycles < ALWAYS_SUCCESS_SCRIPT_CYCLE
                            && limit < (ALWAYS_SUCCESS_SCRIPT_CYCLE - step_cycles)
                        {
                            limit += ALWAYS_SUCCESS_SCRIPT_CYCLE - step_cycles;
                        } else {
                            limit += step_cycles;
                        }
                        break;
                    }
                }
            }
        }
    });

    prop_assert_eq!(cycles, TYPE_ID_CYCLES + ALWAYS_SUCCESS_SCRIPT_CYCLE);
    Ok(())
}

// default is 256, which takes too long times
proptest! {
    #![proptest_config(ProptestConfig::with_cases(42))]
    #[test]
    fn check_type_id_one_in_one_out_resume1(step in 1..ALWAYS_SUCCESS_SCRIPT_CYCLE) {
        if SCRIPT_VERSION >= ScriptVersion::V1 {
            _check_type_id_one_in_one_out_resume(step)?;
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(42))]
    #[test]
    fn check_type_id_one_in_one_out_resume2(step in ALWAYS_SUCCESS_SCRIPT_CYCLE..(ALWAYS_SUCCESS_SCRIPT_CYCLE + TYPE_ID_CYCLES)) {
        _check_type_id_one_in_one_out_resume(step)?;
    }
}

fn _check_type_id_one_in_one_out_resume_with_state(
    step_cycles: Cycle,
) -> Result<(), TestCaseError> {
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

    let verifier = TransactionScriptsVerifierWithEnv::new();

    let (cycles, chunks_count) = verifier
        .verify_map(script_version, &rtx, |verifier| {
            let mut limit = step_cycles;
            let cycles;
            let mut times = 0usize;
            times += 1;
            let mut init_state = match verifier.resumable_verify(limit).unwrap() {
                VerifyResult::Suspended(state) => {
                    limit *= 2;
                    Some(state)
                }
                VerifyResult::Completed(cycle) => {
                    cycles = cycle;
                    return Ok((cycles, times));
                }
            };

            loop {
                times += 1;
                let state = init_state.take().unwrap();
                match verifier.resume_from_state(&state, limit).unwrap() {
                    VerifyResult::Suspended(state) => {
                        init_state = Some(state);
                        limit *= 2;
                    }
                    VerifyResult::Completed(cycle) => {
                        cycles = cycle;
                        break;
                    }
                }
            }

            Ok::<(u64, usize), Error>((cycles, times))
        })
        .unwrap();
    assert_eq!(cycles, TYPE_ID_CYCLES + ALWAYS_SUCCESS_SCRIPT_CYCLE);
    assert!(chunks_count > 1);

    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(42))]
    #[test]
    fn check_type_id_one_in_one_out_resume_with_state(step in ALWAYS_SUCCESS_SCRIPT_CYCLE..(ALWAYS_SUCCESS_SCRIPT_CYCLE + TYPE_ID_CYCLES)) {
        _check_type_id_one_in_one_out_resume_with_state(step)?;
    }
}

fn _check_typical_secp256k1_blake160_2_in_2_out_tx_with_chunk(step_cycles: Cycle) {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();

    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut groups: Vec<_> = verifier.groups_with_type().collect();
        let mut tmp = None;
        let mut limit = step_cycles;

        loop {
            while let Some(group) = groups.pop() {
                match verifier
                    .verify_group_with_chunk(group.2, limit, &tmp)
                    .unwrap()
                {
                    ChunkState::Completed(used_cycles, _consumed_cycles) => {
                        cycles += used_cycles;
                        tmp = None;
                    }
                    ChunkState::Suspended(snapshot) => {
                        tmp = snapshot;
                        if limit < (TWO_IN_TWO_OUT_CYCLES - step_cycles) {
                            limit += TWO_IN_TWO_OUT_CYCLES - step_cycles;
                        } else {
                            limit += step_cycles;
                        }
                        groups.push(group);
                        break;
                    }
                }
            }

            if groups.is_empty() {
                break;
            }
        }

        verifier.verify(TWO_IN_TWO_OUT_CYCLES)
    });

    let cycles_once = result.unwrap();
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES, "step_cycles {step_cycles}");

    if script_version == crate::ScriptVersion::V2 {
        assert!(
            cycles >= TWO_IN_TWO_OUT_CYCLES - V2_CYCLE_BOUND,
            "step_cycles {step_cycles}"
        );
    } else {
        assert!(
            cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND,
            "step_cycles {step_cycles}"
        );
    }
    assert_eq!(cycles, cycles_once, "step_cycles {step_cycles}");
    // Note that different rand versions may cause different randomly
    // generated tx data, which in turn leads to different final cycles.
    if script_version < crate::ScriptVersion::V2 {
        assert_eq!(cycles, 3334802);
    } else {
        assert_eq!(cycles, 3225879);
    }
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_chunk() {
    let cycle_bound = if SCRIPT_VERSION >= ScriptVersion::V2 {
        V2_CYCLE_BOUND
    } else {
        CYCLE_BOUND
    };
    if SCRIPT_VERSION >= ScriptVersion::V1 {
        let mut rng = thread_rng();
        let step_cycles1 = rng.sample(Uniform::from(1..100u64));
        _check_typical_secp256k1_blake160_2_in_2_out_tx_with_chunk(step_cycles1);

        let step_cycles2 = rng.sample(Uniform::from(100u64..TWO_IN_TWO_OUT_CYCLES - cycle_bound));
        _check_typical_secp256k1_blake160_2_in_2_out_tx_with_chunk(step_cycles2);
    }
}

fn _check_typical_secp256k1_blake160_2_in_2_out_tx_with_state(step_cycles: Cycle) {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();
    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        #[allow(unused_assignments)]
        let mut init_state: Option<TransactionState> = None;

        match verifier.resumable_verify(step_cycles).unwrap() {
            VerifyResult::Suspended(state) => init_state = Some(state),
            VerifyResult::Completed(cycle) => return Ok(cycle),
        }

        loop {
            let state = init_state.take().unwrap();
            let (limit_cycles, _last) = state.next_limit_cycles(step_cycles, TWO_IN_TWO_OUT_CYCLES);
            match verifier.resume_from_state(&state, limit_cycles).unwrap() {
                VerifyResult::Suspended(state) => init_state = Some(state),
                VerifyResult::Completed(cycle) => {
                    cycles = cycle;
                    break;
                }
            }
        }

        verifier.verify(TWO_IN_TWO_OUT_CYCLES)
    });

    let cycles_once = result.unwrap();
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES, "step_cycles {step_cycles}");
    if script_version == crate::ScriptVersion::V2 {
        assert!(
            cycles >= TWO_IN_TWO_OUT_CYCLES - V2_CYCLE_BOUND,
            "step_cycles {step_cycles}"
        );
    } else {
        assert!(
            cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND,
            "step_cycles {step_cycles}"
        );
    }
    assert_eq!(cycles, cycles_once, "step_cycles {step_cycles}");
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_state() {
    let cycle_bound = if SCRIPT_VERSION >= ScriptVersion::V2 {
        V2_CYCLE_BOUND
    } else {
        CYCLE_BOUND
    };
    if SCRIPT_VERSION >= ScriptVersion::V1 {
        let mut rng = thread_rng();
        let step_cycles1 = rng.sample(Uniform::from(1..100u64));
        _check_typical_secp256k1_blake160_2_in_2_out_tx_with_state(step_cycles1);

        let step_cycles2 = rng.sample(Uniform::from(100u64..TWO_IN_TWO_OUT_CYCLES - cycle_bound));
        _check_typical_secp256k1_blake160_2_in_2_out_tx_with_state(step_cycles2);
    }
}

fn _check_typical_secp256k1_blake160_2_in_2_out_tx_with_snap(step_cycles: Cycle) {
    let script_version = SCRIPT_VERSION;
    let rtx = random_2_in_2_out_rtx();
    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();

    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        #[allow(unused_assignments)]
        let mut init_snap: Option<TransactionState> = None;
        let mut init_state: Option<TransactionState> = None;

        match verifier.resumable_verify(step_cycles).unwrap() {
            VerifyResult::Suspended(state) => init_snap = Some(state),
            VerifyResult::Completed(cycle) => return Ok(cycle),
        }

        let mut count = 0;
        loop {
            if init_snap.is_some() {
                let snap = init_snap.take().unwrap();
                let (limit_cycles, _last) =
                    snap.next_limit_cycles(step_cycles, TWO_IN_TWO_OUT_CYCLES);
                match verifier.resume_from_state(&snap, limit_cycles).unwrap() {
                    VerifyResult::Suspended(state) => {
                        if count % 500 == 0 {
                            init_snap = Some(state);
                        } else {
                            init_state = Some(state);
                        }
                    }
                    VerifyResult::Completed(cycle) => {
                        cycles = cycle;
                        break;
                    }
                }
            } else {
                let state = init_state.take().unwrap();
                let (limit_cycles, _last) =
                    state.next_limit_cycles(step_cycles, TWO_IN_TWO_OUT_CYCLES);
                match verifier.resume_from_state(&state, limit_cycles).unwrap() {
                    VerifyResult::Suspended(state) => {
                        if count % 500 == 0 {
                            init_snap = Some(state);
                        } else {
                            init_state = Some(state);
                        }
                    }
                    VerifyResult::Completed(cycle) => {
                        cycles = cycle;
                        break;
                    }
                }
            }
            count += 1;
        }

        verifier.verify(TWO_IN_TWO_OUT_CYCLES)
    });

    let cycles_once = result.unwrap();
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES, "step_cycles {step_cycles}");
    if script_version == crate::ScriptVersion::V2 {
        assert!(
            cycles >= TWO_IN_TWO_OUT_CYCLES - V2_CYCLE_BOUND,
            "cycles {cycles} step_cycles {step_cycles}"
        );
    } else {
        assert!(
            cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND,
            "cycles {cycles} step_cycles {step_cycles}"
        );
    }
    assert_eq!(cycles, cycles_once, "step_cycles {step_cycles}");
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_snap() {
    let cycle_bound = if SCRIPT_VERSION >= ScriptVersion::V2 {
        V2_CYCLE_BOUND
    } else {
        CYCLE_BOUND
    };
    if SCRIPT_VERSION >= ScriptVersion::V1 {
        let mut rng = thread_rng();
        let step_cycles1 = rng.sample(Uniform::from(1..100u64));
        _check_typical_secp256k1_blake160_2_in_2_out_tx_with_snap(step_cycles1);

        let step_cycles2 = rng.sample(Uniform::from(
            TWO_IN_TWO_OUT_CYCLES / 10..TWO_IN_TWO_OUT_CYCLES - cycle_bound,
        ));
        _check_typical_secp256k1_blake160_2_in_2_out_tx_with_snap(step_cycles2);
    }
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_complete() {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();
    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionState> = None;

        if let VerifyResult::Suspended(state) = verifier
            .resumable_verify(TWO_IN_TWO_OUT_CYCLES / 10)
            .unwrap()
        {
            init_snap = Some(state);
        }

        for _ in 0..2 {
            let snap = init_snap.take().unwrap();
            let (limit_cycles, _last) =
                snap.next_limit_cycles(TWO_IN_TWO_OUT_CYCLES / 10, TWO_IN_TWO_OUT_CYCLES);
            match verifier.resume_from_state(&snap, limit_cycles).unwrap() {
                VerifyResult::Suspended(state) => init_snap = Some(state),
                VerifyResult::Completed(_) => {
                    unreachable!()
                }
            }
        }

        cycles = verifier
            .complete(&init_snap.take().unwrap(), TWO_IN_TWO_OUT_CYCLES)
            .unwrap();

        verifier.verify(TWO_IN_TWO_OUT_CYCLES)
    });

    let cycles_once = result.unwrap();
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES);

    if script_version == crate::ScriptVersion::V2 {
        assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - V2_CYCLE_BOUND);
    } else {
        assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    }
    assert_eq!(cycles, cycles_once);
    // Note that different rand versions may cause different randomly
    // generated tx data, which in turn leads to different final cycles.
    if script_version <= ScriptVersion::V0 {
        assert_eq!(cycles, 3352333);
    } else if script_version == ScriptVersion::V1 {
        assert_eq!(cycles, 3334802);
    } else if script_version == ScriptVersion::V2 {
        assert_eq!(cycles, 3225879);
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
            vec.pack()
        };

        let (dyn_lock_cell, dyn_lock_data_hash) =
            load_cell_from_path("testdata/load_is_even_into_global");

        let dyn_lock_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(dyn_lock_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
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

#[test]
fn load_code_with_snapshot() {
    let script_version = SCRIPT_VERSION;

    let (dyn_lib_cell, dyn_lib_data_hash) = load_cell_from_path("testdata/is_even.lib");

    let rtx = {
        let args: packed::Bytes = {
            let number = 0x01u64; // a random odd value

            let data_hash = dyn_lib_data_hash.raw_data();
            let mut vec = Vec::with_capacity(8 + data_hash.len());
            vec.extend_from_slice(&number.to_le_bytes());
            vec.extend_from_slice(&data_hash);
            vec.pack()
        };

        let (dyn_lock_cell, dyn_lock_data_hash) =
            load_cell_from_path("testdata/load_is_even_with_snapshot");

        let dyn_lock_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(dyn_lock_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
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

    let mut cycles = 0;
    let max_cycles = Cycle::MAX;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionState> = None;

        if let VerifyResult::Suspended(state) = verifier.resumable_verify(max_cycles).unwrap() {
            init_snap = Some(state);
        }

        let snap = init_snap.take().unwrap();
        let result = verifier.resume_from_state(&snap, max_cycles);

        match result.unwrap() {
            VerifyResult::Suspended(state) => {
                panic!("should be completed, {state:?}");
            }
            VerifyResult::Completed(cycle) => {
                cycles = cycle;
            }
        }

        verifier.set_skip_pause(true);
        verifier.verify(max_cycles)
    });

    let cycles_once = result.unwrap();
    assert_eq!(cycles, cycles_once);
    if script_version == ScriptVersion::V0 {
        assert_eq!(cycles_once, 11062);
    } else if script_version == ScriptVersion::V1 {
        assert_eq!(cycles_once, 11064);
    } else {
        assert_eq!(cycles_once, 11060);
    }
}

#[test]
fn load_code_with_snapshot_more_times() {
    let script_version = SCRIPT_VERSION;

    let (add1_cell, add1_data_hash) = load_cell_from_path("testdata/add1.lib");
    let (sub1_cell, sub1_data_hash) = load_cell_from_path("testdata/sub1.lib");
    let (mul2_cell, mul2_data_hash) = load_cell_from_path("testdata/mul2.lib");
    let (div2_cell, div2_data_hash) = load_cell_from_path("testdata/div2.lib");
    let (lock_cell, lock_data_hash) = load_cell_from_path("testdata/load_arithmetic");

    let rtx = {
        let args: packed::Bytes = {
            let add1 = add1_data_hash.raw_data();
            let sub1 = sub1_data_hash.raw_data();
            let mul2 = mul2_data_hash.raw_data();
            let div2 = div2_data_hash.raw_data();

            let mut vec = Vec::new();

            let num0 = 0u64;
            let num1 = 1u64;

            vec.extend_from_slice(&num0.to_le_bytes());
            vec.extend_from_slice(&num1.to_le_bytes());
            vec.extend_from_slice(&add1); // num0 = 1
            vec.extend_from_slice(&mul2); // num0 = 2
            vec.extend_from_slice(&add1); // num0 = 3
            vec.extend_from_slice(&mul2); // num0 = 6
            vec.extend_from_slice(&mul2); // num0 = 12
            vec.extend_from_slice(&add1); // num0 = 13
            vec.extend_from_slice(&add1); // num0 = 14
            vec.extend_from_slice(&div2); // num0 = 7
            vec.extend_from_slice(&sub1); // num0 = 6
            vec.extend_from_slice(&div2); // num0 = 3
            vec.extend_from_slice(&sub1); // num0 = 2
            vec.extend_from_slice(&div2); // num0 = 1
            vec.pack()
        };

        let lock_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(lock_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input).build();
        let dummy_cell = create_dummy_cell(output);

        ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![add1_cell, sub1_cell, mul2_cell, div2_cell, lock_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        }
    };

    let mut cycles = 0;
    let max_cycles = Cycle::MAX;
    let verifier = TransactionScriptsVerifierWithEnv::new();

    verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionState> = None;

        if let VerifyResult::Suspended(state) = verifier.resumable_verify(max_cycles).unwrap() {
            init_snap = Some(state);
        }

        loop {
            let snap = init_snap.take().unwrap();
            let result = verifier.resume_from_state(&snap, max_cycles);

            match result.unwrap() {
                VerifyResult::Suspended(state) => {
                    init_snap = Some(state);
                }
                VerifyResult::Completed(cycle) => {
                    cycles = cycle;
                    break;
                }
            }
        }
    });

    let result = verifier.verify_without_pause(script_version, &rtx, max_cycles);
    let cycles_once = result.unwrap();
    assert_eq!(cycles, cycles_once);
    if script_version == ScriptVersion::V0 {
        assert_eq!(cycles_once, 45740);
    } else if script_version == ScriptVersion::V1 {
        assert_eq!(cycles_once, 45742);
    } else {
        assert_eq!(cycles_once, 45729);
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
    expected_result: Result<usize, String>,
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
        vec.pack()
    };

    let rtx = {
        let (exec_caller_cell, exec_caller_data_hash) =
            load_cell_from_path("testdata/exec_configurable_caller");
        let (exec_callee_cell, _exec_callee_data_hash) =
            load_cell_from_path("testdata/exec_configurable_callee");

        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();

        let exec_caller_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(exec_caller_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
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
                    .set_witnesses(vec![exec_callee_cell_data.pack()])
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
                    .set_witnesses(vec![data.pack()])
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
                    .capacity(capacity_bytes!(100).pack())
                    .type_(Some(exec_caller_script).pack())
                    .build();
                let tx = TransactionBuilder::default()
                    .output(output)
                    .set_witnesses(vec![exec_callee_cell_data.pack()])
                    .build();
                (tx, vec![])
            }
            ExecFrom::TxInputCell => {
                let callee_output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(1000).pack())
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
                    .capacity(capacity_bytes!(100).pack())
                    .lock(exec_caller_script)
                    .type_(Some(always_success_script.clone()).pack())
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
                    .capacity(capacity_bytes!(100).pack())
                    .lock(always_success_script.clone())
                    .build();
                let tx = TransactionBuilder::default()
                    .input(input)
                    .output(callee_output)
                    .output_data(exec_callee_cell_data.pack())
                    .build();
                (tx, vec![create_dummy_cell(output)])
            }
            ExecFrom::GroupOutputCell => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let callee_output = CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100).pack())
                    .type_(Some(exec_caller_script).pack())
                    .build();
                let tx = TransactionBuilder::default()
                    .output(callee_output)
                    .output_data(exec_callee_cell_data.pack())
                    .build();
                (tx, vec![])
            }
            ExecFrom::OutOfBound(..) => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                let tx = TransactionBuilder::default()
                    .set_witnesses(vec![exec_callee_cell_data.pack()])
                    .build();
                (tx, vec![create_dummy_cell(output)])
            }
        };

        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);
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
        Ok(expected_chunks_count) => {
            assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
            if script_version < ScriptVersion::V1 {
                return;
            }
            let cycles_once = result.unwrap();
            let (cycles, chunks_count) = verifier
                .verify_until_completed(script_version, &rtx)
                .unwrap();
            assert_eq!(cycles, cycles_once);
            assert_eq!(chunks_count, expected_chunks_count);
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
        let res = Ok(2);
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
        let res = Ok(101);
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
        let res = Ok(5);
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
        let res = Ok(104);
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
        let res = Ok(4);
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
        let res = Ok(103);
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
        let res = Ok(7);
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
        let res = Ok(106);
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
        let res = Ok(2);
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
        let res = Ok(101);
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
        let res = Ok(5);
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
        let res = Ok(104);
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
        let res = Ok(4);
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
        let res = Ok(103);
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
        let res = Ok(7);
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
        let res = Ok(106);
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
    let res = Ok(2);
    test_exec(0b0000, 1, 2, 1, from, res);

    let from = ExecFrom::OutOfSlice(length + 1);
    let res = if script_version >= ScriptVersion::V2 {
        Ok(2)
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
    let res = Ok(2);
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

#[test]
fn check_exec_callee_pause() {
    let script_version = SCRIPT_VERSION;
    if script_version < crate::ScriptVersion::V1 {
        return;
    }

    let (exec_caller_cell, exec_caller_data_hash) =
        load_cell_from_path("testdata/exec_caller_from_cell_data");
    let (exec_callee_cell, _exec_callee_data_hash) =
        load_cell_from_path("testdata/exec_callee_pause");

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(exec_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
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
    let result = verifier.verify_until_completed(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
    assert_eq!(result.unwrap().1, 6);
}
