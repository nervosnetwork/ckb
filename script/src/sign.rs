use core::script::Script;
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
use crypto::secp::Privkey;

#[derive(Debug, Serialize)]
pub struct UnsignedCellInput {
    pub previous_output: OutPoint,
}

impl From<CellInput> for UnsignedCellInput {
    fn from(i: CellInput) -> Self {
        UnsignedCellInput {
            previous_output: i.previous_output,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TransactionInputSigner {
    pub version: u32,
    pub inputs: Vec<UnsignedCellInput>,
    pub outputs: Vec<CellOutput>,
}

impl From<Transaction> for TransactionInputSigner {
    fn from(t: Transaction) -> Self {
        TransactionInputSigner {
            version: t.version,
            inputs: t.inputs.into_iter().map(Into::into).collect(),
            outputs: t.outputs,
        }
    }
}

impl TransactionInputSigner {
    pub fn signed_input(
        &self,
        _privkey: &Privkey,
        _input_index: usize,
        _input_capacity: u32,
        _script: &Script,
    ) -> CellInput {
        unimplemented!()
        // let hash = self.sign(input_index, input_capacity, script);
        // let unlock = privkey.sign_recoverable(&hash).unwrap();

        // CellInput {
        // 	previous_output: self.inputs[input_index].previous_output.clone(),
        // 	unlock: unlock.into(),
        // }
    }

    pub fn sign(&self, _input_index: usize, _input_capacity: u32, _script: &Script) -> CellInput {
        unimplemented!()
        // currently only supports hash_all: https://en.bitcoin.it/wiki/OP_CHECKSIG#Hashtype_SIGHASH_ALL_.28default.29
        // TODO add more hash type and optimize: https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki
        // let mut bytes = serialize(&self).unwrap();
        // bytes.write_u64::<LittleEndian>(input_index as u64).unwrap();
        // bytes.write_u32::<LittleEndian>(input_capacity).unwrap();
        // bytes.append(&mut script.hash().to_vec());
        // sha3_256(&bytes).into()

        // CellInput {
        // 	previous_output: self.inputs[input_index].previous_output.clone(),
        // 	unlock: script.into(),
        // }
    }
}
