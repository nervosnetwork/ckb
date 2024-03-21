use ckb_types::{
    core::{capacity_bytes, Capacity, TransactionBuilder},
    packed::{CellInput, CellOutputBuilder, OutPoint, Script},
};

use super::SCRIPT_VERSION;
use crate::verify::{tests::utils::*, *};

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
    println!("result = {:?}", result);
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

// #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// async fn check_spawn_async() {
//     let script_version = SCRIPT_VERSION;
//     if script_version <= ScriptVersion::V1 {
//         return;
//     }

//     let (spawn_caller_cell, spawn_caller_data_hash) =
//         load_cell_from_path("testdata/spawn_caller_exec");
//     let (snapshot_cell, _) = load_cell_from_path("testdata/current_cycles_with_snapshot");

//     let spawn_caller_script = Script::new_builder()
//         .hash_type(script_version.data_hash_type().into())
//         .code_hash(spawn_caller_data_hash)
//         .build();
//     let output = CellOutputBuilder::default()
//         .capacity(capacity_bytes!(100).pack())
//         .lock(spawn_caller_script)
//         .build();
//     let input = CellInput::new(OutPoint::null(), 0);

//     let transaction = TransactionBuilder::default().input(input).build();
//     let dummy_cell = create_dummy_cell(output);

//     let rtx = ResolvedTransaction {
//         transaction,
//         resolved_cell_deps: vec![spawn_caller_cell, snapshot_cell],
//         resolved_inputs: vec![dummy_cell],
//         resolved_dep_groups: vec![],
//     };
//     let verifier = TransactionScriptsVerifierWithEnv::new();
//     let result = verifier.verify_without_pause(script_version, &rtx, Cycle::MAX);
//     let cycles_once = result.unwrap();

//     // we use debug pause to test context resume
//     // `current_cycles_with_snapshot` will try to pause verifier
//     // here we use `channel` to send Resume to verifier until it completes
//     let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);
//     let _jt = tokio::spawn(async move {
//         loop {
//             let res = command_tx.send(ChunkCommand::Resume);
//             if res.is_err() {
//                 break;
//             }
//             tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
//         }
//     });
//     let cycles = verifier
//         .verify_complete_async(script_version, &rtx, &mut command_rx, false)
//         .await
//         .unwrap();
//     assert_eq!(cycles, cycles_once);

//     // we send Resume/Suspend to command_rx in a loop, make sure cycles is still the same
//     let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);
//     let _jt = tokio::spawn(async move {
//         loop {
//             tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
//             let _res = command_tx.send(ChunkCommand::Suspend);
//             tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

//             let _res = command_tx.send(ChunkCommand::Resume);
//             tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;

//             let _res = command_tx.send(ChunkCommand::Suspend);
//             tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

//             let _res = command_tx.send(ChunkCommand::Resume);
//             tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
//         }
//     });

//     let cycles = verifier
//         .verify_complete_async(script_version, &rtx, &mut command_rx, true)
//         .await
//         .unwrap();
//     assert_eq!(cycles, cycles_once);
// }

// #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
// async fn check_spawn_suspend_shutdown() {
//     let script_version = SCRIPT_VERSION;
//     if script_version <= ScriptVersion::V1 {
//         return;
//     }

//     let (spawn_caller_cell, spawn_caller_data_hash) =
//         load_cell_from_path("testdata/spawn_caller_exec");
//     let (snapshot_cell, _) = load_cell_from_path("testdata/infinite_loop");

//     let spawn_caller_script = Script::new_builder()
//         .hash_type(script_version.data_hash_type().into())
//         .code_hash(spawn_caller_data_hash)
//         .build();
//     let output = CellOutputBuilder::default()
//         .capacity(capacity_bytes!(100).pack())
//         .lock(spawn_caller_script)
//         .build();
//     let input = CellInput::new(OutPoint::null(), 0);

//     let transaction = TransactionBuilder::default().input(input).build();
//     let dummy_cell = create_dummy_cell(output);

//     let rtx = ResolvedTransaction {
//         transaction,
//         resolved_cell_deps: vec![spawn_caller_cell, snapshot_cell],
//         resolved_inputs: vec![dummy_cell],
//         resolved_dep_groups: vec![],
//     };

//     let verifier = TransactionScriptsVerifierWithEnv::new();
//     let (command_tx, mut command_rx) = watch::channel(ChunkCommand::Resume);
//     let _jt = tokio::spawn(async move {
//         loop {
//             let _res = command_tx.send(ChunkCommand::Suspend);
//             tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

//             let _res = command_tx.send(ChunkCommand::Resume);
//             tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

//             let _res = command_tx.send(ChunkCommand::Suspend);
//             tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

//             let _res = command_tx.send(ChunkCommand::Stop);
//             tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
//         }
//     });

//     let res = verifier
//         .verify_complete_async(script_version, &rtx, &mut command_rx, true)
//         .await;
//     assert!(res.is_err());
//     let err = res.unwrap_err().to_string();
//     assert!(err.contains("VM Internal Error: External(\"stopped\")"));
// }

// #[test]
// fn check_spawn_state() {
//     let script_version = SCRIPT_VERSION;
//     if script_version <= ScriptVersion::V1 {
//         return;
//     }

//     let (spawn_caller_cell, spawn_caller_data_hash) =
//         load_cell_from_path("testdata/spawn_caller_exec");
//     let (snapshot_cell, _) = load_cell_from_path("testdata/current_cycles_with_snapshot");

