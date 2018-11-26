use super::ScriptError;
use bigint::H256;
use ckb_core::cell::ResolvedTransaction;
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput};
use ckb_vm::{DefaultMachine, SparseMemory};
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use syscalls::{build_tx, Debugger, FetchScriptHash, MmapCell, MmapTx};

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionScriptsVerifier<'a> {
    dep_cells: FnvHashMap<H256, &'a CellOutput>,
    inputs: Vec<&'a CellInput>,
    outputs: Vec<&'a CellOutput>,
    tx_builder: FlatBufferBuilder<'a>,
    input_cells: Vec<&'a CellOutput>,
    hash: H256,
}

impl<'a> TransactionScriptsVerifier<'a> {
    pub fn new(rtx: &'a ResolvedTransaction) -> TransactionScriptsVerifier<'a> {
        let dep_cells: FnvHashMap<H256, &'a CellOutput> = rtx
            .dep_cells
            .iter()
            .map(|cell| {
                let output = cell
                    .get_current()
                    .expect("already verifies that all dep cells are valid");
                let hash = output.data_hash();
                (hash, output)
            }).collect();

        let inputs = rtx.transaction.inputs().iter().collect();
        let outputs = rtx.transaction.outputs().iter().collect();

        let input_cells = rtx
            .input_cells
            .iter()
            .map(|cell| {
                cell.get_current()
                    .expect("already verifies that all input cells are valid")
            }).collect();

        let mut tx_builder = FlatBufferBuilder::new();
        let tx_offset = build_tx(&mut tx_builder, &rtx.transaction);
        tx_builder.finish(tx_offset, None);

        TransactionScriptsVerifier {
            dep_cells,
            inputs,
            tx_builder,
            outputs,
            input_cells,
            hash: rtx.transaction.hash(),
        }
    }

    fn build_mmap_tx(&self) -> MmapTx {
        MmapTx::new(self.tx_builder.finished_data())
    }

    fn build_mmap_cell(&self) -> MmapCell {
        MmapCell::new(&self.outputs, &self.input_cells)
    }

    fn build_fetch_script_hash(&self) -> FetchScriptHash {
        FetchScriptHash::new(&self.outputs, &self.inputs, &self.input_cells)
    }

    // Script struct might contain references to external cells, this
    // method exacts the real script from Stript struct.
    fn extract_script(&self, script: &'a Script) -> Result<&'a [u8], ScriptError> {
        if let Some(ref data) = script.binary {
            return Ok(data);
        }
        if let Some(hash) = script.reference {
            return match self.dep_cells.get(&hash) {
                Some(ref cell_output) => Ok(&cell_output.data),
                None => Err(ScriptError::InvalidReferenceIndex),
            };
        }
        Err(ScriptError::NoScript)
    }

    pub fn verify_script(&self, script: &Script, prefix: &str) -> Result<(), ScriptError> {
        self.extract_script(script).and_then(|script_binary| {
            let mut args = vec![b"verify".to_vec()];
            args.extend_from_slice(&script.signed_args.as_slice());
            args.extend_from_slice(&script.args.as_slice());

            let mut machine = DefaultMachine::<u64, SparseMemory>::default();
            machine.add_syscall_module(Box::new(self.build_mmap_tx()));
            machine.add_syscall_module(Box::new(self.build_mmap_cell()));
            machine.add_syscall_module(Box::new(self.build_fetch_script_hash()));
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
            self.verify_script(&input.unlock, &prefix).map_err(|e| {
                info!(target: "script", "Error validating input {} of transaction {}: {:?}", i, self.hash, e);
                e
            })?;
        }
        for (i, output) in self.outputs.iter().enumerate() {
            if let Some(ref contract) = output.contract {
                let prefix = format!("Transaction {}, output {}", self.hash, i);
                self.verify_script(contract, &prefix).map_err(|e| {
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
    use bigint::H256;
    use ckb_core::cell::CellStatus;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use ckb_core::Capacity;
    use crypto::secp::Generator;
    use faster_hex::hex_to;
    use fnv::FnvHashMap;
    use hash::sha3_256;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::path::Path;

    fn open_cell_verify() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes/spec/cells/verify"))
            .unwrap()
    }
    fn open_cell_always_success() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../nodes/spec/cells/always_success"))
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

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
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

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_err());
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
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let dep_outpoint = OutPoint::new(H256::from(123), 8);
        let dep_cell = CellOutput::new(buffer.len() as Capacity, buffer, H256::from(0), None);
        let mut dep_cells = FnvHashMap::default();
        dep_cells.insert(&dep_outpoint, &dep_cell);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, args, Some(dep_cell.data_hash()), None, vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_outpoint)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![CellStatus::Current(dep_cell.clone())],
            input_cells: vec![],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_ok());
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
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        args.insert(0, hex_signature);

        let dep_outpoint = OutPoint::new(H256::from(123), 8);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");
        let script = Script::new(0, args, Some(H256::from(234)), None, vec![hex_privkey]);

        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .dep(dep_outpoint)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
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
        let output = CellOutput::new(0, Vec::new(), H256::from(0), Some(script));

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
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
        let output = CellOutput::new(0, Vec::new(), H256::from(0), Some(script));

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output.clone())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
        };

        let verifier = TransactionScriptsVerifier::new(&rtx);

        assert!(verifier.verify().is_err());
    }
}
