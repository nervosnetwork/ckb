use crate::{
    common::{CurrentCell, LazyLoadCellOutput},
    cost_model::instruction_cycles,
    syscalls::{build_tx, Debugger, LoadCell, LoadCellByField, LoadInputByField, LoadTx},
    ScriptError,
};
use ckb_core::cell::{CellMeta, ResolvedTransaction};
use ckb_core::script::{Script, ALWAYS_SUCCESS_HASH};
use ckb_core::transaction::{CellInput, OutPoint};
use ckb_core::{Bytes, Cycle};
use ckb_store::ChainStore;
use ckb_vm::{DefaultCoreMachine, DefaultMachineBuilder, SparseMemory, SupportMachine};
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use log::info;
use numext_fixed_hash::H256;
use std::sync::Arc;

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a, CS> {
    store: Arc<CS>,
    binary_index: FnvHashMap<H256, usize>,
    inputs: Vec<&'a CellInput>,
    outputs: Vec<CellMeta>,
    tx_builder: FlatBufferBuilder<'a>,
    input_cells: Vec<&'a CellMeta>,
    dep_cells: Vec<&'a CellMeta>,
    witnesses: FnvHashMap<u32, &'a [Vec<u8>]>,
    hash: H256,
}

impl<'a, CS: ChainStore> TransactionScriptsVerifier<'a, CS> {
    pub fn new(rtx: &'a ResolvedTransaction, store: Arc<CS>) -> TransactionScriptsVerifier<'a, CS> {
        let tx_hash = rtx.transaction.hash();
        let dep_cells: Vec<&'a CellMeta> = rtx
            .dep_cells
            .iter()
            .map(|cell| {
                cell.get_live_cell()
                    .expect("already verifies that all dep cells are valid")
            })
            .collect();
        let input_cells = rtx
            .input_cells
            .iter()
            .map(|cell| {
                cell.get_live_cell()
                    .expect("already verifies that all input cells are valid")
            })
            .collect();
        let inputs = rtx.transaction.inputs().iter().collect();
        let outputs = rtx
            .transaction
            .outputs()
            .iter()
            .enumerate()
            .map({
                |(index, output)| {
                    store
                        .get_cell_meta(&tx_hash, index as u32)
                        .map(|mut cell_meta| {
                            cell_meta.cell_output = Some(output.clone());
                            cell_meta
                        })
                        .unwrap_or_else(|| CellMeta {
                            cell_output: Some(output.clone()),
                            out_point: OutPoint {
                                tx_hash: tx_hash.clone(),
                                index: index as u32,
                            },
                            block_number: None,
                            cellbase: false,
                            capacity: output.capacity,
                            data_hash: None,
                        })
                }
            })
            .collect();
        let witnesses: FnvHashMap<u32, &'a [Vec<u8>]> = rtx
            .transaction
            .witnesses()
            .iter()
            .enumerate()
            .map(|(idx, wit)| (idx as u32, &wit[..]))
            .collect();

        let binary_index: FnvHashMap<H256, usize> = dep_cells
            .iter()
            .enumerate()
            .map(|(i, dep_cell)| {
                let hash = match dep_cell.data_hash() {
                    Some(hash) => hash.to_owned(),
                    None => {
                        let output = store.lazy_load_cell_output(dep_cell);
                        output.data_hash()
                    }
                };
                (hash, i)
            })
            .collect();

        let mut tx_builder = FlatBufferBuilder::new();
        let tx_offset = build_tx(&mut tx_builder, &rtx.transaction);
        tx_builder.finish(tx_offset, None);

        TransactionScriptsVerifier {
            store,
            binary_index,
            inputs,
            tx_builder,
            outputs,
            input_cells,
            dep_cells,
            witnesses,
            hash: tx_hash,
        }
    }

    fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(self.tx_builder.finished_data())
    }

