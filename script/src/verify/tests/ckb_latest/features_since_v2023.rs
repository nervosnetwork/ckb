use super::SCRIPT_VERSION;
use crate::scheduler::{MAX_FDS, MAX_VMS_COUNT};
use crate::syscalls::SOURCE_GROUP_FLAG;
use crate::verify::{tests::utils::*, *};
use ckb_types::{
    core::{Capacity, TransactionBuilder, capacity_bytes, cell::CellMetaBuilder},
    packed::{CellInput, CellOutputBuilder, OutPoint, Script},
};
use proptest::prelude::*;
use proptest::proptest;
use std::collections::{BTreeMap, HashMap};

fn simple_spawn_test(bin_path: &str, args: &[u8]) -> Result<Cycle, Error> {
    let script_version = SCRIPT_VERSION;

    let (cell, data_hash) = load_cell_from_path(bin_path);
    let script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(data_hash)
        .args(Bytes::copy_from_slice(args).pack())
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    verifier.verify_without_limit(script_version, &rtx)
}

#[test]
fn check_spawn_simple_read_write() {
    let result = simple_spawn_test("testdata/spawn_cases", &[1]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_write_dead_lock() {
    let result = simple_spawn_test("testdata/spawn_cases", &[2]);
    assert_eq!(
        result.unwrap_err().to_string().contains("deadlock"),
        SCRIPT_VERSION == ScriptVersion::V2
    );
}

#[test]
fn check_spawn_invalid_fd() {
    let result = simple_spawn_test("testdata/spawn_cases", &[3]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_wait_dead_lock() {
    let result = simple_spawn_test("testdata/spawn_cases", &[4]);
    assert_eq!(
        result.unwrap_err().to_string().contains("deadlock"),
        SCRIPT_VERSION == ScriptVersion::V2
    );
}

#[test]
fn check_spawn_read_write_with_close() {
    let result = simple_spawn_test("testdata/spawn_cases", &[5]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_wait_multiple() {
    let result = simple_spawn_test("testdata/spawn_cases", &[6]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_inherited_fds() {
    let result = simple_spawn_test("testdata/spawn_cases", &[7]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_inherited_fds_without_owner() {
    let result = simple_spawn_test("testdata/spawn_cases", &[8]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_read_then_close() {
    let result = simple_spawn_test("testdata/spawn_cases", &[9]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_max_vms_count() {
    let result = simple_spawn_test("testdata/spawn_cases", &[10]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_max_fds_limit() {
    let result = simple_spawn_test("testdata/spawn_cases", &[11]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ExceededMaximumCycles")
        )
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
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ExceededMaximumCycles")
        )
    } else {
        assert!(result.is_err());
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
    let result = verifier.verify(script_version, &rtx, 70_000_000);
    if script_version >= ScriptVersion::V2 {
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("error code 8"))
    } else {
        assert!(result.is_err())
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_spawn_async() {
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

    // we use debug pause to test context resume
    // `current_cycles_with_snapshot` will try to pause verifier
    // here we use `channel` to send Resume to verifier until it completes
    let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);
    let _jt = tokio::spawn(async move {
        loop {
            let res = command_tx.send(ChunkCommand::Resume);
            if res.is_err() {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        }
    });
    let cycles = verifier
        .verify_complete_async(script_version, &rtx, &mut command_rx, false, None)
        .await
        .unwrap();
    assert_eq!(cycles, cycles_once);

    // we send Resume/Suspend to command_rx in a loop, make sure cycles is still the same
    let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);
    let _jt = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            let _res = command_tx.send(ChunkCommand::Suspend);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            let _res = command_tx.send(ChunkCommand::Resume);
            tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

            let _res = command_tx.send(ChunkCommand::Suspend);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            let _res = command_tx.send(ChunkCommand::Resume);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    let cycles = verifier
        .verify_complete_async(script_version, &rtx, &mut command_rx, true, None)
        .await
        .unwrap();
    assert_eq!(cycles, cycles_once);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_spawn_suspend_shutdown() {
    let script_version = SCRIPT_VERSION;
    if script_version <= ScriptVersion::V1 {
        return;
    }

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_exec");
    let (snapshot_cell, _) = load_cell_from_path("testdata/infinite_loop");

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
    let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);
    let _jt = tokio::spawn(async move {
        loop {
            let _res = command_tx.send(ChunkCommand::Suspend);
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

            let _res = command_tx.send(ChunkCommand::Resume);
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

            let _res = command_tx.send(ChunkCommand::Suspend);
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

            let _res = command_tx.send(ChunkCommand::Stop);
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
        }
    });

    let res = verifier
        .verify_complete_async(script_version, &rtx, &mut command_rx, true, None)
        .await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.to_string().contains("VM Interrupts"));

    let reject = ckb_types::core::tx_pool::Reject::Verification(err);
    assert!(!reject.is_malformed_tx());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn check_run_vm_with_pause_and_max_cycles() {
    let script_version = SCRIPT_VERSION;
    if script_version <= ScriptVersion::V1 {
        return;
    }

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_exec");
    let (snapshot_cell, _) = load_cell_from_path("testdata/infinite_loop");

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
    let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);

    let _jt = tokio::spawn(async move {
        loop {
            let _res = command_tx.send(ChunkCommand::Resume);
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    });

    let res = verifier
        .verify_complete_async(script_version, &rtx, &mut command_rx, true, Some(10000))
        .await;

    let err = res.unwrap_err();
    assert!(
        err.to_string()
            .contains("ExceededMaximumCycles: expect cycles <= 10000")
    );
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
                let state: TransactionState = init_state.take().unwrap();
                match verifier.resume_from_state(&state, max_cycles).unwrap() {
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

#[test]
fn check_spawn_current_cycles() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_caller_current_cycles");
    let (spawn_callee_cell, _spawn_callee_data_hash) =
        load_cell_from_path("testdata/spawn_callee_current_cycles");

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

#[derive(Clone, Copy)]
enum SpawnFrom {
    TxInputWitness,
    GroupInputWitness,
    TxOutputWitness,
    GroupOutputWitness,
    TxCellDep,
    TxInputCell,
    TxOutputCell,
    GroupInputCell,
    GroupOutputCell,
    Slice(u64, u64),
}

fn check_spawn_configurable_once(spawn_from: SpawnFrom) {
    let script_version = SCRIPT_VERSION;

    let args = {
        let mut args: Vec<u8> = vec![];
        let position = match spawn_from {
            SpawnFrom::TxInputWitness => vec![0, 1, 1, 0],
            SpawnFrom::GroupInputWitness => vec![0, SOURCE_GROUP_FLAG | 1, 1, 0],
            SpawnFrom::TxOutputWitness => vec![0, 2, 1, 0],
            SpawnFrom::GroupOutputWitness => vec![0, SOURCE_GROUP_FLAG | 2, 1, 0],
            SpawnFrom::TxCellDep => vec![1, 3, 0, 0],
            SpawnFrom::TxInputCell => vec![1, 1, 0, 0],
            SpawnFrom::TxOutputCell => vec![0, 2, 0, 0],
            SpawnFrom::GroupInputCell => vec![0, SOURCE_GROUP_FLAG | 1, 0, 0],
            SpawnFrom::GroupOutputCell => vec![0, SOURCE_GROUP_FLAG | 2, 0, 0],
            SpawnFrom::Slice(offset, size) => {
                let (spawn_callee_cell, _) =
                    load_cell_from_path("testdata/spawn_configurable_callee");
                let h = offset << 32;
                let l = if size == 0 {
                    0
                } else {
                    spawn_callee_cell.mem_cell_data.unwrap().len() as u64
                };
                vec![0, 1, 1, h | l]
            }
        };
        for e in position {
            args.extend(e.to_le_bytes());
        }
        args
    };

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_configurable_caller");
    let (spawn_callee_cell, _) = load_cell_from_path("testdata/spawn_configurable_callee");
    let (always_success_cell, always_success_data_hash) =
        load_cell_from_path("testdata/always_success");
    let spawn_callee_cell_data = spawn_callee_cell.mem_cell_data.as_ref().unwrap();
    let spawn_caller_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(spawn_caller_data_hash)
        .args(args.pack())
        .build();
    let always_success_script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(always_success_data_hash)
        .build();

    let input_spawn_caller = create_dummy_cell(
        CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(spawn_caller_script.clone())
            .build(),
    );

    let rtx = match spawn_from {
        SpawnFrom::TxInputWitness | SpawnFrom::TxOutputWitness | SpawnFrom::GroupInputWitness => {
            ResolvedTransaction {
                transaction: TransactionBuilder::default()
                    .set_witnesses(vec![spawn_callee_cell_data.pack()])
                    .build(),
                resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
                resolved_inputs: vec![input_spawn_caller],
                resolved_dep_groups: vec![],
            }
        }
        SpawnFrom::GroupOutputWitness => ResolvedTransaction {
            transaction: TransactionBuilder::default()
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(100).pack())
                        .type_(Some(spawn_caller_script).pack())
                        .build(),
                )
                .set_witnesses(vec![spawn_callee_cell_data.pack()])
                .build(),
            resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
            resolved_inputs: vec![],
            resolved_dep_groups: vec![],
        },
        SpawnFrom::TxCellDep => ResolvedTransaction {
            transaction: TransactionBuilder::default().build(),
            resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
            resolved_inputs: vec![input_spawn_caller],
            resolved_dep_groups: vec![],
        },
        SpawnFrom::TxInputCell => {
            let input_spawn_callee_output = CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000).pack())
                .lock(always_success_script)
                .build();
            let input_spawn_callee = CellMetaBuilder::from_cell_output(
                input_spawn_callee_output,
                spawn_callee_cell_data.clone(),
            )
            .build();
            ResolvedTransaction {
                transaction: TransactionBuilder::default().build(),
                resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell, always_success_cell],
                resolved_inputs: vec![input_spawn_caller, input_spawn_callee],
                resolved_dep_groups: vec![],
            }
        }
        SpawnFrom::TxOutputCell => ResolvedTransaction {
            transaction: TransactionBuilder::default()
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(100).pack())
                        .lock(always_success_script)
                        .build(),
                )
                .output_data(spawn_callee_cell_data.pack())
                .build(),
            resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell, always_success_cell],
            resolved_inputs: vec![input_spawn_caller],
            resolved_dep_groups: vec![],
        },
        SpawnFrom::GroupInputCell => {
            let input_spawn_caller = CellMetaBuilder::from_cell_output(
                CellOutputBuilder::default()
                    .capacity(capacity_bytes!(100).pack())
                    .lock(spawn_caller_script)
                    .build(),
                spawn_callee_cell_data.clone(),
            )
            .build();
            ResolvedTransaction {
                transaction: TransactionBuilder::default().build(),
                resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell, always_success_cell],
                resolved_inputs: vec![input_spawn_caller],
                resolved_dep_groups: vec![],
            }
        }
        SpawnFrom::GroupOutputCell => ResolvedTransaction {
            transaction: TransactionBuilder::default()
                .output(
                    CellOutputBuilder::default()
                        .capacity(capacity_bytes!(100).pack())
                        .type_(Some(spawn_caller_script).pack())
                        .build(),
                )
                .output_data(spawn_callee_cell_data.pack())
                .build(),
            resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell, always_success_cell],
            resolved_inputs: vec![],
            resolved_dep_groups: vec![],
        },
        SpawnFrom::Slice(offset, size) => {
            let mut data = vec![0; offset as usize];
            data.extend(spawn_callee_cell_data);
            if size != 0 {
                data.extend(vec![0; 0x12]);
            }
            ResolvedTransaction {
                transaction: TransactionBuilder::default()
                    .set_witnesses(vec![data.pack()])
                    .build(),
                resolved_cell_deps: vec![spawn_caller_cell, spawn_callee_cell],
                resolved_inputs: vec![input_spawn_caller],
                resolved_dep_groups: vec![],
            }
        }
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify_without_limit(script_version, &rtx);
    assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
}

#[test]
fn check_spawn_configurable() {
    check_spawn_configurable_once(SpawnFrom::TxInputWitness);
    check_spawn_configurable_once(SpawnFrom::GroupInputWitness);
    check_spawn_configurable_once(SpawnFrom::TxOutputWitness);
    check_spawn_configurable_once(SpawnFrom::GroupOutputWitness);
    check_spawn_configurable_once(SpawnFrom::TxCellDep);
    check_spawn_configurable_once(SpawnFrom::TxInputCell);
    check_spawn_configurable_once(SpawnFrom::TxOutputCell);
    check_spawn_configurable_once(SpawnFrom::GroupInputCell);
    check_spawn_configurable_once(SpawnFrom::GroupOutputCell);
    check_spawn_configurable_once(SpawnFrom::Slice(0, 0));
    check_spawn_configurable_once(SpawnFrom::Slice(1, 0));
    check_spawn_configurable_once(SpawnFrom::Slice(0, 1));
    check_spawn_configurable_once(SpawnFrom::Slice(1, 1));
}

#[allow(dead_code)]
#[path = "../../../../testdata/spawn_dag.rs"]
mod spawn_dag;
use ckb_types::bytes::Bytes;
use daggy::{Dag, Walker};
use molecule::prelude::Byte;
use rand::{Rng, SeedableRng, rngs::StdRng};
use spawn_dag as dag;
use std::collections::{HashSet, VecDeque};

pub fn generate_data_graph(
    seed: u64,
    spawns: u32,
    writes: u32,
    converging_threshold: u32,
) -> Result<dag::Data, Error> {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut spawn_dag: Dag<(), ()> = Dag::new();
    let mut write_dag: Dag<(), ()> = Dag::new();

    // Root node denoting entrypoint VM
    let spawn_root = spawn_dag.add_node(());
    let write_root = write_dag.add_node(());
    assert_eq!(spawn_root.index(), 0);
    assert_eq!(write_root.index(), 0);

    let mut spawn_nodes = vec![spawn_root];
    let mut write_nodes = vec![write_root];

    for _ in 1..=spawns {
        let write_node = write_dag.add_node(());
        write_nodes.push(write_node);

        let previous_node = spawn_nodes[rng.gen_range(0..spawn_nodes.len())];
        let (_, spawn_node) = spawn_dag.add_child(previous_node, (), ());
        spawn_nodes.push(spawn_node);
    }

    let mut write_edges = Vec::new();
    if spawns > 0 {
        for _ in 1..=writes {
            let mut updated = false;

            for _ in 0..converging_threshold {
                let first_index = rng.gen_range(0..write_nodes.len());
                let second_index = {
                    let mut i = first_index;
                    while i == first_index {
                        i = rng.gen_range(0..write_nodes.len());
                    }
                    i
                };

                let first_node = write_nodes[first_index];
                let second_node = write_nodes[second_index];

                if let Ok(e) = write_dag.add_edge(first_node, second_node, ()) {
                    write_edges.push(e);
                    updated = true;
                    break;
                }
            }

            if !updated {
                break;
            }
        }
    }

    // Edge index -> pipe indices. Daggy::edge_endpoints helps us finding
    // nodes (vms) from edges (spawns)
    let mut spawn_ops: HashMap<usize, Vec<usize>> = HashMap::default();
    // Node index -> created pipes
    let mut pipes_ops: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::default();

    let mut spawn_edges = Vec::new();
    // Traversing spawn_dag for spawn operations
    let mut processing = VecDeque::from([spawn_root]);
    while !processing.is_empty() {
        let node = processing.pop_front().unwrap();
        pipes_ops.insert(node.index(), Vec::new());
        let children: Vec<_> = spawn_dag.children(node).iter(&spawn_dag).collect();
        for (e, n) in children.into_iter().rev() {
            spawn_ops.insert(e.index(), Vec::new());
            spawn_edges.push(e);

            processing.push_back(n);
        }
    }

    let mut writes_builder = dag::WritesBuilder::default();
    // Traversing all edges in write_dag
    for e in write_edges {
        let (writer, reader) = write_dag.edge_endpoints(e).unwrap();
        assert_ne!(writer, reader);
        let writer_pipe_index = e.index() * 2 + 1;
        let reader_pipe_index = e.index() * 2;

        // Generate finalized write op
        {
            let data_len = rng.gen_range(1..=1024);
            let mut data = vec![0u8; data_len];
            rng.fill(&mut data[..]);

            writes_builder = writes_builder.push(
                dag::WriteBuilder::default()
                    .from(build_vm_index(writer.index() as u64))
                    .from_fd(build_fd_index(writer_pipe_index as u64))
                    .to(build_vm_index(reader.index() as u64))
                    .to_fd(build_fd_index(reader_pipe_index as u64))
                    .data(
                        dag::BytesBuilder::default()
                            .extend(data.iter().map(|b| Byte::new(*b)))
                            .build(),
                    )
                    .build(),
            );
        }

        // Finding the lowest common ancestor of writer & reader nodes
        // in spawn_dag, which will creates the pair of pipes. Note that
        // all traversed spawn edges will have to pass the pipes down.
        //
        // TODO: we use a simple yet slow LCA solution, a faster algorithm
        // can be used to replace the code here if needed.
        let ancestor = {
            let mut a = writer;
            let mut b = reader;

            let mut set_a = HashSet::new();
            set_a.insert(a);
            let mut set_b = HashSet::new();
            set_b.insert(b);

            loop {
                let parents_a: Vec<_> = spawn_dag.parents(a).iter(&spawn_dag).collect();
                let parents_b: Vec<_> = spawn_dag.parents(b).iter(&spawn_dag).collect();

                assert!(
                    ((parents_a.len() == 1) && (parents_b.len() == 1))
                        || (parents_a.is_empty() && (parents_b.len() == 1))
                        || ((parents_a.len() == 1) && parents_b.is_empty())
                );

                // Update spawn ops to pass down pipes via edges, also update
                // each node's path node list
                if parents_a.len() == 1 {
                    let (_, parent_a) = parents_a[0];
                    set_a.insert(parent_a);

                    a = parent_a;
                }
                if parents_b.len() == 1 {
                    let (_, parent_b) = parents_b[0];
                    set_b.insert(parent_b);

                    b = parent_b;
                }

                // Test for ancestor
                if parents_a.len() == 1 {
                    let (_, parent_a) = parents_a[0];
                    if set_b.contains(&parent_a) {
                        break parent_a;
                    }
                }
                if parents_b.len() == 1 {
                    let (_, parent_b) = parents_b[0];
                    if set_a.contains(&parent_b) {
                        break parent_b;
                    }
                }
            }
        };

        // Update the path from each node to the LCA so we can pass created
        // pipes from LCA to each node
        {
            let mut a = writer;
            while a != ancestor {
                let parents_a: Vec<_> = spawn_dag.parents(a).iter(&spawn_dag).collect();
                assert!(parents_a.len() == 1);
                let (edge_a, parent_a) = parents_a[0];
                spawn_ops
                    .get_mut(&edge_a.index())
                    .unwrap()
                    .push(writer_pipe_index);
                a = parent_a;
            }

            let mut b = reader;
            while b != ancestor {
                let parents_b: Vec<_> = spawn_dag.parents(b).iter(&spawn_dag).collect();
                assert!(parents_b.len() == 1);
                let (edge_b, parent_b) = parents_b[0];
                spawn_ops
                    .get_mut(&edge_b.index())
                    .unwrap()
                    .push(reader_pipe_index);
                b = parent_b;
            }
        }

        // Create the pipes at the ancestor node
        pipes_ops
            .get_mut(&ancestor.index())
            .unwrap()
            .push((reader_pipe_index, writer_pipe_index));
    }

    let mut spawns_builder = dag::SpawnsBuilder::default();
    for e in spawn_edges {
        let (parent, child) = spawn_dag.edge_endpoints(e).unwrap();

        let pipes = {
            let mut builder = dag::FdIndicesBuilder::default();
            for p in &spawn_ops[&e.index()] {
                builder = builder.push(build_fd_index(*p as u64));
            }
            builder.build()
        };

        spawns_builder = spawns_builder.push(
            dag::SpawnBuilder::default()
                .from(build_vm_index(parent.index() as u64))
                .child(build_vm_index(child.index() as u64))
                .fds(pipes)
                .build(),
        );
    }

    let mut pipes_builder = dag::PipesBuilder::default();
    for (vm_index, pairs) in pipes_ops {
        for (reader_pipe_index, writer_pipe_index) in pairs {
            pipes_builder = pipes_builder.push(
                dag::PipeBuilder::default()
                    .vm(build_vm_index(vm_index as u64))
                    .read_fd(build_fd_index(reader_pipe_index as u64))
                    .write_fd(build_fd_index(writer_pipe_index as u64))
                    .build(),
            );
        }
    }

    Ok(dag::DataBuilder::default()
        .spawns(spawns_builder.build())
        .pipes(pipes_builder.build())
        .writes(writes_builder.build())
        .build())
}

fn build_vm_index(val: u64) -> dag::VmIndex {
    let mut data = [Byte::new(0); 8];
    for (i, v) in val.to_le_bytes().into_iter().enumerate() {
        data[i] = Byte::new(v);
    }
    dag::VmIndexBuilder::default().set(data).build()
}

fn build_fd_index(val: u64) -> dag::FdIndex {
    let mut data = [Byte::new(0); 8];
    for (i, v) in val.to_le_bytes().into_iter().enumerate() {
        data[i] = Byte::new(v);
    }
    dag::FdIndexBuilder::default().set(data).build()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn test_random_dag(
        seed: u64,
        spawns in 5u32..MAX_VMS_COUNT as u32,
        writes in 3u32..MAX_FDS as u32 / 2,
    ) {
        let script_version = SCRIPT_VERSION;
        let program: Bytes = std::fs::read("./testdata/spawn_dag").unwrap().into();
        let data = generate_data_graph(seed, spawns, writes, 3).unwrap();

        let (code_dep, code_dep_hash) = load_cell_from_slice(&program[..]);
        let spawn_caller_script = Script::new_builder()
            .hash_type(script_version.data_hash_type().into())
            .code_hash(code_dep_hash)
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(spawn_caller_script)
            .build();
        let dummy_cell = create_dummy_cell(output);

        let rtx = ResolvedTransaction {
            transaction: TransactionBuilder::default().witness(data.as_bytes().pack()).build(),
            resolved_cell_deps: vec![code_dep],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let verifier = TransactionScriptsVerifierWithEnv::new();
        let result = verifier.verify_without_limit(script_version, &rtx);
        assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
    }
}

#[test]
fn check_spawn_close_invalid_fd() {
    let result = simple_spawn_test("testdata/spawn_cases", &[12]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_write_closed_fd() {
    let result = simple_spawn_test("testdata/spawn_cases", &[13]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_pid() {
    let result = simple_spawn_test("testdata/spawn_cases", &[14]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_offset_out_of_bound() {
    let result = simple_spawn_test("testdata/spawn_cases", &[15]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_length_out_of_bound() {
    let result = simple_spawn_test("testdata/spawn_cases", &[16]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_huge_swap() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) =
        load_cell_from_path("testdata/spawn_huge_swap");

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

    let tic = std::time::Instant::now();
    let result = verifier.verify(script_version, &rtx, 70_000_000);
    let toc = tic.elapsed().as_millis();
    if script_version >= ScriptVersion::V2 {
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("ExceededMaximumCycles"));
        // Normally, this test should take less than 1 second.
        assert!(toc < 5000);
    } else {
        assert!(result.is_err())
    }
}

#[test]
fn check_spawn_invaild_index() {
    let result = simple_spawn_test("testdata/spawn_cases", &[17]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_index_out_of_bound() {
    let result = simple_spawn_test("testdata/spawn_cases", &[18]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_root_inherited_fds() {
    let result = simple_spawn_test("testdata/spawn_cases", &[19]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_spawn_cycles() {
    let script_version = SCRIPT_VERSION;

    let (spawn_caller_cell, spawn_caller_data_hash) = load_cell_from_path("testdata/spawn_cycles");
    let (spawn_callee_cell, _spawn_callee_data_hash) = load_cell_from_path("testdata/spawn_cycles");

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
    if script_version >= ScriptVersion::V2 {
        assert_eq!(result.unwrap(), 1525087);
    }
}

fn spawn_io_test(io_size: u64, enable_check: bool) -> Result<u64, Error> {
    let script_version = SCRIPT_VERSION;

    let mut args = vec![0u8; 16];
    args[..8].copy_from_slice(&io_size.to_le_bytes());
    args[8] = enable_check as u8;

    let (cell, data_hash) = load_cell_from_path("testdata/spawn_io_cycles");
    let script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(data_hash)
        .args(Bytes::copy_from_slice(&args).pack())
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    verifier.verify_without_limit(script_version, &rtx)
}

#[test]
fn check_spawn_io_cycles() {
    if SCRIPT_VERSION != ScriptVersion::V2 {
        return;
    }

    let offset_size = 1024;
    let r = spawn_io_test(128, true);
    r.unwrap();
    let r = spawn_io_test(128 + offset_size, true);
    r.unwrap();

    let r = spawn_io_test(128, false);
    let cycles1 = r.unwrap();
    let r = spawn_io_test(128 + offset_size, false);
    let cycles2 = r.unwrap();

    assert_eq!(cycles2 - cycles1, offset_size / 2);
}

#[test]
fn check_spawn_saturate_memory() {
    let result = simple_spawn_test("testdata/spawn_saturate_memory", &[0]);
    assert_eq!(result.is_ok(), SCRIPT_VERSION == ScriptVersion::V2);
}

#[test]
fn check_infinite_exec() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) = load_cell_from_path("testdata/infinite_exec");
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
        resolved_cell_deps: vec![exec_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 70000000);
    if script_version >= ScriptVersion::V1 {
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("ExceededMaximumCycles")
        )
    } else {
        assert!(result.is_err())
    }
}

#[test]
fn check_fuzz_crash_1() {
    let script_version = SCRIPT_VERSION;

    let (exec_caller_cell, exec_caller_data_hash) = load_cell_from_path("testdata/crash-5a27052f");
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
        resolved_cell_deps: vec![exec_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };

    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 70000000);
    match script_version {
        ScriptVersion::V0 => assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("MemWriteOnExecutablePage")
        ),
        ScriptVersion::V1 | ScriptVersion::V2 => assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("SourceEntry parse_from_u64 0")
        ),
    }
}

#[test]
fn check_fuzz_crash_2() {
    let script_version = SCRIPT_VERSION;
    let (exec_caller_cell, exec_caller_data_hash) = load_cell_from_path("testdata/crash-45a6098d");
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
        resolved_cell_deps: vec![exec_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 70000000);
    match script_version {
        ScriptVersion::V0 => assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("MemWriteOnExecutablePage")
        ),
        ScriptVersion::V1 => assert_eq!(result.unwrap(), 58741),
        ScriptVersion::V2 => assert_eq!(result.unwrap(), 58686),
    }
}

#[test]
fn check_fuzz_crash_3() {
    let script_version = SCRIPT_VERSION;
    let (exec_caller_cell, exec_caller_data_hash) = load_cell_from_path("testdata/crash-4717eb0e");
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
        resolved_cell_deps: vec![exec_caller_cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let result = verifier.verify(script_version, &rtx, 70000000);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("MemWriteOnExecutablePage")
    );
}

// This test documents a bug in Meepo hardfork version: when IO processing
// code suspends or resumes any VMs, the cycles consumed by suspending / resuming
// VMs will not be reflected by `current cycles` syscall in the immediate
// subsequent VM execution. Here we are asserting the exact cycles consumed
// by a program touching this behavior, so as to prevent any future regressions.
#[test]
fn spawn_create_17_spawn() {
    if SCRIPT_VERSION < ScriptVersion::V2 {
        return;
    }
    let script_version = ScriptVersion::V2;

    let (cell, data_hash) = load_cell_from_path("testdata/spawn_create_17_spawn");
    let script = Script::new_builder()
        .hash_type(script_version.data_hash_type().into())
        .code_hash(data_hash)
        .build();
    let output = CellOutputBuilder::default()
        .capacity(capacity_bytes!(100).pack())
        .lock(script)
        .build();
    let input = CellInput::new(OutPoint::null(), 0);

    let transaction = TransactionBuilder::default().input(input).build();
    let dummy_cell = create_dummy_cell(output);

    let rtx = ResolvedTransaction {
        transaction,
        resolved_cell_deps: vec![cell],
        resolved_inputs: vec![dummy_cell],
        resolved_dep_groups: vec![],
    };
    let verifier = TransactionScriptsVerifierWithEnv::new();
    let cycles = verifier
        .verify_without_limit(script_version, &rtx)
        .expect("verify");

    assert_eq!(cycles, 36445673);
}
