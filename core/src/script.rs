use faster_hex::hex_encode;
use hash::blake2b_256;
use numext_fixed_hash::H256;
use occupied_capacity::OccupiedCapacity;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::mem;

pub const ALWAYS_SUCCESS_HASH: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1,
];

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Script {
    pub version: u8,
    pub args: Vec<Vec<u8>>,
    // Binary hash here can be used to refer to binary in one of the dep
    // cells of current transaction. The hash here must match the hash of
    // cell data so as to reference a dep cell.
    pub binary_hash: H256,
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
        write!(f, "Script {{ version: {}, args: ", self.version,)?;
        f.debug_list()
            .entries(self.args.iter().map(|arg| prefix_hex(arg)))
            .finish()?;

        write!(f, ", binary_hash: {:#x}", self.binary_hash,)?;

        write!(f, " }}")
    }
}

type ScriptTuple = (u8, Vec<Vec<u8>>, H256);

const VEC_WRITE_ALL_EXPECT: &str =
    "Essentially, Vec::write_all invoke extend_from_slice, should not fail";

impl Script {
    pub fn new(version: u8, args: Vec<Vec<u8>>, binary_hash: H256) -> Self {
        Script {
            version,
            args,
            binary_hash,
        }
    }

    pub fn always_success() -> Self {
        Self::new(0, vec![], H256(ALWAYS_SUCCESS_HASH))
    }

    pub fn destruct(self) -> ScriptTuple {
        let Script {
            version,
            args,
            binary_hash,
        } = self;
        (version, args, binary_hash)
    }

    pub fn hash(&self) -> H256 {
        match self.version {
            0 => {
                let mut bytes = vec![];
                bytes
                    .write_all(self.binary_hash.as_bytes())
                    .expect(VEC_WRITE_ALL_EXPECT);
                for argument in &self.args {
                    bytes.write_all(argument).expect(VEC_WRITE_ALL_EXPECT);
                }
                blake2b_256(bytes).into()
            }
            _ => H256::zero(),
        }
    }
}

impl OccupiedCapacity for Script {
    fn occupied_capacity(&self) -> usize {
        mem::size_of::<u8>() + self.args.occupied_capacity() + self.binary_hash.occupied_capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::{Script, H256};
    use hash::blake2b_256;

    #[test]
    fn empty_script_hash() {
        let script = Script::new(0, vec![], H256::zero());
        let expect =
            H256::from_hex_str("266cec97cbede2cfbce73666f08deed9560bdf7841a7a5a51b3a3f09da249e21")
                .unwrap();
        assert_eq!(script.hash(), expect);
    }

    #[test]
    fn always_success_script_hash() {
        let always_success = include_bytes!("../../resource/specs/cells/always_success");
        let always_success_hash: H256 = (&blake2b_256(&always_success[..])).into();

        let script = Script::new(0, vec![], always_success_hash);
        let expect =
            H256::from_hex_str("9a9a6bdbc38d4905eace1822f85237e3a1e238bb3f277aa7b7c8903441123510")
                .unwrap();
        assert_eq!(script.hash(), expect);
    }

    #[test]
    fn one_script_hash() {
        let one = Script::new(0, vec![vec![1]], H256::zero());
        let expect =
            H256::from_hex_str("dade0e507e27e2a5995cf39c8cf454b6e70fa80d03c1187db7a4cb2c9eab79da")
                .unwrap();

        assert_eq!(one.hash(), expect);
    }
}
