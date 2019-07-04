use crate::{Error, MMRBatch, MMRStore, MerkleElem, Result};
use ckb_hash::Blake2bWriter;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Eq, PartialEq, Clone, Debug, Default)]
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

#[derive(Clone, Default)]
pub struct MemStore<Elem>(Arc<Mutex<HashMap<u64, Elem>>>);

impl<Elem: MerkleElem> MemStore<Elem> {
    pub fn commit(&self, batch: MMRBatch<Elem>) -> Result<()> {
        let mut store = self.0.lock().unwrap();
        for (pos, elems) in batch.into_iter() {
            for (i, elem) in elems.into_iter().enumerate() {
                store.insert(pos + i as u64, elem);
            }
        }
        Ok(())
    }
}

impl<Elem: MerkleElem + Clone> MMRStore<Elem> for MemStore<Elem> {
    fn get_elem(&self, pos: u64) -> Result<Option<Elem>> {
        Ok(self.0.lock().unwrap().get(&pos).map(Clone::clone))
    }
}
