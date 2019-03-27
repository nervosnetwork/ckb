use crate::{
    cost_model::instruction_cycles,
    syscalls::{
        build_tx, Debugger, LoadCell, LoadCellByField, LoadEmbed, LoadInputByField, LoadTx,
    },
    ScriptError,
};
use ckb_core::cell::ResolvedTransaction;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput};
use ckb_core::Cycle;
use ckb_vm::{CoreMachine, DefaultMachine, SparseMemory};
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use hash::blake2b_256;
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
    embeds: Vec<&'a Vec<u8>>,
    hash: H256,
}

impl<'a> TransactionScriptsVerifier<'a> {
    pub fn new(rtx: &'a ResolvedTransaction) -> TransactionScriptsVerifier<'a> {
        let dep_cells: Vec<&'a CellOutput> = rtx
            .dep_cells
            .iter()
            .map(|cell| {
                cell.get_live()
                    .expect("already verifies that all dep cells are valid")
            })
            .collect();
        let input_cells = rtx
            .input_cells
            .iter()
            .map(|cell| {
                cell.get_live()
                    .expect("already verifies that all input cells are valid")
            })
            .collect();
        let inputs = rtx.transaction.inputs().iter().collect();
        let outputs = rtx.transaction.outputs().iter().collect();
        let embeds: Vec<&'a Vec<u8>> = rtx.transaction.embeds().iter().collect();

        let mut binary_index: FnvHashMap<H256, &'a [u8]> = FnvHashMap::default();
        for dep_cell in &dep_cells {
            binary_index.insert(dep_cell.data_hash(), &dep_cell.data);
        }
        for embed in &embeds {
            binary_index.insert(blake2b_256(&embed).into(), &embed);
        }

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
            embeds,
            hash: rtx.transaction.hash().clone(),
        }
    }

    fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(self.tx_builder.finished_data())
    }

    fn build_load_embeds(&self) -> LoadEmbed {
        LoadEmbed::new(&self.embeds)
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

    // Extracts actual script binary either in dep cell or in embeds.
    fn extract_script(&self, script: &'a Script) -> Result<&'a [u8], ScriptError> {
        match self.binary_index.get(&script.binary_hash) {
            Some(ref binary) => Ok(binary),
            None => Err(ScriptError::InvalidReferenceIndex),
        }
    }

    pub fn verify_script(
        &self,
        script: &Script,
        prefix: &str,
        current_cell: &'a CellOutput,
        current_input: Option<&'a CellInput>,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        let mut args = vec![b"verify".to_vec()];
        self.extract_script(script).and_then(|script_binary| {
            args.extend_from_slice(&script.args.as_slice());
            if let Some(ref input) = current_input {
                args.extend_from_slice(&input.args.as_slice());
            }

            let mut machine = DefaultMachine::<u64, SparseMemory>::new_with_cost_model(
                Box::new(instruction_cycles),
                max_cycles,
            );
            machine.add_syscall_module(Box::new(self.build_load_tx()));
            machine.add_syscall_module(Box::new(self.build_load_cell(current_cell)));
            machine.add_syscall_module(Box::new(self.build_load_cell_by_field(current_cell)));
            machine.add_syscall_module(Box::new(self.build_load_input_by_field(current_input)));
            machine.add_syscall_module(Box::new(self.build_load_embeds()));
            machine.add_syscall_module(Box::new(Debugger::new(prefix)));
            machine
                .run(script_binary, &args)
                .map_err(ScriptError::VMError)
                .and_then(|code| {
                    if code == 0 {
                        Ok(machine.cycles())
                    } else {
                        Err(ScriptError::ValidationFailure(code))
                    }
                })
        })
    }

    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let mut cycles = 0;
        for (i, input) in self.inputs.iter().enumerate() {
            let prefix = format!("Transaction {}, input {}", self.hash, i);
            let cycle = self.verify_script(&self.input_cells[i].lock, &prefix, self.input_cells[i], Some(input), max_cycles - cycles).map_err(|e| {
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
                let cycle = self.verify_script(type_, &prefix, output, None, max_cycles - cycles).map_err(|e| {
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
    use ckb_core::cell::CellStatus;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::Capacity;
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
    fn open_cell_always_success() -> File {
        File::open(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../nodes_template/spec/cells/always_success"),
        )
        .unwrap()
    }

    #[test]
    fn check_signature() {
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
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        args.insert(0, hex_signature);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let binary_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(0, args, binary_hash);
        let input = CellInput::new(OutPoint::null(), vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .embed(buffer)
            .build();

        let dummy_cell = CellOutput::new(100, vec![], script, None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(dummy_cell)],
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

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let binary_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(0, args, binary_hash);
        let input = CellInput::new(OutPoint::null(), vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .embed(buffer)
            .build();

        let dummy_cell = CellOutput::new(100, vec![], script, None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(dummy_cell)],
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

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let binary_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(0, args, binary_hash);
        let input = CellInput::new(OutPoint::null(), vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .embed(buffer)
            .build();

        let dummy_cell = CellOutput::new(100, vec![], script, None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_err());
    }

    #[test]
    fn check_valid_dep_reference() {
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

        let binary_hash: H256 = (&blake2b_256(&buffer)).into();

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_lock = Script::new(0, vec![], H256::zero());
        let dep_cell = CellOutput::new(buffer.len() as Capacity, buffer, dep_lock, None);
        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let script = Script::new(0, args, binary_hash);
        let input = CellInput::new(OutPoint::null(), vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point.clone())
            .build();

        let dummy_cell = CellOutput::new(100, vec![], script, None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![CellStatus::Live(dep_cell.clone())],
            input_cells: vec![CellStatus::Live(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_invalid_dep_reference() {
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

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let binary_hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(0, args, binary_hash);
        let input = CellInput::new(OutPoint::null(), vec![]);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let dummy_cell = CellOutput::new(100, vec![], script, None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_err());
    }

    fn create_always_success_script() -> (Script, Vec<u8>) {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let hash: H256 = (&blake2b_256(&buffer)).into();
        let script = Script::new(0, Vec::new(), hash);

        (script, buffer)
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

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let (input_lock_script, input_lock_binary) = create_always_success_script();
        let input = CellInput::new(OutPoint::null(), vec![]);
        let dummy_cell = CellOutput::new(100, vec![], input_lock_script, None);

        let script = Script::new(0, args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(
            0,
            Vec::new(),
            Script::new(0, vec![], H256::zero()),
            Some(script),
        );

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .embed(buffer)
            .embed(input_lock_binary)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(dummy_cell)],
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

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_encode(&privkey, &mut hex_privkey).expect("hex privkey");
        args.insert(0, hex_privkey);

        let (input_lock_script, input_lock_binary) = create_always_success_script();
        let input = CellInput::new(OutPoint::null(), vec![]);
        let dummy_cell = CellOutput::new(100, vec![], input_lock_script, None);

        let script = Script::new(0, args, (&blake2b_256(&buffer)).into());
        let output = CellOutput::new(0, Vec::new(), Script::default(), Some(script));

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .embed(buffer)
            .embed(input_lock_binary)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Live(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify(100_000_000).is_err());
    }
}
