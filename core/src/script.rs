use faster_hex::hex_encode;
use hash::blake2b_256;
use numext_fixed_hash::{h256, H256};
use occupied_capacity::HasOccupiedCapacity;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;

pub const ALWAYS_SUCCESS_HASH: H256 = h256!("0x1");

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, HasOccupiedCapacity)]
pub struct Script {
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
        write!(f, "Script {{ args: ")?;
        f.debug_list()
            .entries(self.args.iter().map(|arg| prefix_hex(arg)))
            .finish()?;

        write!(f, ", binary_hash: {:#x}", self.binary_hash,)?;

        write!(f, " }}")
    }
}

type ScriptTuple = (Vec<Vec<u8>>, H256);

const VEC_WRITE_ALL_EXPECT: &str =
    "Essentially, Vec::write_all invoke extend_from_slice, should not fail";

impl Script {
    pub fn new(args: Vec<Vec<u8>>, binary_hash: H256) -> Self {
        Script { args, binary_hash }
    }

    pub fn always_success() -> Self {
        Self::new(vec![], ALWAYS_SUCCESS_HASH)
    }

    pub fn destruct(self) -> ScriptTuple {
        let Script { args, binary_hash } = self;
        (args, binary_hash)
    }

    pub fn hash(&self) -> H256 {
        let mut bytes = vec![];
        bytes
            .write_all(self.binary_hash.as_bytes())
            .expect(VEC_WRITE_ALL_EXPECT);
        for argument in &self.args {
            bytes.write_all(argument).expect(VEC_WRITE_ALL_EXPECT);
        }
        blake2b_256(bytes).into()
    }
}

#[cfg(test)]
mod tests {
    use super::{h256, Script, H256};
    use hash::blake2b_256;
    use occupied_capacity::OccupiedCapacity;

    #[test]
    fn empty_script_hash() {
        let script = Script::new(vec![], H256::zero());
        let expect = h256!("0x266cec97cbede2cfbce73666f08deed9560bdf7841a7a5a51b3a3f09da249e21");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity =
            script.args.occupied_capacity() + script.binary_hash.occupied_capacity();
        assert_eq!(script.occupied_capacity(), expect_occupied_capacity);
    }

    #[test]
    fn always_success_script_hash() {
        let always_success = include_bytes!("../../resource/specs/cells/always_success");
        let always_success_hash: H256 = (&blake2b_256(&always_success[..])).into();

        let script = Script::new(vec![], always_success_hash);
        let expect = h256!("0x9a9a6bdbc38d4905eace1822f85237e3a1e238bb3f277aa7b7c8903441123510");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity =
            script.args.occupied_capacity() + script.binary_hash.occupied_capacity();
        assert_eq!(script.occupied_capacity(), expect_occupied_capacity);
    }

    #[test]
    fn one_script_hash() {
        let one = Script::new(vec![vec![1]], H256::zero());
        let expect = h256!("0xdade0e507e27e2a5995cf39c8cf454b6e70fa80d03c1187db7a4cb2c9eab79da");
        assert_eq!(one.hash(), expect);

        let expect_occupied_capacity =
            one.args.occupied_capacity() + one.binary_hash.occupied_capacity();
        assert_eq!(one.occupied_capacity(), expect_occupied_capacity);
    }
}
