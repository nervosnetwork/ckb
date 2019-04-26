use crate::{
    cost_model::instruction_cycles,
    syscalls::{build_tx, Debugger, LoadCell, LoadCellByField, LoadInputByField, LoadTx},
    ScriptError,
};
use ckb_core::cell::{LiveCell, ResolvedTransaction};
use ckb_core::script::{Script, ALWAYS_SUCCESS_HASH};
use ckb_core::transaction::{CellInput, CellOutput};
use ckb_core::Cycle;
use ckb_vm::{DefaultCoreMachine, DefaultMachineBuilder, SparseMemory, SupportMachine};
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use log::info;
use numext_fixed_hash::H256;

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a> {
    binary_index: FnvHashMap<H256, &'a [u8]>,
    inputs: Vec<&'a CellInput>,
    outputs: Vec<&'a CellOutput>,
    tx_builder: FlatBufferBuilder<'a>,
    input_cells: Vec<&'a CellOutput>,
    dep_cells: Vec<&'a CellOutput>,
    witnesses: FnvHashMap<u32, &'a [Vec<u8>]>,
    hash: H256,
}

impl<'a> TransactionScriptsVerifier<'a> {
    pub fn new(rtx: &'a ResolvedTransaction) -> TransactionScriptsVerifier<'a> {
        let dep_cells: Vec<&'a CellOutput> = rtx
            .dep_cells
            .iter()
            .filter_map(LiveCell::get_live_output)
            .collect();
        let input_cells = rtx
            .input_cells
            .iter()
            .filter_map(LiveCell::get_live_output)
            .collect();
        let inputs = rtx.transaction.inputs().iter().collect();
        let outputs = rtx.transaction.outputs().iter().collect();
        let witnesses: FnvHashMap<u32, &'a [Vec<u8>]> = rtx
            .transaction
            .witnesses()
            .iter()
            .enumerate()
            .map(|(idx, wit)| (idx as u32, &wit[..]))
            .collect();

        let binary_index: FnvHashMap<H256, &'a [u8]> = dep_cells
            .iter()
            .map(|dep_cell| (dep_cell.data_hash(), &dep_cell.data[..]))
            .collect();

        let mut tx_builder = FlatBufferBuilder::new();
        let tx_offset = build_tx(&mut tx_builder, &rtx.transaction);
        tx_builder.finish(tx_offset, None);

        TransactionScriptsVerifier {
            binary_index,
            inputs,
            tx_builder,
            outputs,
            input_cells,
            dep_cells,
            witnesses,
            hash: rtx.transaction.hash().clone(),
        }
    }

    fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(self.tx_builder.finished_data())
    }

    fn build_load_cell(&self, current_cell: &'a CellOutput) -> LoadCell {
        LoadCell::new(
            &self.outputs,
            &self.input_cells,
            current_cell,
            &self.dep_cells,
        )
    }

