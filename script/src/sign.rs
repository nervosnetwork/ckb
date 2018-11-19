use bigint::H256;
use bincode::serialize;
use byteorder::{LittleEndian, WriteBytesExt};
use core::script::Script;
use core::transaction::{CellInput, CellOutput, OutPoint, Transaction};
use crypto::secp::Privkey;
use hash::sha3_256;

#[derive(Debug, Serialize)]
pub struct UnsignedCellInput {
    pub previous_output: OutPoint,
    pub unlock: UnsignedScript,
}

#[derive(Debug, Serialize)]
pub struct UnsignedScript {
    pub version: u8,
    pub redeem_script: Vec<u8>,
}

impl From<CellInput> for UnsignedCellInput {
    fn from(i: CellInput) -> Self {
        UnsignedCellInput {
            previous_output: i.previous_output,
            unlock: UnsignedScript {
                version: i.unlock.version,
                redeem_script: i.unlock.redeem_script,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TransactionInputSigner {
    pub version: u32,
    pub deps: Vec<OutPoint>,
    pub inputs: Vec<UnsignedCellInput>,
    pub outputs: Vec<CellOutput>,
}

impl From<Transaction> for TransactionInputSigner {
    fn from(t: Transaction) -> Self {
        TransactionInputSigner {
            version: t.version,
            deps: t.deps,
            inputs: t.inputs.into_iter().map(Into::into).collect(),
            outputs: t.outputs,
        }
    }
}

impl TransactionInputSigner {
    pub fn signed_input(&self, privkey: &Privkey, input_index: usize) -> CellInput {
        let hash = self.signature_hash(input_index);
        let signature = privkey.sign_schnorr(&hash).unwrap();
        let input = &self.inputs[input_index];

        CellInput {
            previous_output: input.previous_output.clone(),
            unlock: Script {
                version: input.unlock.version,
                arguments: vec![signature.serialize()],
                redeem_script: input.unlock.redeem_script.clone(),
            },
        }
    }

    pub fn signature_hash(&self, input_index: usize) -> H256 {
        // currently only supports hash_all: https://en.bitcoin.it/wiki/OP_CHECKSIG#Hashtype_SIGHASH_ALL_.28default.29
        // TODO add more hash type and optimize: https://github.com/bitcoin/bips/blob/master/bip-0143.mediawiki
        let mut bytes = serialize(&self).unwrap();
        bytes.write_u64::<LittleEndian>(input_index as u64).unwrap();
        sha3_256(&bytes).into()
    }
}
