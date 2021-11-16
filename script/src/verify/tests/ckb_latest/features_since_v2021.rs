use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_CYCLES, TYPE_ID_CODE_HASH};
use ckb_error::assert_error_eq;
use ckb_hash::blake2b_256;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    core::{capacity_bytes, cell::CellMetaBuilder, Capacity, ScriptHashType, TransactionBuilder},
    h256,
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint, Script},
};
use ckb_vm::Error as VmError;
use std::convert::TryInto;
use std::path::Path;

use super::SCRIPT_VERSION;
use crate::{
    type_id::TYPE_ID_CYCLES,
    verify::{tests::utils::*, *},
};

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

    let cpop_lock_cell_data = Bytes::from(
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/cpop_lock")).unwrap(),
    );
    let cpop_lock_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(cpop_lock_cell_data.len()).unwrap().pack())
        .build();
    let cpop_lock_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&cpop_lock_cell_data))
        .args(args)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(cpop_lock_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let cpop_lock_cell = CellMetaBuilder::from_cell_output(cpop_lock_cell, cpop_lock_cell_data)
        .transaction_info(default_transaction_info())
        .build();

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
        let vm_error = VmError::InvalidInstruction(0x60291913);
        let script_error = ScriptError::VMInternalError(format!("{:?}", vm_error));
        assert_error_eq!(result.unwrap_err(), script_error.input_lock_script(0));
    }
}

#[test]
fn test_cycles_difference() {
    let script_version = SCRIPT_VERSION;

    let always_success_cell_data = Bytes::from(
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/mop_adc_lock")).unwrap(),
    );
    let always_success_cell = CellOutput::new_builder()
        .capacity(
            Capacity::bytes(always_success_cell_data.len())
                .unwrap()
                .pack(),
        )
        .build();
    let always_success_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&always_success_cell_data))
        .build();

    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(always_success_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let always_success_cell =
        CellMetaBuilder::from_cell_output(always_success_cell, always_success_cell_data)
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
    let cycles_actual = result.unwrap();
    let cycles_expected = if script_version >= ScriptVersion::V1 {
        686
    } else {
        696
    };
    assert_eq!(cycles_actual, cycles_expected);
}

