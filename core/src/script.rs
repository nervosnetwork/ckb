use faster_hex::hex_encode;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use occupied_capacity::OccupiedCapacity;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::mem;

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
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

struct OptionDisplay<T>(Option<T>);

impl<T: fmt::Display> fmt::Display for OptionDisplay<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            Some(ref v) => write!(f, "Some({})", v),
            None => write!(f, "None"),
        }
    }
}

fn prefix_hex(bytes: &[u8]) -> String {
    let mut dst = vec![0u8; bytes.len() * 2 + 2];
    dst[0] = b'0';
    dst[1] = b'x';
    let _ = hex_encode(bytes, &mut dst[2..]);
    unsafe { String::from_utf8_unchecked(dst) }
}

impl fmt::Debug for Script {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Script {{ version: {}, args: ", self.version,)?;
        f.debug_list()
            .entries(self.args.iter().map(|arg| prefix_hex(arg)))
            .finish()?;

        write!(
            f,
            ", reference: {}",
            OptionDisplay(
                self.reference
                    .as_ref()
                    .map(|reference| format!("{:#x}", reference))
            )
        )?;

        write!(
            f,
            ", binary: {}",
            OptionDisplay(self.binary.as_ref().map(|binary| prefix_hex(binary)))
        )?;

        write!(f, " , signed_args: ")?;

        f.debug_list()
            .entries(
                self.signed_args
                    .iter()
                    .map(|signed_arg| prefix_hex(signed_arg)),
            )
            .finish()?;

        write!(f, " }}")
    }
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
                blake2b_256(bytes).into()
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

#[cfg(test)]
mod tests {
    use super::{Script, H256};

    #[test]
    fn empty_script_type_hash() {
        let script = Script::new(0, vec![], None, None, vec![]);
        let expect =
            H256::from_hex_str("4b29eb5168ba6f74bff824b15146246109c732626abd3c0578cbf147d8e28479")
                .unwrap();
        assert_eq!(script.type_hash(), expect);
    }

    #[test]
    fn always_success_script_type_hash() {
        let always_success = include_bytes!("../../nodes_template/spec/cells/always_success");
        let script = Script::new(0, vec![], None, Some(always_success.to_vec()), vec![]);
        let expect =
            H256::from_hex_str("9f94d2511b787387638faa4a5bfd448baf21aa5fde3afaa54bb791188b5cf002")
                .unwrap();
        assert_eq!(script.type_hash(), expect);
    }

    #[test]
    fn one_script_type_hash() {
        let one = Script::new(
            0,
            vec![vec![1]],
            Some(H256::zero()),
            Some(vec![1]),
            vec![vec![1]],
        );
        let expect =
            H256::from_hex_str("afb140d0673571ed5710d220d6146d41bd8bc18a3a4ff723dad4331da5af5bb6")
                .unwrap();

        assert_eq!(one.type_hash(), expect);
    }
}
