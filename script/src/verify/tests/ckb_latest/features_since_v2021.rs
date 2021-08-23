use ckb_chain_spec::consensus::{TWO_IN_TWO_OUT_BYTES, TWO_IN_TWO_OUT_CYCLES, TYPE_ID_CODE_HASH};
use ckb_crypto::secp::Generator;
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
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint, Script, WitnessArgs},
    H256,
};
use ckb_vm::Error as VmError;
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
