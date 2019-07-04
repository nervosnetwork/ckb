use crate::header::Header;
use ckb_hash::Blake2bWriter;
use ckb_merkle_mountain_range_core::{MerkleElem, Result};
use failure::bail;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use std::io::Write;

#[derive(Eq, PartialEq, Clone, Debug, Default)]
pub struct HashWithTD {
    hash: H256,
    td: U256,
}

impl HashWithTD {
    pub fn new(hash: H256, td: U256) -> Self {
        HashWithTD { hash, td }
    }

    pub fn hash(&self) -> &H256 {
        &self.hash
    }

    pub fn td(&self) -> &U256 {
        &self.td
    }

    pub fn destruct(self) -> (H256, U256) {
        let HashWithTD { hash, td } = self;
        (hash, td)
    }
}

impl From<&Header> for HashWithTD {
    fn from(header: &Header) -> HashWithTD {
        HashWithTD::new(header.hash().to_owned(), header.difficulty().to_owned())
    }
}

impl MerkleElem for HashWithTD {
    fn serialize(&self) -> Result<Vec<u8>> {
        let mut data = self.hash.to_vec();
        data.extend(&self.td.to_le_bytes().to_vec());
        Ok(data)
    }

    fn deserialize(data: Vec<u8>) -> Result<Self> {
        assert_eq!(data.len(), 64);
        let hash = H256::from_slice(&data[0..32])?;
        let td = U256::from_little_endian(&data[32..])?;
        Ok(HashWithTD { hash, td })
    }

    fn merge(lhs: &Self, rhs: &Self) -> Result<Self> {
        let mut hasher = Blake2bWriter::new();
        hasher.write_all(&lhs.serialize()?)?;
        hasher.write_all(&rhs.serialize()?)?;
        let hash = H256::from(hasher.finalize());
        let td = match lhs.td.checked_add(&rhs.td) {
            Some(td) => td,
            None => bail!("total difficulty overflow"),
        };
        let parent = HashWithTD { hash, td };
        Ok(parent)
    }
}