#[test]
fn check_vm_version() {
    let script_version = SCRIPT_VERSION;

    let vm_version_cell_data = Bytes::from(
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vm_version")).unwrap(),
    );
    let vm_version_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(vm_version_cell_data.len()).unwrap().pack())
        .build();
    let vm_version_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&vm_version_cell_data))
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(vm_version_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let vm_version_cell = CellMetaBuilder::from_cell_output(vm_version_cell, vm_version_cell_data)
        .transaction_info(default_transaction_info())
        .build();

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
fn check_exec_from_cell_data() {
    let script_version = SCRIPT_VERSION;

    let exec_caller_cell_data = Bytes::from(
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_caller_from_cell_data"),
        )
        .unwrap(),
    );
    let exec_caller_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
        .build();

    let exec_callee_cell_data = Bytes::from(
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_callee")).unwrap(),
    );
    let exec_callee_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_callee_cell_data.len()).unwrap().pack())
        .build();

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let exec_caller_cell =
        CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
            .transaction_info(default_transaction_info())
            .build();

    let exec_callee_cell =
        CellMetaBuilder::from_cell_output(exec_callee_cell, exec_callee_cell_data)
            .transaction_info(default_transaction_info())
            .build();

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

    let exec_caller_cell_data = Bytes::from(
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_caller_from_witness"),
        )
        .unwrap(),
    );
    let exec_caller_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
        .build();

    let exec_callee = Bytes::from(
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_callee")).unwrap(),
    )
    .pack();

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default()
        .input(input)
        .set_witnesses(vec![exec_callee])
        .build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let exec_caller_cell =
        CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
            .transaction_info(default_transaction_info())
            .build();

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

    let exec_caller_cell_data = Bytes::from(
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_caller_from_cell_data"),
        )
        .unwrap(),
    );
    let exec_caller_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
        .build();

    let exec_callee_cell_data = Bytes::copy_from_slice(&[0x00, 0x01, 0x02, 0x03]);
    let exec_callee_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_callee_cell_data.len()).unwrap().pack())
        .build();

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let exec_caller_cell =
        CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
            .transaction_info(default_transaction_info())
            .build();

    let exec_callee_cell =
        CellMetaBuilder::from_cell_output(exec_callee_cell, exec_callee_cell_data)
            .transaction_info(default_transaction_info())
            .build();

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

    let exec_caller_cell_data = Bytes::from(
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_caller_big_offset_length"),
        )
        .unwrap(),
    );
    let exec_caller_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
        .build();

    let exec_callee_cell_data = Bytes::copy_from_slice(&[0x00, 0x01, 0x02, 0x03]);
    let exec_callee_cell = CellOutput::new_builder()
        .capacity(Capacity::bytes(exec_callee_cell_data.len()).unwrap().pack())
        .build();

    let exec_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(exec_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();

    let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
        .transaction_info(default_transaction_info())
        .build();
    let exec_caller_cell =
        CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
            .transaction_info(default_transaction_info())
            .build();

    let exec_callee_cell =
        CellMetaBuilder::from_cell_output(exec_callee_cell, exec_callee_cell_data)
            .transaction_info(default_transaction_info())
            .build();

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
        let mut groups: Vec<_> = verifier.groups().collect();
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
                match verifier
                    .verify_group_with_chunk(&group, max, &None)
                    .unwrap()
                {
                    ChunkState::Completed(used_cycles) => {
                        cycles += used_cycles;
                    }
                    ChunkState::Suspended(vm) => {
                        tmp = Some(vm);
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
        let mut groups: Vec<_> = verifier.groups().collect();
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
                    .verify_group_with_chunk(&group, TWO_IN_TWO_OUT_CYCLES / 10, &None)
                    .unwrap()
                {
                    ChunkState::Completed(used_cycles) => {
                        cycles += used_cycles;
                    }
                    ChunkState::Suspended(vm) => {
                        tmp = Some(vm);
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
fn check_resume_from_snapshot() {
    let script_version = SCRIPT_VERSION;

    let (dyn_lib_cell, dyn_lib_data_hash) = {
        let dyn_lib_cell_data = Bytes::from(
            std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("testdata/dyn_load_code/dyn_load_code_lib"),
            )
            .unwrap(),
        );
        let dyn_lib_cell_output = CellOutput::new_builder()
            .capacity(Capacity::bytes(dyn_lib_cell_data.len()).unwrap().pack())
            .build();
        let dyn_lib_data_hash = blake2b_256(&dyn_lib_cell_data);
        let dyn_lib_cell =
            CellMetaBuilder::from_cell_output(dyn_lib_cell_output, dyn_lib_cell_data)
                .transaction_info(default_transaction_info())
                .build();
        (dyn_lib_cell, dyn_lib_data_hash)
    };

    let rtx = {
        let args: packed::Bytes = {
            let number = 0x01u64; // a random odd value

            let mut vec = Vec::with_capacity(8 + dyn_lib_data_hash.len());
            vec.extend_from_slice(&number.to_le_bytes());
            vec.extend_from_slice(&dyn_lib_data_hash);
            vec.pack()
        };

        let dyn_lock_cell_data = Bytes::from(
            std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("testdata/dyn_load_code/dyn_load_code_lock"),
            )
            .unwrap(),
        );
        let dyn_lock_cell_output = CellOutput::new_builder()
            .capacity(Capacity::bytes(dyn_lock_cell_data.len()).unwrap().pack())
            .build();
        let dyn_lock_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(CellOutput::calc_data_hash(&dyn_lock_cell_data))
            .args(args)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(dyn_lock_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input).build();

        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let dyn_lock_cell =
            CellMetaBuilder::from_cell_output(dyn_lock_cell_output, dyn_lock_cell_data)
                .transaction_info(default_transaction_info())
                .build();

        ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dyn_lock_cell, dyn_lib_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        }
    };

    let mut cycles = 0;
    let cycles_step_1 = 100_000;
    let max_cycles = Cycle::MAX;
    let verifier = TransactionScriptsVerifierWithEnv::new();
    // TODO fix resume from snapshot
    let should_be_invalid_permission = script_version <= ScriptVersion::V1;
    let result = verifier.verify_map(script_version, &rtx, |verifier| {
        let mut init_snap: Option<TransactionSnapshot> = None;

        if let VerifyResult::Suspended(state) = verifier.resumable_verify(cycles_step_1).unwrap() {
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
                    cycles = cycle;
                }
            }
        }

        verifier.verify(max_cycles)
    });

    if should_be_invalid_permission {
        return;
    }

    let cycles_once = result.unwrap();
    assert_eq!(cycles, cycles_once);
}
