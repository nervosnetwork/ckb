use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_CYCLES, TYPE_ID_CODE_HASH};
use ckb_error::assert_error_eq;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{capacity_bytes, cell::CellMetaBuilder, Capacity, ScriptHashType, TransactionBuilder},
    h256,
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint, Script},
};
use ckb_vm::Error as VmError;

use super::SCRIPT_VERSION;
use crate::{
    type_id::TYPE_ID_CYCLES,
    verify::{tests::utils::*, *},
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
        let script_error = ScriptError::VMInternalError(format!("{:?}", vm_error));
        assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
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
    if script_version < ScriptVersion::V1 {
        let vm_error = VmError::InvalidInstruction {
            pc: 0x10182,
            instruction: 0x60291913,
        };
        let script_error = ScriptError::VMInternalError(format!("{:?}", vm_error));
        assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
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
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V1);
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
    if script_version >= ScriptVersion::V1 {
        assert!(result.unwrap_err().to_string().contains("error code 3"));
    }
}

#[test]
fn check_type_id_one_in_one_out_resume() {
    use std::collections::VecDeque;

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
        let mut tmp: Option<ResumableMachine<'_>> = None;
        let mut step_cycles = match groups.front().unwrap().0 {
            ScriptGroupType::Lock => ALWAYS_SUCCESS_SCRIPT_CYCLE,
            ScriptGroupType::Type => TYPE_ID_CYCLES - 10,
        };

        loop {
            if let Some(mut vm) = tmp.take() {
                cycles += vm.cycles();
                vm.set_cycles(0);
                match vm.machine.run() {
                    Ok(code) => {
                        if code == 0 {
                            cycles += vm.cycles();
                            groups.pop_front();
                        } else {
                            unreachable!()
                        }
                    }
                    Err(error) => match error {
                        VMInternalError::InvalidCycles => {
                            tmp = Some(vm);
                            continue;
                        }
                        _ => unreachable!(),
                    },
                }
            }
            if groups.is_empty() {
                break;
            }

            while let Some((_ty, _, group)) = groups.front().cloned() {
                match verifier
                    .verify_group_with_chunk(group, step_cycles, &None)
                    .unwrap()
                {
                    ChunkState::Completed(used_cycles) => {
                        cycles += used_cycles;
                        groups.pop_front();
                        if let Some(front) = groups.front() {
                            step_cycles = match front.0 {
                                ScriptGroupType::Lock => ALWAYS_SUCCESS_SCRIPT_CYCLE,
                                ScriptGroupType::Type => TYPE_ID_CYCLES - 10,
                            };
                        }
                    }
                    ChunkState::Suspended(vm) => {
                        if vm.is_some() {
                            tmp = vm;
                        } else {
                            step_cycles += 10
                        }
                        break;
                    }
                }
            }
        }
    });

    assert_eq!(cycles, TYPE_ID_CYCLES + ALWAYS_SUCCESS_SCRIPT_CYCLE);
}

