use crate::syscalls::{build_tx, Debugger, LoadCell, LoadCellByField, LoadInputByField, LoadTx};
use crate::ScriptError;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput};
use ckb_protocol::{FlatbuffersVectorIterator, Script as FbsScript};
use ckb_vm::{DefaultMachine, SparseMemory};
use flatbuffers::{get_root, FlatBufferBuilder};
use fnv::FnvHashMap;
use log::info;
use numext_fixed_hash::H256;

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a> {
    dep_cell_index: FnvHashMap<H256, &'a CellOutput>,
    inputs: Vec<&'a CellInput>,
    outputs: Vec<&'a CellOutput>,
    tx_builder: FlatBufferBuilder<'a>,
    input_cells: Vec<&'a CellOutput>,
    dep_cells: Vec<&'a CellOutput>,
    hash: H256,
}

impl<'a> TransactionScriptsVerifier<'a> {
    pub fn new(rtx: &'a ResolvedTransaction) -> TransactionScriptsVerifier<'a> {
        let dep_cells: Vec<&'a CellOutput> = rtx
            .dep_cells
            .iter()
            .map(|cell| {
                cell.get_current()
                    .expect("already verifies that all dep cells are valid")
            })
            .collect();
        let dep_cell_index: FnvHashMap<H256, &'a CellOutput> = dep_cells
            .iter()
            .map(|cell| {
                let hash = cell.data_hash();
                (hash, *cell)
            })
            .collect();

        let inputs = rtx.transaction.inputs().iter().collect();
        let outputs = rtx.transaction.outputs().iter().collect();

        let input_cells = rtx
            .input_cells
            .iter()
            .map(|cell| {
                cell.get_current()
                    .expect("already verifies that all input cells are valid")
            })
            .collect();

        let mut tx_builder = FlatBufferBuilder::new();
        let tx_offset = build_tx(&mut tx_builder, &rtx.transaction);
        tx_builder.finish(tx_offset, None);

        TransactionScriptsVerifier {
            dep_cell_index,
            inputs,
            tx_builder,
            outputs,
            input_cells,
            dep_cells,
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

    // Script struct might contain references to external cells, this
    // method exacts the referenced script if any. It also fills signed args
    // so we don't need to do a second time of memory copy
    fn extract_script(
        &self,
        script: &'a Script,
        signed_args: &mut Vec<Vec<u8>>,
    ) -> Result<&'a [u8], ScriptError> {
        if let Some(ref data) = script.binary {
            signed_args.extend_from_slice(&script.signed_args);
            return Ok(data);
        }
        if let Some(ref hash) = script.reference {
            return match self.dep_cell_index.get(hash) {
                Some(ref cell_output) => {
                    let fbs_script = get_root::<FbsScript>(&cell_output.data);
                    // This way we can avoid copying the actual script binary one more
                    // time, which could be a lot of data.
                    let binary = fbs_script
                        .binary()
                        .and_then(|s| s.seq())
                        .ok_or(ScriptError::NoScript)?;
                    // When the reference script has signed arguments, we will concat
                    // signed arguments from the reference script with the signed
                    // arguments from the main script together.
                    if let Some(args) = fbs_script.signed_args() {
                        let args: Option<Vec<Vec<u8>>> = FlatbuffersVectorIterator::new(args)
                            .map(|arg| arg.seq().map(|s| s.to_vec()))
                            .collect();
                        let args = args.ok_or(ScriptError::ArgumentError)?;
                        signed_args.extend_from_slice(&args);
                        signed_args.extend_from_slice(&script.signed_args);
                    }
                    Ok(binary)
                }
                None => Err(ScriptError::InvalidReferenceIndex),
            };
        }
        Err(ScriptError::NoScript)
    }

    pub fn verify_script(
        &self,
        script: &Script,
        prefix: &str,
        current_cell: &'a CellOutput,
        current_input: Option<&'a CellInput>,
    ) -> Result<(), ScriptError> {
        let mut args = vec![b"verify".to_vec()];
        self.extract_script(script, &mut args)
            .and_then(|script_binary| {
                args.extend_from_slice(&script.args.as_slice());

                let mut machine = DefaultMachine::<u64, SparseMemory>::default();
                machine.add_syscall_module(Box::new(self.build_load_tx()));
                machine.add_syscall_module(Box::new(self.build_load_cell(current_cell)));
                machine.add_syscall_module(Box::new(self.build_load_cell_by_field(current_cell)));
                machine.add_syscall_module(Box::new(self.build_load_input_by_field(current_input)));
                machine.add_syscall_module(Box::new(Debugger::new(prefix)));
                machine
                    .run(script_binary, &args)
                    .map_err(ScriptError::VMError)
                    .and_then(|code| {
                        if code == 0 {
                            Ok(())
                        } else {
                            Err(ScriptError::ValidationFailure(code))
                        }
                    })
            })
    }

    pub fn verify(&self) -> Result<(), ScriptError> {
        for (i, input) in self.inputs.iter().enumerate() {
            let prefix = format!("Transaction {}, input {}", self.hash, i);
            self.verify_script(&input.unlock, &prefix, self.input_cells[i], Some(input)).map_err(|e| {
                info!(target: "script", "Error validating input {} of transaction {}: {:?}", i, self.hash, e);
                e
            })?;
        }
        for (i, output) in self.outputs.iter().enumerate() {
            if let Some(ref type_) = output.type_ {
                let prefix = format!("Transaction {}, output {}", self.hash, i);
                self.verify_script(type_, &prefix, output, None).map_err(|e| {
                    info!(target: "script", "Error validating output {} of transaction {}: {:?}", i, self.hash, e);
                    e
                })?;
            }
        }
        Ok(())
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
    use faster_hex::hex_to;
    use hash::sha3_256;
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
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, args, None, Some(buffer), vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = CellOutput::new(100, vec![], H256::default(), None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Current(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_ok());
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
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);
        // This line makes the verification invalid
        args.push(b"extrastring".to_vec());

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, args, None, Some(buffer), vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let dummy_cell = CellOutput::new(100, vec![], H256::default(), None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Current(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_err());
    }

    #[test]
    fn check_valid_dep_reference() {
        let mut file = open_cell_verify();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let script = Script::new(0, vec![], None, Some(buffer), vec![]);
        let mut builder = FlatBufferBuilder::new();
        let offset = FbsScript::build(&mut builder, &script);
        builder.finish(offset, None);
        let buffer = builder.finished_data().to_vec();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![b"foo".to_vec(), b"bar".to_vec()];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);
        let dep_cell = CellOutput::new(buffer.len() as Capacity, buffer, H256::zero(), None);
        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, args, Some(dep_cell.data_hash()), None, vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point.clone())
            .build();

        let dummy_cell = CellOutput::new(100, vec![], H256::default(), None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![CellStatus::Current(dep_cell.clone())],
            input_cells: vec![CellStatus::Current(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_ok());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut args = vec![b"foo".to_vec(), b"bar".to_vec()];

        let mut bytes = vec![];
        for argument in &args {
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let dep_out_point = OutPoint::new(H256::from_trimmed_hex_str("123").unwrap(), 8);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");
        let script = Script::new(
            0,
            args,
            Some(H256::from_trimmed_hex_str("234").unwrap()),
            None,
            vec![hex_privkey],
        );

        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_out_point)
            .build();

        let dummy_cell = CellOutput::new(100, vec![], H256::default(), None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Current(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_err());
    }

    fn create_always_success_script() -> Script {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        Script::new(0, Vec::new(), None, Some(buffer), Vec::new())
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
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, args, None, Some(buffer), vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), create_always_success_script());
        let output = CellOutput::new(0, Vec::new(), H256::zero(), Some(script));

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .build();

        let dummy_cell = CellOutput::new(100, vec![], H256::default(), None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Current(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_ok());
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
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);
        // This line makes the verification invalid
        args.push(b"extrastring".to_vec());

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, args, None, Some(buffer), vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), create_always_success_script());
        let output = CellOutput::new(0, Vec::new(), H256::zero(), Some(script));

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .build();

        let dummy_cell = CellOutput::new(100, vec![], H256::default(), None);

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![CellStatus::Current(dummy_cell)],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_err());
    }
}