//     let spawn_caller_script = Script::new_builder()
//         .hash_type(script_version.data_hash_type().into())
//         .code_hash(spawn_caller_data_hash)
//         .build();
//     let output = CellOutputBuilder::default()
//         .capacity(capacity_bytes!(100).pack())
//         .lock(spawn_caller_script)
//         .build();
//     let input = CellInput::new(OutPoint::null(), 0);

//     let transaction = TransactionBuilder::default().input(input).build();
//     let dummy_cell = create_dummy_cell(output);

//     let rtx = ResolvedTransaction {
//         transaction,
//         resolved_cell_deps: vec![spawn_caller_cell, snapshot_cell],
//         resolved_inputs: vec![dummy_cell],
//         resolved_dep_groups: vec![],
//     };
//     let verifier = TransactionScriptsVerifierWithEnv::new();
//     let result = verifier.verify_without_pause(script_version, &rtx, Cycle::MAX);
//     let cycles_once = result.unwrap();

//     let (cycles, chunks_count) = verifier
//         .verify_map(script_version, &rtx, |verifier| {
//             let max_cycles = Cycle::MAX;
//             let cycles;
//             let mut times = 0usize;
//             times += 1;
//             let mut init_state = match verifier.resumable_verify(max_cycles).unwrap() {
//                 VerifyResult::Suspended(state) => Some(state),
//                 VerifyResult::Completed(cycle) => {
//                     cycles = cycle;
//                     return Ok((cycles, times));
//                 }
//             };

//             loop {
//                 times += 1;
//                 let state = init_state.take().unwrap();
//                 match verifier.resume_from_state(state, max_cycles).unwrap() {
//                     VerifyResult::Suspended(state) => {
//                         init_state = Some(state);
//                     }
//                     VerifyResult::Completed(cycle) => {
//                         cycles = cycle;
//                         break;
//                     }
//                 }
//             }

//             Ok::<(u64, usize), Error>((cycles, times))
//         })
//         .unwrap();
//     assert_eq!(cycles, cycles_once);
//     assert!(chunks_count > 1);
// }

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

// TODO: will reach memory limit
// #[test]
// fn check_spawn_times_bug_1() {
//     let script_version = SCRIPT_VERSION;

//     let (spawn_caller_cell, spawn_caller_data_hash) = load_cell_from_path("testdata/spawn_times");

//     let spawn_caller_script = Script::new_builder()
//         .hash_type(script_version.data_hash_type().into())
//         .code_hash(spawn_caller_data_hash)
//         .build();
//     let output = CellOutputBuilder::default()
//         .capacity(capacity_bytes!(100).pack())
//         .lock(spawn_caller_script)
//         .build();
//     let input = CellInput::new(OutPoint::null(), 0);

//     let transaction = TransactionBuilder::default().input(input).build();
//     let dummy_cell = create_dummy_cell(output);

//     let rtx = ResolvedTransaction {
//         transaction,
//         resolved_cell_deps: vec![spawn_caller_cell],
//         resolved_inputs: vec![dummy_cell],
//         resolved_dep_groups: vec![],
//     };
//     let verifier = TransactionScriptsVerifierWithEnv::new();
//     let result = verifier.verify_without_limit(script_version, &rtx);
//     let err = result.unwrap_err();
//     println!("{:?}", err);
//     // assert_eq!(result.is_ok(), script_version >= ScriptVersion::V2);
// }

// #[test]
// fn check_spawn_times_bug_2() {
//     let script_version = SCRIPT_VERSION;
//     if script_version <= ScriptVersion::V1 {
//         return;
//     }
//     let (spawn_caller_cell, spawn_caller_data_hash) = load_cell_from_path("testdata/spawn_times");

//     let spawn_caller_script = Script::new_builder()
//         .hash_type(script_version.data_hash_type().into())
//         .code_hash(spawn_caller_data_hash)
//         .build();
//     let output = CellOutputBuilder::default()
//         .capacity(capacity_bytes!(100).pack())
//         .lock(spawn_caller_script)
//         .build();
//     let input = CellInput::new(OutPoint::null(), 0);

//     let transaction = TransactionBuilder::default().input(input).build();
//     let dummy_cell = create_dummy_cell(output);

//     let rtx = ResolvedTransaction {
//         transaction,
//         resolved_cell_deps: vec![spawn_caller_cell],
//         resolved_inputs: vec![dummy_cell],
//         resolved_dep_groups: vec![],
//     };
//     let verifier = TransactionScriptsVerifierWithEnv::new();
//     let result = verifier.verify_without_pause(script_version, &rtx, Cycle::MAX);
//     let cycles_once = result.unwrap();

//     let (cycles, _) = verifier
//         .verify_map(script_version, &rtx, |verifier| {
//             let max_cycles = Cycle::MAX;
//             let cycles;
//             let mut times = 0usize;
//             times += 1;
//             let mut init_state = match verifier.resumable_verify(max_cycles).unwrap() {
//                 VerifyResult::Suspended(state) => Some(state),
//                 VerifyResult::Completed(cycle) => {
//                     cycles = cycle;
//                     return Ok((cycles, times));
//                 }
//             };

//             loop {
//                 times += 1;
//                 let state = init_state.take().unwrap();
//                 match verifier.resume_from_state(state, max_cycles).unwrap() {
//                     VerifyResult::Suspended(state) => {
//                         init_state = Some(state);
//                     }
//                     VerifyResult::Completed(cycle) => {
//                         cycles = cycle;
//                         break;
//                     }
//                 }
//             }

//             Ok::<(u64, usize), Error>((cycles, times))
//         })
//         .unwrap();
//     assert_eq!(cycles, cycles_once);
// }