    fn build_load_cell_by_field(&self, current_cell: &'a CellOutput) -> LoadCellByField {
        LoadCellByField::new(
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
    fn extract_script(&self, script: &'a Script) -> Result<&'a [u8], ScriptError> {
        match self.binary_index.get(&script.code_hash) {
            Some(ref binary) => Ok(binary),
            None => Err(ScriptError::InvalidReferenceIndex),
        }
    }

    pub fn verify_script(
        &self,
        script: &Script,
        prefix: &str,
        current_cell: &'a CellOutput,
        witness: Option<&&'a [Vec<u8>]>,
        current_input: Option<&'a CellInput>,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        if script.code_hash == ALWAYS_SUCCESS_HASH {
            return Ok(0);
        }
        let mut args = vec![b"verify".to_vec()];
        self.extract_script(script).and_then(|script_binary| {
            args.extend_from_slice(&script.args.as_slice());
            if let Some(ref input) = current_input {
                args.extend_from_slice(&input.args.as_slice());
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
                .load_program(script_binary, &args)
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
            let cycle = self.verify_script(&input_cell.lock, &prefix, input_cell, witness, Some(input), max_cycles - cycles).map_err(|e| {
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
            if let Some(ref type_) = output.type_ {
                let prefix = format!("Transaction {}, output {}", self.hash, i);
                let cycle = self.verify_script(type_, &prefix, output, None, None, max_cycles - cycles).map_err(|e| {
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
    use ckb_core::cell::{CellMeta, LiveCell};
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::{capacity_bytes, Capacity};
    use crypto::secp::Generator;
    use faster_hex::hex_encode;
    use hash::{blake2b_256, sha3_256};
    use numext_fixed_hash::H256;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;

    fn open_cell_verify() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap()
    }

    #[test]
    fn check_always_success_hash() {
        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(
                capacity_bytes!(100),
                vec![],
                Script::always_success(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(0).is_ok());
    }

    #[test]
    fn check_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![b"foo".to_vec(), b"bar".to_vec()];
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
        let dep_cell = CellMeta {
            cell_output: CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                buffer,
                Script::default(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(capacity_bytes!(100), vec![], script, None),
            block_number: Some(1),
            cellbase: false,
        };

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![LiveCell::Output(dep_cell)],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_with_not_enough_cycles() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![b"foo".to_vec(), b"bar".to_vec()];
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
        let dep_cell = CellMeta {
            cell_output: CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                buffer,
                Script::default(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(capacity_bytes!(100), vec![], script, None),
            block_number: Some(1),
            cellbase: false,
        };

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![LiveCell::Output(dep_cell)],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100).is_err());
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![b"foo".to_vec(), b"bar".to_vec()];
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
        args.push(b"extrastring".to_vec());

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        witness_data.insert(0, hex_pubkey);

        let code_hash: H256 = (&blake2b_256(&buffer)).into();
        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_cell = CellMeta {
            cell_output: CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                buffer,
                Script::default(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let script = Script::new(args, code_hash);
        let input = CellInput::new(OutPoint::null(), 0, vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .witness(witness_data)
            .build();

        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(capacity_bytes!(100), vec![], script, None),
            block_number: Some(1),
            cellbase: false,
        };

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![LiveCell::Output(dep_cell)],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let args = vec![b"foo".to_vec(), b"bar".to_vec()];
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

        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(capacity_bytes!(100), vec![], script, None),
            block_number: Some(1),
            cellbase: false,
        };

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![b"foo".to_vec(), b"bar".to_vec()];

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
        args.push(hex_pubkey);

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        args.push(hex_signature);

        let input = CellInput::new(OutPoint::null(), 0, vec![]);
        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(
                capacity_bytes!(100),
                vec![],
                Script::always_success(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let script = Script::new(args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            Capacity::zero(),
            Vec::new(),
            Script::new(vec![], H256::zero()),
            Some(script),
        );

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_cell = CellMeta {
            cell_output: CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                buffer,
                Script::default(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![LiveCell::Output(dep_cell)],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_invalid_output_contract() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![b"foo".to_vec(), b"bar".to_vec()];

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
        args.insert(0, hex_signature);
        // This line makes the verification invalid
        args.push(b"extrastring".to_vec());

        let pubkey = privkey.pubkey().unwrap().serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        args.insert(0, hex_pubkey);

        let input = CellInput::new(OutPoint::null(), 0, vec![]);
        let dummy_cell = CellMeta {
            cell_output: CellOutput::new(
                capacity_bytes!(100),
                vec![],
                Script::always_success(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let script = Script::new(args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            Capacity::zero(),
            Vec::new(),
            Script::default(),
            Some(script),
        );

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_cell = CellMeta {
            cell_output: CellOutput::new(
                Capacity::bytes(buffer.len()).unwrap(),
                buffer,
                Script::default(),
                None,
            ),
            block_number: Some(1),
            cellbase: false,
        };

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .dep(dep_out_point)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![LiveCell::Output(dep_cell)],
            input_cells: vec![LiveCell::Output(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_err());
    }
}
