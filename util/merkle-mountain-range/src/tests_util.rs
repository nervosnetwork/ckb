use crate::{MerkleElem, Result};
use ckb_hash::Blake2bWriter;
use failure::Error;
use std::convert::TryFrom;
use std::io::Write;

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct NumberHash(pub Vec<u8>);
impl TryFrom<u32> for NumberHash {
    type Error = Error;
    fn try_from(num: u32) -> Result<Self> {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&num.to_le_bytes())?;
        Ok(NumberHash(hasher.finalize().to_vec()))
    }
}
impl MerkleElem for NumberHash {
    fn merge(lhs: &Self, rhs: &Self) -> Result<Self> {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&lhs.0)?;
        hasher.write_all(&rhs.0)?;
        Ok(NumberHash(hasher.finalize().to_vec()))
    }
    fn deserialize(data: Vec<u8>) -> Result<Self> {
        Ok(NumberHash(data))
    }
    fn serialize(&self) -> Result<Vec<u8>> {
        Ok(self.0.clone())
    }
}
