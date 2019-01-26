use hash::sha3_256;
use numext_fixed_hash::H256;
use occupied_capacity::OccupiedCapacity;
use serde_derive::{Deserialize, Serialize};
use std::io::Write;
use std::mem;

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub version: u8,
    pub args: Vec<Vec<u8>>,

    // There're 2 ways of specifying script: one way is directly embed
    // the script to run in binary part; however, a common use case is
    // that CKB would provide common system cells containing common verification
    // algorithm, such as P2SH-SHA3-SECP256K1, in the meantime, crypto experts might
    // also put alternative advanced verfication algorithms on the chain. So another
    // way of loading a script, is that reference can be used to specify
    // an existing cell, when CKB runs the script, CKB will load the script from the
    // existing cell. This has the benefit of promoting code reuse, and reducing
    // transaction size: a typical secp256k1 verfication algorithm can take 1.2 MB
    // in space, which is not ideal to put in every tx input.
    // Note that the referenced cell here might also be included in transaction's
    // deps part, otherwise CKB will fail to verify the script.
    // CKB only enforces that reference and binary cannot both be
    // None, when they both contains actual value(though this is not recommended),
    // binary will be used.
    // When calculating script type hash, reference, binary,
    // and signed_args will all be included.
    pub reference: Option<H256>,
    pub binary: Option<Vec<u8>>,
    // Pre-defined arguments that are considered part of the script.
    // When signed_args contains <a>, <b>, and args contains <c>,
    // <d>, <e>, binary will then be executed with arguments <a>, <b>,
    // <c>, <d>, <e>.
    // This can be useful when binary is fixed, but depending on different
    // use case, we might have different initial parameters. For example, in
    // secp256k1 verification, we need to provide pubkey first, this cannot be
    // part of arguments, otherwise users can provide signatures signed by
    // arbitrary private keys. On the other hand, include pubkey inside
    // binary is not good for distribution, since the script here can
    // be over 1 megabytes. So signed_args helps here to preserve one common
    // binary, while enable us to provide different pubkeys for different
    // transactions.
    // For most verification algorithms, args will contain the signature
    // and any additional parameters needed by cell validator, while
    // signed_args will contain pubkey used in the signing part.
    pub signed_args: Vec<Vec<u8>>,
}

type ScriptTuple = (
    u8,
    Vec<Vec<u8>>,
    Option<H256>,
    Option<Vec<u8>>,
    Vec<Vec<u8>>,
);

impl Script {
    pub fn new(
        version: u8,
        args: Vec<Vec<u8>>,
        reference: Option<H256>,
        binary: Option<Vec<u8>>,
        signed_args: Vec<Vec<u8>>,
    ) -> Self {
        Script {
            version,
            args,
            reference,
            binary,
            signed_args,
        }
    }

    pub fn destruct(self) -> ScriptTuple {
        let Script {
            version,
            args,
            reference,
            binary,
            signed_args,
        } = self;
        (version, args, reference, binary, signed_args)
    }

    pub fn type_hash(&self) -> H256 {
        match self.version {
            0 => {
                let mut bytes = vec![];
                // TODO: switch to flatbuffer serialization once we
                // can do stable serialization using flatbuffer.
                if let Some(ref data) = self.reference {
                    bytes.write_all(data.as_bytes()).unwrap();
                }
                // A separator is used here to prevent the rare case
                // that some binary might contain the exactly
                // same data as reference. In this case we might
                // still want to distinguish between the 2 script in
                // the hash. Note this might not solve every problem,
                // when flatbuffer change is done, we can leverage flatbuffer
                // serialization directly, which will be more reliable.
                bytes.write_all(b"|").unwrap();
                if let Some(ref data) = self.binary {
                    bytes.write_all(&data).unwrap()
                }
                for argument in &self.signed_args {
                    bytes.write_all(argument).unwrap();
                }
                sha3_256(bytes).into()
            }
            _ => H256::zero(),
        }
    }
}

impl OccupiedCapacity for Script {
    fn occupied_capacity(&self) -> usize {
        mem::size_of::<u8>()
            + self.args.occupied_capacity()
            + self.reference.occupied_capacity()
            + self.binary.occupied_capacity()
            + self.signed_args.occupied_capacity()
    }
}
