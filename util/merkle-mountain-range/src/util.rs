use crate::{MMRStore, Merge, Result, MMR};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;

#[derive(Clone)]
pub struct MemStore<T>(RefCell<HashMap<u64, T>>);

impl<T> Default for MemStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> MemStore<T> {
    fn new() -> Self {
        MemStore(RefCell::new(HashMap::new()))
    }
}

impl<T: Clone> MMRStore<T> for &MemStore<T> {
    fn get_elem(&self, pos: u64) -> Result<Option<T>> {
        Ok(self.0.borrow().get(&pos).cloned())
    }

    fn append(&mut self, pos: u64, elems: Vec<T>) -> Result<()> {
        let mut store = self.0.borrow_mut();
        for (i, elem) in elems.into_iter().enumerate() {
            store.insert(pos + i as u64, elem);
        }
        Ok(())
    }
}

pub struct MemMMR<T, M> {
    store: MemStore<T>,
    mmr_size: u64,
    merge: PhantomData<M>,
}

impl<T: Clone + Debug + PartialEq, M: Merge<Item = T>> Default for MemMMR<T, M> {
    fn default() -> Self {
        Self::new(0, Default::default())
    }
}

impl<T: Clone + Debug + PartialEq, M: Merge<Item = T>> MemMMR<T, M> {
    pub fn new(mmr_size: u64, store: MemStore<T>) -> Self {
        MemMMR {
            mmr_size,
            store,
            merge: PhantomData,
        }
    }

    pub fn store(&self) -> &MemStore<T> {
        &self.store
    }

    pub fn get_root(&self) -> Result<T> {
        let mmr = MMR::<T, M, &MemStore<T>>::new(self.mmr_size, &self.store);
        mmr.get_root()
    }

    pub fn push(&mut self, elem: T) -> Result<u64> {
        let mut mmr = MMR::<T, M, &MemStore<T>>::new(self.mmr_size, &self.store);
        let pos = mmr.push(elem)?;
        self.mmr_size = mmr.mmr_size();
        mmr.commit()?;
        Ok(pos)
    }
}
