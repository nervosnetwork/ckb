use bigint::H256;
use bincode::serialize;
use hash::sha3_256;
use std::io::Write;
use transaction::OutPoint;

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub version: u8,
    pub arguments: Vec<Vec<u8>>,

    // There're 2 ways of specifying redeem script: one way is directly embed
    // the script to run in redeem_script part; however, a common use case is
    // that CKB would provide common system cells containing common verification
    // algorithm, such as P2SH-SHA3-SECP256K1, in the meantime, crypto experts might
    // also put alternative advanced verfication algorithms on the chain. So another
    // way of loading a script, is that redeem_reference can be used to specify
    // an existing cell, when CKB runs the script, CKB will load the script from the
    // existing cell. This has the benefit of promoting code reuse, and reducing
    // transaction size: a typical secp256k1 verfication algorithm can take 1.2 MB
    // in space, which is not ideal to put in every tx input.
    // Note that the referenced cell here might also be included in transaction's
    // deps part, otherwise CKB will fail to verify the script.
    // CKB only enforces that redeem_reference and redeem_script cannot both be
    // None, when they both contains actual value(though this is not recommended),
    // redeem_script will be used.
    // When calculating redeem script hash, redeem_reference, redeem_script,
    // and redeem_arguments will all be included.
    pub redeem_reference: Option<OutPoint>,
    pub redeem_script: Option<Vec<u8>>,
    // Pre-defined arguments that are considered part of the redeem script.
    // When redeem_arguments contains <a>, <b>, and arguments contains <c>,
    // <d>, <e>, redeem_script will then be executed with arguments <a>, <b>,
    // <c>, <d>, <e>.
    // This can be useful when redeem_script is fixed, but depending on different
    // use case, we might have different initial parameters. For example, in
    // secp256k1 verification, we need to provide pubkey first, this cannot be
    // part of arguments, otherwise users can provide signatures signed by
    // arbitrary private keys. On the other hand, include pubkey inside
    // redeem_script is not good for distribution, since the script here can
    // be over 1 megabytes. So redeem_arguments helps here to preserve one common
    // redeem_script, while enable us to provide different pubkeys for different
    // transactions.
    // For most verification algorithms, arguments will contain the signature
    // and any additional parameters needed by cell validator, while
    // redeem_arguments will contain pubkey used in the signing part.
    pub redeem_arguments: Vec<Vec<u8>>,
}

impl Script {
    pub fn new(
        version: u8,
        arguments: Vec<Vec<u8>>,
        redeem_reference: Option<OutPoint>,
        redeem_script: Option<Vec<u8>>,
        redeem_arguments: Vec<Vec<u8>>,
    ) -> Self {
        Script {
            version,
            arguments,
            redeem_reference,
            redeem_script,
            redeem_arguments,
        }
    }

    pub fn redeem_script_hash(&self) -> H256 {
        match self.version {
            0 => {
                let mut bytes = vec![];
                if let Some(outpoint) = self.redeem_reference {
                    let data = serialize(&outpoint).unwrap();
                    bytes.write_all(&data).unwrap();
                }
                // A separator is used here to prevent the rare case
                // that some redeem_script might contain the exactly
                // same data as redeem_reference. In this case we might
                // still want to distinguish between the 2 script in
                // the hash. Note this might not solve every problem,
                // when flatbuffer change is done, we can leverage flatbuffer
                // serialization directly, which will be more reliable.
                bytes.write_all(b"|").unwrap();
                if let Some(ref data) = self.redeem_script {
                    bytes.write_all(&data).unwrap()
                }
                for argument in &self.redeem_arguments {
                    bytes.write_all(argument).unwrap();
                }
                sha3_256(bytes).into()
            }
            _ => H256::from(0),
        }
    }
}

// impl From<&'static str> for Script {
// 	fn from(s: &'static str) -> Self {
// 		Script::new(s.into())
// 	}
// }

// impl From<Bytes> for Script {
// 	fn from(s: Bytes) -> Self {
// 		Script::new(s)
// 	}
// }

// impl From<Vec<u8>> for Script {
// 	fn from(v: Vec<u8>) -> Self {
// 		Script::new(v.into())
// 	}
// }

// impl From<Script> for Bytes {
// 	fn from(script: Script) -> Self {
// 		script.data
// 	}
// }