    fn build_load_cell(&'a self, current_cell: CurrentCell) -> LoadCell<'a, CS> {
        LoadCell::new(
            Arc::clone(&self.store),
            &self.outputs,
            &self.input_cells,
            current_cell,
            &self.dep_cells,
        )
    }

    fn build_load_cell_by_field(&'a self, current_cell: CurrentCell) -> LoadCellByField<'a, CS> {
        LoadCellByField::new(
            Arc::clone(&self.store),
            &self.outputs,
            &self.input_cells,
            current_cell,
            &self.dep_cells,
        )
    }

    fn build_load_input_by_field(&self, current_input: Option<&'a CellInput>) -> LoadInputByField {
        LoadInputByField::new(&self.inputs, current_input)
    }

    // Extracts actual script binary either in dep cells.
    fn extract_script(&self, script: &'a Script) -> Result<Bytes, ScriptError> {
        match self
            .binary_index
            .get(&script.code_hash)
            .map(|index| self.store.lazy_load_cell_output(&self.dep_cells[*index]))
        {
            Some(cell_output) => Ok(cell_output.data),
            None => Err(ScriptError::InvalidReferenceIndex),
        }
    }

    pub fn verify_script(
        &self,
        script: &Script,
        prefix: &str,
        current_cell: CurrentCell,
        witness: Option<&&'a [Vec<u8>]>,
        current_input: Option<&'a CellInput>,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        if script.code_hash == ALWAYS_SUCCESS_HASH {
            return Ok(0);
        }
        let mut args = vec![b"verify".to_vec()];
        self.extract_script(script).and_then(|script_binary| {
            args.extend_from_slice(
                &script
                    .args
                    .iter()
                    .map(|b| b[..].to_vec())
                    .collect::<Vec<Vec<u8>>>(),
            );
            if let Some(ref input) = current_input {
                args.extend_from_slice(
                    &input
                        .args
                        .iter()
                        .map(|b| b[..].to_vec())
                        .collect::<Vec<Vec<u8>>>(),
                );
            }
            if let Some(witness) = witness {
                args.extend_from_slice(&witness);
            }

            let core_machine =
                DefaultCoreMachine::<u64, SparseMemory<u64>>::new_with_max_cycles(max_cycles);
            let mut machine =
                DefaultMachineBuilder::<DefaultCoreMachine<u64, SparseMemory<u64>>>::new(
                    core_machine,
                )
                .instruction_cycle_func(Box::new(instruction_cycles))
                .syscall(Box::new(self.build_load_tx()))
                .syscall(Box::new(self.build_load_cell(current_cell)))
                .syscall(Box::new(self.build_load_cell_by_field(current_cell)))
                .syscall(Box::new(self.build_load_input_by_field(current_input)))
                .syscall(Box::new(Debugger::new(prefix)))
                .build()
                .load_program(&script_binary, &args)
                .map_err(ScriptError::VMError)?;
            let code = machine.interpret().map_err(ScriptError::VMError)?;
            if code == 0 {
                Ok(machine.cycles())
            } else {
                Err(ScriptError::ValidationFailure(code))
            }
        })
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let mut cycles = 0;
        for (i, (input, input_cell)) in self.inputs.iter().zip(self.input_cells.iter()).enumerate()
        {
            let prefix = format!("Transaction {}, input {}", self.hash, i);
            let witness = self.witnesses.get(&(i as u32));
            let output = self.store.lazy_load_cell_output(input_cell);
            let cycle = self.verify_script(&output.lock, &prefix, CurrentCell::Input(i), witness, Some(input), max_cycles - cycles).map_err(|e| {
                info!(target: "script", "Error validating input {} of transaction {}: {:?}", i, self.hash, e);
                e
            })?;
            let current_cycles = cycles
                .checked_add(cycle)
                .ok_or(ScriptError::ExceededMaximumCycles)?;
            if current_cycles > max_cycles {
                return Err(ScriptError::ExceededMaximumCycles);
            }
            cycles = current_cycles;
        }
        for (i, output) in self.outputs.iter().enumerate() {
            let output = self.store.lazy_load_cell_output(output);
            if let Some(ref type_) = output.type_ {
                let prefix = format!("Transaction {}, output {}", self.hash, i);
                let cycle = self.verify_script(type_, &prefix, CurrentCell::Output(i), None, None, max_cycles - cycles).map_err(|e| {
                    info!(target: "script", "Error validating output {} of transaction {}: {:?}", i, self.hash, e);
                    e
                })?;
                let current_cycles = cycles
                    .checked_add(cycle)
                    .ok_or(ScriptError::ExceededMaximumCycles)?;
                if current_cycles > max_cycles {
                    return Err(ScriptError::ExceededMaximumCycles);
                }
                cycles = current_cycles;
            }
        }
        Ok(cycles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::cell::{CellMeta, CellStatus, LiveCell};
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, Capacity};
    use ckb_db::MemoryKeyValueDB;
    use ckb_store::{ChainKVStore, COLUMNS};
    use crypto::secp::Generator;
    use faster_hex::hex_encode;
    use hash::{blake2b_256, sha3_256};
    use numext_fixed_hash::H256;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;
    use std::sync::Arc;

    fn open_cell_verify() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap()
    }

    fn new_memory_store() -> ChainKVStore<MemoryKeyValueDB> {
        ChainKVStore::new(MemoryKeyValueDB::open(COLUMNS as usize))
    }

    #[test]
    fn check_always_success_hash() {
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::always_success(),
            None,
        );
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = CellMeta {
            capacity: output.capacity,
            cell_output: Some(output),
            block_number: Some(1),
            ..Default::default()
        };

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };

        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(0).is_ok());
    }

    #[test]
    fn check_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        witness_data.insert(0, hex_signature);

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, hex_pubkey);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = CellMeta {
            block_number: Some(1),
            cellbase: false,
            capacity: output.capacity,
            data_hash: Some(code_hash.clone()),
            out_point: dep_out_point.clone(),
            cell_output: Some(output),
        };

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            capacity: output.capacity,
            ..Default::default()
        };

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dep_cell)))],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };
        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_with_not_enough_cycles() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        witness_data.insert(0, hex_signature);

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, hex_pubkey);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            cellbase: false,
            data_hash: Some(code_hash.clone()),
            capacity: output.capacity,
            out_point: dep_out_point.clone(),
        };

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            capacity: output.capacity,
            ..Default::default()
        };

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dep_cell)))],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };

        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(100).is_err());
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        witness_data.insert(0, hex_signature);
        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, hex_pubkey);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let output = CellOutput::new(
            Capacity::bytes(buffer.len()).unwrap(),
            Bytes::from(buffer),
            Script::default(),
            None,
        );
        let dep_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            data_hash: Some(code_hash.clone()),
            capacity: output.capacity,
            ..Default::default()
        };

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            capacity: output.capacity,
            ..Default::default()
        };

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dep_cell)))],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };

        let store = Arc::new(new_memory_store());
        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];
        let mut witness_data = vec![];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        witness_data.insert(0, hex_signature);

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, hex_pubkey);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let output = CellOutput::new(capacity_bytes!(100), Bytes::default(), script, None);
        let dummy_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            capacity: output.capacity,
            ..Default::default()
        };

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };

        let store = Arc::new(new_memory_store());
        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.push(Bytes::from(hex_pubkey));

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        args.push(Bytes::from(hex_signature));

        let input = CellInput::new(OutPoint::null(), 0, vec![]);
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::always_success(),
            None,
        );
        let dummy_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            capacity: output.capacity,
            ..Default::default()
        };

        let script = Script::new(args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            Capacity::zero(),
            Bytes::default(),
            Script::new(vec![], H256::zero()),
            Some(script),
        );

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_cell = {
            let output = CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                Bytes::from(buffer),
                Script::default(),
                None,
            );
            CellMeta {
                cell_output: Some(output.clone()),
                block_number: Some(1),
                cellbase: false,
                capacity: output.capacity,
                data_hash: None,
                out_point: dep_out_point.clone(),
            }
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dep_cell)))],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };

        let store = Arc::new(new_memory_store());
        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_invalid_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![Bytes::from(b"foo".to_vec()), Bytes::from(b"bar".to_vec())];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write_all(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, Bytes::from(hex_signature));
        // This line makes the verification invalid
        args.push(Bytes::from(b"extrastring".to_vec()));

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.insert(0, Bytes::from(hex_pubkey));

        let input = CellInput::new(OutPoint::null(), 0, vec![]);
        let output = CellOutput::new(
            capacity_bytes!(100),
            Bytes::default(),
            Script::always_success(),
            None,
        );
        let dummy_cell = CellMeta {
            cell_output: Some(output.clone()),
            block_number: Some(1),
            capacity: output.capacity,
            ..Default::default()
        };

        let script = Script::new(args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            Capacity::zero(),
            Bytes::default(),
            Script::default(),
            Some(script),
        );

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_cell = {
            let output = CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                Bytes::from(buffer),
                Script::default(),
                None,
            );
            CellMeta {
                cell_output: Some(output.clone()),
                capacity: output.capacity,
                block_number: Some(1),
                ..Default::default()
            }
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction: &transaction,
            dep_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dep_cell)))],
            input_cells: vec![CellStatus::Live(LiveCell::Output(Box::new(dummy_cell)))],
        };

        let store = Arc::new(new_memory_store());

        let verifier = TransactionScriptsVerifier::new(&rtx, store);

        assert!(verifier.verify(100_000_000).is_err());
    }
}
