use crate::{MMRBatch, MMRStore, MerkleElem, Result, MMR};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Clone)]
pub struct MemStore<Elem>(RefCell<HashMap<u64, Elem>>);

impl<Elem> Default for MemStore<Elem> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Elem> MemStore<Elem> {
    fn new() -> Self {
        MemStore(RefCell::new(HashMap::new()))
    }
}

impl<Elem: MerkleElem + Clone> MMRStore<Elem> for &MemStore<Elem> {
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

pub struct MemMMR<Elem> {
    store: MemStore<Elem>,
    mmr_size: u64,
}

impl<Elem: MerkleElem + Clone + Debug + PartialEq> Default for MemMMR<Elem> {
    fn default() -> Self {
        Self::new(0, Default::default())
    }
}

impl<Elem: MerkleElem + Clone + Debug + PartialEq> MemMMR<Elem> {
    pub fn new(mmr_size: u64, store: MemStore<Elem>) -> Self {
        MemMMR { mmr_size, store }
    }

    pub fn store(&self) -> &MemStore<Elem> {
        &self.store
    }

    pub fn get_root(&self) -> Result<Elem> {
        let mut batch = MMRBatch::new(&self.store);
        let mmr = MMR::new(self.mmr_size, &mut batch);
        mmr.get_root()
    }

    pub fn push(&mut self, elem: Elem) -> Result<u64> {
        let mut batch = MMRBatch::new(&self.store);
        let mut mmr = MMR::new(self.mmr_size, &mut batch);
        let pos = mmr.push(elem)?;
        self.mmr_size = mmr.mmr_size();
        batch.commit()?;
        Ok(pos)
    }
}
