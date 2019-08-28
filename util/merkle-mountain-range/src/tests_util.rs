use crate::{Error, MMRStore, MerkleElem, Result};
use bytes::Bytes;
use ckb_hash::new_blake2b;
use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::rc::Rc;

#[derive(Eq, PartialEq, Clone, Debug, Default)]
pub struct NumberHash(pub Bytes);
impl TryFrom<u32> for NumberHash {
    type Error = Error;
    fn try_from(num: u32) -> Result<Self> {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&num.to_le_bytes());
        hasher.finalize(&mut hash);
        Ok(NumberHash(hash.to_vec().into()))
    }
}

impl MerkleElem for NumberHash {
    fn merge(lhs: &Self, rhs: &Self) -> Result<Self> {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&lhs.0);
        hasher.update(&rhs.0);
        hasher.finalize(&mut hash);
        Ok(NumberHash(hash.to_vec().into()))
    }
}

#[derive(Clone, Default)]
pub struct MemStore<Elem>(Rc<RefCell<HashMap<u64, Elem>>>);

impl<Elem: MerkleElem + Clone> MMRStore<Elem> for MemStore<Elem> {
    fn get_elem(&self, pos: u64) -> Result<Option<Elem>> {
        Ok(self.0.borrow().get(&pos).map(Clone::clone))
    }

    fn append(&mut self, pos: u64, elems: Vec<Elem>) -> Result<()> {
        let mut store = self.0.borrow_mut();
        for (i, elem) in elems.into_iter().enumerate() {
            store.insert(pos + i as u64, elem);
        }
        Ok(())
    }
}