#[test]
fn check_type_id_one_in_one_out_chunk() {
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
        let mut groups: Vec<_> = verifier.groups_with_type().collect();
        let mut tmp: Option<ResumableMachine<'_>> = None;

        loop {
            if let Some(mut vm) = tmp.take() {
                cycles += vm.cycles();
                vm.set_cycles(0);
                match vm.machine.run() {
                    Ok(code) => {
                        if code == 0 {
                            cycles += vm.cycles();
                        } else {
                            unreachable!()
                        }
                    }
                    Err(error) => match error {
                        VMInternalError::InvalidCycles => {
                            tmp = Some(vm);
                            continue;
                        }
                        _ => unreachable!(),
                    },
                }
            }
            while let Some((ty, _, group)) = groups.pop() {
                let max = match ty {
                    ScriptGroupType::Lock => ALWAYS_SUCCESS_SCRIPT_CYCLE - 10,
                    ScriptGroupType::Type => TYPE_ID_CYCLES,
                };
                match verifier.verify_group_with_chunk(group, max, &None).unwrap() {
                    ChunkState::Completed(used_cycles) => {
                        cycles += used_cycles;
                    }
                    ChunkState::Suspended(vm) => {
                        tmp = vm;
                        break;
                    }
                }
            }

            if tmp.is_none() {
                break;
            }
        }
    });

    assert_eq!(cycles, TYPE_ID_CYCLES + ALWAYS_SUCCESS_SCRIPT_CYCLE);
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_chunk() {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();

    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut groups: Vec<_> = verifier.groups_with_type().collect();
        let mut tmp: Option<ResumableMachine<'_>> = None;

        loop {
            if let Some(mut vm) = tmp.take() {
                cycles += vm.cycles();
                vm.set_cycles(0);
                match vm.machine.run() {
                    Ok(code) => {
                        if code == 0 {
                            cycles += vm.cycles();
                        } else {
                            unreachable!()
                        }
                    }
                    Err(error) => match error {
                        VMInternalError::InvalidCycles => {
                            tmp = Some(vm);
                            continue;
                        }
                        _ => unreachable!(),
                    },
                }
            }
            while let Some((_, _, group)) = groups.pop() {
                match verifier
                    .verify_group_with_chunk(group, TWO_IN_TWO_OUT_CYCLES / 10, &None)
                    .unwrap()
                {
                    ChunkState::Completed(used_cycles) => {
                        cycles += used_cycles;
                    }
                    ChunkState::Suspended(vm) => {
                        tmp = vm;
                        break;
                    }
                }
            }

            if tmp.is_none() {
                break;
            }
        }

        verifier.verify(TWO_IN_TWO_OUT_CYCLES)
    });

    let cycles_once = result.unwrap();
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES);
    assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    assert_eq!(cycles, cycles_once);
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_snap() {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();
    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionSnapshot> = None;

        if let VerifyResult::Suspended(state) = verifier
            .resumable_verify(TWO_IN_TWO_OUT_CYCLES / 10)
            .unwrap()
        {
            init_snap = Some(state.try_into().unwrap());
        }

        loop {
            let snap = init_snap.take().unwrap();
            let (limit_cycles, _last) =
                snap.next_limit_cycles(TWO_IN_TWO_OUT_CYCLES / 10, TWO_IN_TWO_OUT_CYCLES);
            match verifier.resume_from_snap(&snap, limit_cycles).unwrap() {
                VerifyResult::Suspended(state) => init_snap = Some(state.try_into().unwrap()),
                VerifyResult::Completed(cycle) => {
                    cycles = cycle;
                    break;
                }
            }
        }

        verifier.verify(TWO_IN_TWO_OUT_CYCLES)
    });

    let cycles_once = result.unwrap();
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES);
    assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    assert_eq!(cycles, cycles_once);
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_state() {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();
    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_state: Option<TransactionState<'_>> = None;

        if let VerifyResult::Suspended(state) = verifier
            .resumable_verify(TWO_IN_TWO_OUT_CYCLES / 10)
            .unwrap()
        {
            init_state = Some(state);
        }

        loop {
            let state = init_state.take().unwrap();
            let (limit_cycles, _last) =
                state.next_limit_cycles(TWO_IN_TWO_OUT_CYCLES / 10, TWO_IN_TWO_OUT_CYCLES);
            match verifier.resume_from_state(state, limit_cycles).unwrap() {
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
    assert!(cycles <= TWO_IN_TWO_OUT_CYCLES);
    assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    assert_eq!(cycles, cycles_once);
}

#[test]
fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_complete() {
    let script_version = SCRIPT_VERSION;

    let rtx = random_2_in_2_out_rtx();
    let mut cycles = 0;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionSnapshot> = None;

        if let VerifyResult::Suspended(state) = verifier
            .resumable_verify(TWO_IN_TWO_OUT_CYCLES / 10)
            .unwrap()
        {
            init_snap = Some(state.try_into().unwrap());
        }

        for _ in 0..2 {
            let snap = init_snap.take().unwrap();
            let (limit_cycles, _last) =
                snap.next_limit_cycles(TWO_IN_TWO_OUT_CYCLES / 10, TWO_IN_TWO_OUT_CYCLES);
            match verifier.resume_from_snap(&snap, limit_cycles).unwrap() {
                VerifyResult::Suspended(state) => init_snap = Some(state.try_into().unwrap()),
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
    assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    assert_eq!(cycles, cycles_once);
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
    if script_version < ScriptVersion::V1 {
        let vm_error = VmError::InvalidPermission;
        let script_error = ScriptError::VMInternalError(format!("{:?}", vm_error));
        assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
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
    let should_be_invalid_permission = script_version < ScriptVersion::V1;
    let result = verifier.verify_map(script_version, &rtx, |mut verifier| {
        let mut init_snap: Option<TransactionSnapshot> = None;

        if let VerifyResult::Suspended(state) = verifier.resumable_verify(max_cycles).unwrap() {
            init_snap = Some(state.try_into().unwrap());
        }

        let snap = init_snap.take().unwrap();
        let result = verifier.resume_from_snap(&snap, max_cycles);
        if should_be_invalid_permission {
            let vm_error = VmError::InvalidPermission;
            let script_error = ScriptError::VMInternalError(format!("{:?}", vm_error));
            assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
        } else {
            match result.unwrap() {
                VerifyResult::Suspended(state) => {
                    panic!("should be completed, {:?}", state);
                }
                VerifyResult::Completed(cycle) => {
                    assert!(
                        verifier.tracing_data_as_code_pages.borrow().is_empty(),
                        "Any group execution is complete, this must be empty"
                    );
                    cycles = cycle;
                }
            }
        }

        verifier.set_skip_pause(true);
        verifier.verify(max_cycles)
    });

    if should_be_invalid_permission {
        return;
    }

    let cycles_once = result.unwrap();
    assert_eq!(cycles, cycles_once);
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
    let should_be_invalid_permission = script_version < ScriptVersion::V1;

    verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionSnapshot> = None;

        if let VerifyResult::Suspended(state) = verifier.resumable_verify(max_cycles).unwrap() {
            init_snap = Some(state.try_into().unwrap());
        }

        loop {
            let snap = init_snap.take().unwrap();
            let result = verifier.resume_from_snap(&snap, max_cycles);
            if should_be_invalid_permission {
                let vm_error = VmError::InvalidPermission;
                let script_error = ScriptError::VMInternalError(format!("{:?}", vm_error));
                assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
                break;
            } else {
                match result.unwrap() {
                    VerifyResult::Suspended(state) => {
                        init_snap = Some(state.try_into().unwrap());
                    }
                    VerifyResult::Completed(cycle) => {
                        assert!(
                            verifier.tracing_data_as_code_pages.borrow().is_empty(),
                            "Any group execution is complete, this must be empty"
                        );
                        cycles = cycle;
                        break;
                    }
                }
            }
        }
    });

    if should_be_invalid_permission {
        return;
    }

    let result = verifier.verify_without_pause(script_version, &rtx, max_cycles);
    let cycles_once = result.unwrap();
    assert_eq!(cycles, cycles_once);
}

#[derive(Clone, Copy)]
enum ExecFrom {
    Witness,
    CellData,
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
    expected_result: Result<usize, ()>,
) {
    let script_version = SCRIPT_VERSION;

    let (dyn_lib_cell, dyn_lib_data_hash) = load_cell_from_path("testdata/mul2.lib");

    let args: packed::Bytes = {
        // The args for invoke exec.
        let (index, source, place, bounds): (u64, u64, u64, u64) = match exec_from {
            ExecFrom::Witness => (0, 1, 1, 0),
            ExecFrom::CellData => (1, 3, 0, 0),
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

        let exec_caller_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(exec_caller_data_hash)
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(exec_caller_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = match exec_from {
            ExecFrom::Witness => {
                let exec_callee_cell_data = exec_callee_cell.mem_cell_data.as_ref().unwrap();
                TransactionBuilder::default()
                    .input(input)
                    .set_witnesses(vec![exec_callee_cell_data.pack()])
                    .build()
            }
            ExecFrom::CellData => TransactionBuilder::default().input(input).build(),
        };

        let dummy_cell = create_dummy_cell(output);

        ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell, dyn_lib_cell],
            resolved_inputs: vec![dummy_cell],
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
        Err(_) => {
            assert!(result.is_err());
        }
    }
}

#[test]
fn exec_from_cell_data_1times_no_load() {
    let from = ExecFrom::CellData;
    let res = Ok(2);
    test_exec(0b0000, 1, 2, 1, from, res);
}

#[test]
fn exec_from_cell_data_100times_no_load() {
    let from = ExecFrom::CellData;
    let res = Ok(101);
    test_exec(0b0000, 100, 101, 1, from, res);
}

#[test]
fn exec_from_cell_data_1times_and_load_before() {
    let from = ExecFrom::CellData;
    let res = Ok(5);
    test_exec(0b0001, 1, 1, 1, from, res);
}

#[test]
fn exec_from_cell_data_100times_and_load_before() {
    let from = ExecFrom::CellData;
    let res = Ok(104);
    test_exec(0b0001, 100, 51, 2, from, res);
}

#[test]
fn exec_from_cell_data_1times_and_load_after() {
    let from = ExecFrom::CellData;
    let res = Ok(4);
    test_exec(0b0100, 1, 2, 2, from, res);
}

#[test]
fn exec_from_cell_data_100times_and_load_after() {
    let from = ExecFrom::CellData;
    let res = Ok(103);
    test_exec(0b0100, 100, 101, 2, from, res);
}

#[test]
fn exec_from_cell_data_1times_and_load_both_and_write() {
    let from = ExecFrom::CellData;
    let res = Ok(7);
    test_exec(0b0111, 1, 1, 2, from, res);
}

#[test]
fn exec_from_cell_data_100times_and_load_both_and_write() {
    let from = ExecFrom::CellData;
    let res = Ok(106);
    test_exec(0b0111, 100, 51, 4, from, res);
}

#[test]
fn exec_from_witness_1times_no_load() {
    let from = ExecFrom::Witness;
    let res = Ok(2);
    test_exec(0b0000, 1, 2, 1, from, res);
}

#[test]
fn exec_from_witness_100times_no_load() {
    let from = ExecFrom::Witness;
    let res = Ok(101);
    test_exec(0b0000, 100, 101, 1, from, res);
}

#[test]
fn exec_from_witness_1times_and_load_before() {
    let from = ExecFrom::Witness;
    let res = Ok(5);
    test_exec(0b0001, 1, 1, 1, from, res);
}

#[test]
fn exec_from_witness_100times_and_load_before() {
    let from = ExecFrom::Witness;
    let res = Ok(104);
    test_exec(0b0001, 100, 51, 2, from, res);
}

#[test]
fn exec_from_witness_1times_and_load_after() {
    let from = ExecFrom::Witness;
    let res = Ok(4);
    test_exec(0b0100, 1, 2, 2, from, res);
}

#[test]
fn exec_from_witness_100times_and_load_after() {
    let from = ExecFrom::Witness;
    let res = Ok(103);
    test_exec(0b0100, 100, 101, 2, from, res);
}

#[test]
fn exec_from_witness_1times_and_load_both_and_write() {
    let from = ExecFrom::Witness;
    let res = Ok(7);
    test_exec(0b0111, 1, 1, 2, from, res);
}

#[test]
fn exec_from_witness_100times_and_load_both_and_write() {
    let from = ExecFrom::Witness;
    let res = Ok(106);
    test_exec(0b0111, 100, 51, 4, from, res);
}
