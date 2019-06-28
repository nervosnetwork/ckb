use bytes::Bytes;
use ckb_hash::new_blake2b;
use ckb_occupied_capacity::{Capacity, Result as CapacityResult};
use faster_hex::hex_encode;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::fmt;

// TODO: when flatbuffer work is done, remove Serialize/Deserialize here and
// implement proper From trait
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Script {
    pub args: Vec<Bytes>,
    // Code hash here can be used to refer to the data in one of the dep
    // cells of current transaction. The hash here must match the hash of
    // cell data so as to reference a dep cell.
    pub code_hash: H256,
}

impl Script {
    pub fn into_witness(self) -> Vec<Bytes> {
        let Script {
            code_hash,
            mut args,
        } = self;
        args.insert(0, Bytes::from(code_hash.to_vec()));
        args
    }

    pub fn from_witness(witness: &[Bytes]) -> Option<Self> {
        witness.split_first().and_then(|(code_hash, args)| {
            H256::from_slice(code_hash).ok().map(|code_hash| Script {
                code_hash,
                args: args.to_vec(),
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

type ScriptTuple = (Vec<Bytes>, H256);

impl Script {
    pub fn new(args: Vec<Bytes>, code_hash: H256) -> Self {
        Script { args, code_hash }
    }

    pub fn destruct(self) -> ScriptTuple {
        let Script { args, code_hash } = self;
        (args, code_hash)
    }

    pub fn hash(&self) -> H256 {
        let mut ret = [0u8; 32];
        let mut blake2b = new_blake2b();
        blake2b.update(self.code_hash.as_bytes());
        for argument in &self.args {
            blake2b.update(argument);
        }
        blake2b.finalize(&mut ret);
        ret.into()
    }

    pub fn serialized_size(&self) -> usize {
        self.args.iter().map(|b| b.len() + 4).sum::<usize>() + 4 + H256::size_of()
    }

    pub fn occupied_capacity(&self) -> CapacityResult<Capacity> {
        Capacity::bytes(self.args.iter().map(Bytes::len).sum::<usize>() + 32)
    }
}

#[cfg(test)]
mod tests {
    use super::Script;
    use crate::{Bytes, Capacity};
    use ckb_hash::blake2b_256;
    use numext_fixed_hash::{h256, H256};

    #[test]
    fn test_from_into_witness() {
        let script = Script::new(vec![Bytes::from(vec![1])], H256::zero());
        let witness = script.clone().into_witness();
        assert_eq!(Script::from_witness(&witness), Some(script));
    }

    #[test]
    fn empty_script_hash() {
        let script = Script::new(vec![], H256::zero());
        let expect = h256!("0x266cec97cbede2cfbce73666f08deed9560bdf7841a7a5a51b3a3f09da249e21");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity = Capacity::bytes(script.args.len() + 32).unwrap();
        assert_eq!(
            script.occupied_capacity().unwrap(),
            expect_occupied_capacity
        );
    }

    #[test]
    fn always_success_script_hash() {
        let always_success = include_bytes!("../../script/testdata/always_success");
        let always_success_hash: H256 = (&blake2b_256(&always_success[..])).into();

        let script = Script::new(vec![], always_success_hash);
        let expect = h256!("0x9a9a6bdbc38d4905eace1822f85237e3a1e238bb3f277aa7b7c8903441123510");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity = Capacity::bytes(script.args.len() + 32).unwrap();
        assert_eq!(
            script.occupied_capacity().unwrap(),
            expect_occupied_capacity
        );
    }

    #[test]
    fn one_script_hash() {
        let script = Script::new(vec![Bytes::from(vec![1])], H256::zero());
        let expect = h256!("0xdade0e507e27e2a5995cf39c8cf454b6e70fa80d03c1187db7a4cb2c9eab79da");
        assert_eq!(script.hash(), expect);

        let expect_occupied_capacity = Capacity::bytes(script.args.len() + 32).unwrap();
        assert_eq!(
            script.occupied_capacity().unwrap(),
            expect_occupied_capacity
        );
    }
}
