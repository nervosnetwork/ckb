use bytes::Bytes;
use ckb_hash::new_blake2b;
use ckb_occupied_capacity::{Capacity, Result as CapacityResult};
use failure::{err_msg, Error as FailureError};
use faster_hex::hex_encode;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fmt;

// NOTE: we could've used enum as well in the wire format, but as of
// flatbuffer 1.11.0, unused constants will be generated in the Rust
// code for enum types, resulting in both compiler warnings and clippy
// errors. So for now we are sticking to a single integer in the wire
// format, and only use enums in core data structures.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ScriptHashType {
    Data = 0,
    Type = 1,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
}

impl TryFrom<u8> for ScriptHashType {
    type Error = FailureError;

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(ScriptHashType::Data),
            1 => Ok(ScriptHashType::Type),
            _ => Err(err_msg(format!("Invalid string hash type {}", v))),
        }
    }
}

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Script {
    pub args: Vec<Bytes>,
    // Code hash here can be used to refer to the data in one of the dep
    // cells of current transaction. The hash here must match the hash of
    // cell data so as to reference a dep cell.
    pub code_hash: H256,
    pub hash_type: ScriptHashType,
}

impl Script {
    pub fn into_witness(self) -> Vec<Bytes> {
        let Script {
            code_hash,
            mut args,
            ..
        } = self;
        args.insert(0, Bytes::from(code_hash.to_vec()));
        args
    }

    pub fn from_witness(witness: &[Bytes]) -> Option<Self> {
        witness.split_first().and_then(|(code_hash, args)| {
            H256::from_slice(code_hash).ok().map(|code_hash| Script {
                code_hash,
                args: args.to_vec(),
                hash_type: ScriptHashType::Data,
            })
        })
    }
}

fn prefix_hex(bytes: &[u8]) -> String {
    let mut dst = vec![0u8; bytes.len() * 2 + 2];
    dst[0] = b'0';
    dst[1] = b'x';
    hex_encode(bytes, &mut dst[2..]).expect("hex encode buffer checked");
    unsafe { String::from_utf8_unchecked(dst) }
}

impl fmt::Debug for Script {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Script {{ args: ")?;
        f.debug_list()
            .entries(self.args.iter().map(|arg| prefix_hex(arg)))
            .finish()?;

        write!(f, ", code_hash: {:#x}", self.code_hash,)?;

        write!(f, " }}")
    }
}

type ScriptTuple = (Vec<Bytes>, H256, ScriptHashType);

impl Script {
    pub fn new(args: Vec<Bytes>, code_hash: H256, hash_type: ScriptHashType) -> Self {
        Script {
            args,
            code_hash,
            hash_type,
        }
    }

    pub fn destruct(self) -> ScriptTuple {
        let Script {
            args,
            code_hash,
            hash_type,
        } = self;
        (args, code_hash, hash_type)
    }

    pub fn hash(&self) -> H256 {
        let mut ret = [0u8; 32];
        let mut blake2b = new_blake2b();
        blake2b.update(self.code_hash.as_bytes());
        blake2b.update(&[self.hash_type.to_owned() as u8]);
        for argument in &self.args {
            blake2b.update(argument);
        }
        blake2b.finalize(&mut ret);
        ret.into()
    }

    pub fn serialized_size(&self) -> usize {
        self.args.iter().map(|b| b.len() + 4).sum::<usize>() + 4 + H256::size_of() + 1
    }

    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        Capacity::bytes(self.args.iter().map(Bytes::len).sum::<usize>() + 32 + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{Script, ScriptHashType};
    use crate::{Bytes, Capacity};
    use ckb_hash::blake2b_256;
    use numext_fixed_hash::{h256, H256};

    #[test]
    fn test_from_into_witness() {
        let script = Script::new(
            vec![Bytes::from(vec![1])],
            H256::zero(),
            ScriptHashType::Data,
        );
        let witness = script.clone().into_witness();
        assert_eq!(Script::from_witness(&witness), Some(script));
    }

    #[test]
    fn empty_script_hash() {
        let script = Script::new(vec![], H256::zero(), ScriptHashType::Data);
        let expect = h256!("0xc371c8d6a0aed6018e91202d047c35055cfb0228e6709f1cd1d5f756525628b9");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity = Capacity::bytes(script.args.len() + 32 + 1).unwrap();
        assert_eq!(
            script.occupied_capacity().unwrap(),
            expect_occupied_capacity
        );
    }

    #[test]
    fn always_success_script_hash() {
        let always_success = include_bytes!("../../script/testdata/always_success");
        let always_success_hash: H256 = (&blake2b_256(&always_success[..])).into();

        let script = Script::new(vec![], always_success_hash, ScriptHashType::Data);
        let expect = h256!("0xa6ed2edad0d48a3d58d0bec407ddf2e40ddd5f533d7059a160149f4021c2a589");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity = Capacity::bytes(script.args.len() + 32 + 1).unwrap();
        assert_eq!(
            script.occupied_capacity().unwrap(),
            expect_occupied_capacity
        );
    }

    #[test]
    fn one_script_hash() {
        let script = Script::new(
            vec![Bytes::from(vec![1])],
            H256::zero(),
            ScriptHashType::Data,
        );
        let expect = h256!("0xcd5b0c29b8f5528d3a75e3918576db4d962a1d4b315dff7d3c50818cc373b3f5");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity = Capacity::bytes(script.args.len() + 32 + 1).unwrap();
        assert_eq!(
            script.occupied_capacity().unwrap(),
            expect_occupied_capacity
        );
    }
}
