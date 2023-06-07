use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionBuilder},
    packed::{CellInput, CellOutputBuilder, OutPoint, Script},
};

use super::SCRIPT_VERSION;
use crate::verify::{tests::utils::*, *};

// check_vm_version: vm_version() returns 2.
// check_get_memory_limit: get_memory_limit() returns 8 in prime script.
// check_set_content: set_content() succeed in prime script but write length is 0.
// check_spawn_strcat: a smoking test for spawn().
// check_spawn_strcat_data_hash: position child script by data hash.
// check_spawn_get_memory_limit: call get_memory_limit() in child script.
// check_spawn_set_content: set_content() with content < lenght, = length and > length.
// check_spawn_out_of_cycles: child script out-of-cycles.
// check_spawn_exec: A exec B spawn C.
// check_spawn_strcat_wrap: A spawn B spwan C.
// check_spawn_out_of_cycles_wrap: A spawn B spwan C, but C out-of-cycles.
// check_spawn_recursive: A spawn A spawn A ... ... spawn A
// check_spawn_big_memory_size: fails when memory_limit > 8.
// check_spawn_big_content_length: fails when content_length > 256K.
// check_peak_memory_4m_to_32m: spawn should success when peak memory <= 32M
// check_peak_memory_2m_to_32m: spawn should success when peak memory <= 32M
// check_spawn_snapshot: A spawn B, then B gets suspended to snapshot and resume again.
// check_spawn_state: Like check_spawn_snapshot but invoking verifier.resume_from_state instead.

#[test]
fn check_vm_version() {
    let script_version = SCRIPT_VERSION;

    let (vm_version_cell, vm_version_data_hash) = load_cell_from_path("testdata/vm_version_2");

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
    assert_eq!(result.is_ok(), script_version == ScriptVersion::V2);
}

#[test]
fn check_get_memory_limit() {
    let script_version = SCRIPT_VERSION;

    let (memory_limit_cell, memory_limit_data_hash) =
        load_cell_from_path("testdata/get_memory_limit");

    let memory_limit_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(memory_limit_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(memory_limit_script)
        .build();

    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![memory_limit_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_set_content() {
    let script_version = SCRIPT_VERSION;

    let (set_content_cell, set_content_data_hash) = load_cell_from_path("testdata/set_content");

    let memory_limit_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(set_content_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(memory_limit_script)
        .build();

    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![set_content_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_strcat() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_strcat");
    let (spawn_callee_cell, _spawn_callee_data_hash) =
        load_cell_from_path("testdata/spawn_callee_strcat");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_strcat_data_hash() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_strcat_data_hash");
    let (spawn_callee_cell, _spawn_callee_data_hash) =
        load_cell_from_path("testdata/spawn_callee_strcat");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_get_memory_limit() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_get_memory_limit");
    let (spawn_callee_cell, _spawn_callee_data_hash) =
        load_cell_from_path("testdata/spawn_callee_get_memory_limit");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_set_content() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_set_content");
    let (spawn_callee_cell, _spawn_callee_data_hash) =
        load_cell_from_path("testdata/spawn_callee_set_content");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_out_of_cycles() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_out_of_cycles");
    let (spawn_callee_cell, _spawn_callee_data_hash) =
        load_cell_from_path("testdata/spawn_callee_out_of_cycles");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 0xffffff);
    if script_version >= ScriptVersion::V2 {
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ExceededMaximumCycles"))
    } else {
        assert!(result.is_err())
    }
}

#[test]
fn check_spawn_exec() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_exec");
    let (spawn_callee_caller_cell, _) = load_cell_from_path("testdata/spawn_callee_exec_caller");
    let (spawn_callee_callee_cell, _) = load_cell_from_path("testdata/spawn_callee_exec_callee");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![
            spawn_caller_cell,
            spawn_callee_caller_cell,
            spawn_callee_callee_cell,
        ],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 0xffffff);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_strcat_wrap() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_strcat_wrap");
    let (spawn_callee_caller_cell, _) = load_cell_from_path("testdata/spawn_caller_strcat");
    let (spawn_callee_callee_cell, _) = load_cell_from_path("testdata/spawn_callee_strcat");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![
            spawn_caller_cell,
            spawn_callee_callee_cell,
            spawn_callee_caller_cell,
        ],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 0xffffff);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_out_of_cycles_wrap() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_out_of_cycles_wrap");
    let (spawn_callee_caller_cell, _) = load_cell_from_path("testdata/spawn_caller_out_of_cycles");
    let (spawn_callee_callee_cell, _) = load_cell_from_path("testdata/spawn_callee_out_of_cycles");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![
            spawn_caller_cell,
            spawn_callee_callee_cell,
            spawn_callee_caller_cell,
        ],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 0xffffff);
    if script_version >= ScriptVersion::V2 {
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ExceededMaximumCycles"))
    } else {
        assert!(result.is_err())
    }
}

#[test]
fn check_spawn_recursive() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_recursive");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    if script_version >= ScriptVersion::V2 {
        assert!(result.unwrap_err().to_string().contains("error code 7"))
    } else {
        assert!(result.is_err())
    }
}

#[test]
fn check_spawn_big_memory_size() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_big_memory_size");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_big_content_length() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_big_content_length");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_peak_memory_4m_to_32m() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_peak_memory_4m_to_32m");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_peak_memory_2m_to_32m() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_peak_memory_2m_to_32m");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_snapshot() {
    let script_version = SCRIPT_VERSION;
    if script_version <= ScriptVersion::V1 {
        return;
    }

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_exec");
    let (snapshot_cell, _) = load_cell_from_path("testdata/current_cycles_with_snapshot");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, snapshot_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_pause(script_version, &rtx, Cycle::MAX);
    let cycles_once = result.unwrap();

    let (cycles, chunks_count) = verifier
        .verify_until_completed(script_version, &rtx)
        .unwrap();
    assert_eq!(cycles, cycles_once);
    assert!(chunks_count > 1);
}

#[test]
fn check_spawn_state() {
    let script_version = SCRIPT_VERSION;
    if script_version <= ScriptVersion::V1 {
        return;
    }

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_exec");
    let (snapshot_cell, _) = load_cell_from_path("testdata/current_cycles_with_snapshot");

    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(spawn_caller_script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![spawn_caller_cell, snapshot_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_pause(script_version, &rtx, Cycle::MAX);
    let cycles_once = result.unwrap();

    let (cycles, chunks_count) = verifier
        .verify_map(script_version, &rtx, |verifier| {
            let max_cycles = Cycle::MAX;
            let cycles;
            let mut times = 0usize;
            times += 1;
            let mut init_state = match verifier.resumable_verify(max_cycles).unwrap() {
                VerifyResult::Suspended(state) => Some(state),
                VerifyResult::Completed(cycle) => {
                    cycles = cycle;
                    return Ok((cycles, times));
                }
            };

            loop {
                times += 1;
                let state = init_state.take().unwrap();
                match verifier.resume_from_state(state, max_cycles).unwrap() {
                    VerifyResult::Suspended(state) => {
                        init_state = Some(state);
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
    assert_eq!(cycles, cycles_once);
    assert!(chunks_count > 1);
}
