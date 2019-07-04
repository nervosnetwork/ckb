use crate::Result;

pub trait MerkleElem: Sized {
    fn merge(left: &Self, right: &Self) -> Result<Self>;
    fn serialize(&self) -> Result<Vec<u8>>;
    fn deserialize(data: Vec<u8>) -> Result<Self>;
}
