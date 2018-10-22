use super::Error;
use core::cell::ResolvedTransaction;
use core::transaction::{CellInput, CellOutput, OutPoint};
use flatbuffers::FlatBufferBuilder;
use fnv::FnvHashMap;
use syscalls::{build_tx, MmapCell, MmapTx};
use vm::{DefaultMachine, SparseMemory};

// This struct leverages CKB VM to verify transaction inputs.
// FlatBufferBuilder owned Vec<u8> that grows as needed, in the
// future, we might refactor this to share buffer to achive zero-copy
pub struct TransactionInputVerifier<'a> {
    dep_cells: FnvHashMap<&'a OutPoint, &'a CellOutput>,
    inputs: Vec<&'a CellInput>,
    outputs: Vec<&'a CellOutput>,
    tx_builder: FlatBufferBuilder<'a>,
    input_cells: Vec<&'a CellOutput>,
}

impl<'a> TransactionInputVerifier<'a> {
    pub fn new(rtx: &'a ResolvedTransaction) -> TransactionInputVerifier<'a> {
        let dep_cell_outputs = rtx.dep_cells.iter().map(|cell| {
            cell.get_current()
                .expect("already verifies that all dep cells are valid")
        });
        let dep_outpoints = rtx.transaction.deps().iter();

        let dep_cells: FnvHashMap<&'a OutPoint, &'a CellOutput> =
            dep_outpoints.zip(dep_cell_outputs).collect();

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

        TransactionInputVerifier {
            dep_cells,
            inputs,
            tx_builder,
            outputs,
            input_cells,
        }
    }

    fn build_mmap_tx(&self) -> MmapTx {
        MmapTx::new(self.tx_builder.finished_data())
    }

    fn build_mmap_cell(&self) -> MmapCell {
        MmapCell::new(&self.outputs, &self.input_cells)
    }

    fn extract_script(&self, index: usize) -> Result<&[u8], Error> {
        let input = self.inputs[index];
        if let Some(ref data) = input.unlock.redeem_script {
            return Ok(data);
        }
        if let Some(outpoint) = input.unlock.redeem_reference {
            return match self.dep_cells.get(&outpoint) {
                Some(ref cell_output) => Ok(&cell_output.data),
                None => Err(Error::InvalidReferenceIndex),
            };
        }
        Err(Error::NoScript)
    }

    pub fn verify(&self, index: usize) -> Result<(), Error> {
        let input = self.inputs[index];
        self.extract_script(index).and_then(|script| {
            let mut args = vec![b"verify".to_vec()];
            args.extend_from_slice(&input.unlock.redeem_arguments.as_slice());
            args.extend_from_slice(&input.unlock.arguments.as_slice());

            let mut machine = DefaultMachine::<u64, SparseMemory>::default();
            machine.add_syscall_module(Box::new(self.build_mmap_tx()));
            machine.add_syscall_module(Box::new(self.build_mmap_cell()));
            machine
                .run(script, &args)
                .map_err(|_| Error::VMError)
                .and_then(|code| {
                    if code == 0 {
                        Ok(())
                    } else {
                        Err(Error::ValidationFailure(code))
                    }
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::H256;
    use core::cell::CellStatus;
    use core::script::Script;
    use core::transaction::{CellInput, CellOutput, OutPoint, TransactionBuilder};
    use core::Capacity;
    use crypto::secp::Generator;
    use faster_hex::hex_to;
    use fnv::FnvHashMap;
    use hash::sha3_256;
    use std::fs::File;
    use std::io::{Read, Write};

    #[test]
    fn check_signature() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments = vec![b"foo".to_vec(), b"bar".to_vec()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        arguments.insert(0, hex_signature);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, arguments, None, Some(buffer), vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
        };

        let verifier = TransactionInputVerifier::new(&rtx);

        assert!(verifier.verify(0).is_ok());
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments = vec![b"foo".to_vec(), b"bar".to_vec()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();

        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        arguments.insert(0, hex_signature);
        // This line makes the verification invalid
        arguments.push(b"extrastring".to_vec());

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, arguments, None, Some(buffer), vec![hex_privkey]);
        let input = CellInput::new(OutPoint::null(), script);

        let transaction = TransactionBuilder::default().input(input.clone()).build();

        let rtx = ResolvedTransaction {
            transaction,
            dep_cells: vec![],
            input_cells: vec![],
        };

        let verifier = TransactionInputVerifier::new(&rtx);

        assert!(verifier.verify(0).is_err());
    }

    #[test]
    fn check_valid_dep_reference() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments = vec![b"foo".to_vec(), b"bar".to_vec()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        arguments.insert(0, hex_signature);

        let dep_outpoint = OutPoint::new(H256::from(123), 8);
        let dep_cell = CellOutput::new(buffer.len() as Capacity, buffer, H256::from(0));
        let mut dep_cells = FnvHashMap::default();
        dep_cells.insert(&dep_outpoint, &dep_cell);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");

        let script = Script::new(0, arguments, Some(dep_outpoint), None, vec![hex_privkey]);
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

        let verifier = TransactionInputVerifier::new(&rtx);

        assert!(verifier.verify(0).is_ok());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments = vec![b"foo".to_vec(), b"bar".to_vec()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_to(&signature_der, &mut hex_signature).expect("hex privkey");
        arguments.insert(0, hex_signature);

        let dep_outpoint = OutPoint::new(H256::from(123), 8);

        let privkey = privkey.pubkey().unwrap().serialize();
        let mut hex_privkey = vec![0; privkey.len() * 2];
        hex_to(&privkey, &mut hex_privkey).expect("hex privkey");
        let script = Script::new(0, arguments, Some(dep_outpoint), None, vec![hex_privkey]);

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

        let verifier = TransactionInputVerifier::new(&rtx);

        assert!(verifier.verify(0).is_err());
    }
}
