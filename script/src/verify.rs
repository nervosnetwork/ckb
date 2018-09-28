use super::Error;
use core::transaction::{CellInput, CellOutput, OutPoint};
use fnv::FnvHashMap;
use vm::run;

// This struct leverages CKB VM to verify transaction inputs.
pub struct TransactionInputVerifier<'a> {
    pub dep_cells: FnvHashMap<&'a OutPoint, &'a CellOutput>,
    pub inputs: Vec<&'a CellInput>,
}

impl<'a> TransactionInputVerifier<'a> {
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
            let mut args = vec!["verify".to_string()];
            args.extend_from_slice(&input.unlock.redeem_arguments[..]);
            args.extend_from_slice(&input.unlock.arguments[..]);
            run(script, &args)
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
    use core::script::Script;
    use core::transaction::{CellInput, CellOutput, OutPoint};
    use core::Capacity;
    use crypto::secp::Generator;
    use fnv::FnvHashMap;
    use hash::sha3_256;
    use rustc_hex::ToHex;
    use std::fs::File;
    use std::io::{Read, Write};

    #[test]
    fn check_signature() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments: Vec<String> = vec!["foo".to_string(), "bar".to_string()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument.as_bytes()).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        arguments.insert(0, signature.serialize_der().to_hex());

        let script = Script::new(
            0,
            arguments,
            None,
            Some(buffer),
            vec![privkey.pubkey().unwrap().serialize().to_hex()],
        );
        let input = CellInput::new(OutPoint::null(), script);
        let inputs = vec![&input];

        let verifier = TransactionInputVerifier {
            dep_cells: FnvHashMap::default(),
            inputs,
        };

        assert!(verifier.verify(0).is_ok());
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments: Vec<String> = vec!["foo".to_string(), "bar".to_string()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument.as_bytes()).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        arguments.insert(0, signature.serialize_der().to_hex());
        // This line makes the verification invalid
        arguments.push("extrastring".to_string());

        let script = Script::new(
            0,
            arguments,
            None,
            Some(buffer),
            vec![privkey.pubkey().unwrap().serialize().to_hex()],
        );
        let input = CellInput::new(OutPoint::null(), script);
        let inputs = vec![&input];

        let verifier = TransactionInputVerifier {
            dep_cells: FnvHashMap::default(),
            inputs,
        };

        assert!(verifier.verify(0).is_err());
    }

    #[test]
    fn check_valid_dep_reference() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments: Vec<String> = vec!["foo".to_string(), "bar".to_string()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument.as_bytes()).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        arguments.insert(0, signature.serialize_der().to_hex());

        let dep_outpoint = OutPoint::new(H256::from(123), 8);
        let dep_cell = CellOutput::new(buffer.len() as Capacity, buffer, H256::from(0));
        let mut dep_cells = FnvHashMap::default();
        dep_cells.insert(&dep_outpoint, &dep_cell);
        let script = Script::new(
            0,
            arguments,
            Some(dep_outpoint),
            None,
            vec![privkey.pubkey().unwrap().serialize().to_hex()],
        );
        let input = CellInput::new(OutPoint::null(), script);
        let inputs = vec![&input];

        let verifier = TransactionInputVerifier { dep_cells, inputs };

        assert!(verifier.verify(0).is_ok());
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = File::open("../spec/res/cells/verify").unwrap();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let gen = Generator::new();
        let privkey = gen.random_privkey();
        let mut arguments: Vec<String> = vec!["foo".to_string(), "bar".to_string()];

        let mut bytes = vec![];
        for argument in &arguments {
            bytes.write(argument.as_bytes()).unwrap();
        }
        let hash1 = sha3_256(&bytes);
        let hash2 = sha3_256(hash1);
        let signature = privkey.sign_recoverable(&hash2.into()).unwrap();
        arguments.insert(0, signature.serialize_der().to_hex());

        let dep_outpoint = OutPoint::new(H256::from(123), 8);
        let script = Script::new(
            0,
            arguments,
            Some(dep_outpoint),
            None,
            vec![privkey.pubkey().unwrap().serialize().to_hex()],
        );
        let input = CellInput::new(OutPoint::null(), script);
        let inputs = vec![&input];

        let verifier = TransactionInputVerifier {
            dep_cells: FnvHashMap::default(),
            inputs,
        };

        assert!(verifier.verify(0).is_err());
    }
}
