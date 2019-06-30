use crate::MerkleElem;
use crate::Result;
use ckb_db::{Col, DbBatch, KeyValueDB};
use std::marker::PhantomData;

pub struct MMRStore<Elem, DB: Sized> {
    db: DB,
    col: Col,
    merkle_elem: PhantomData<Elem>,
}

impl<Elem: MerkleElem, DB: KeyValueDB> MMRStore<Elem, DB> {
    pub fn new(db: DB, col: Col) -> Self {
        MMRStore {
            db,
            col,
            merkle_elem: PhantomData,
        }
    }
    pub fn get_elem(&self, pos: u64) -> Result<Option<Elem>> {
        match self.db.read(self.col, &pos.to_le_bytes()[..])? {
            Some(data) => Ok(Some(Elem::deserialize(data)?)),
            None => Ok(None),
        }
    }
    pub fn append(&self, pos: u64, elems: &[Elem]) -> Result<()> {
        let mut batch = self.db.batch()?;
        for (offset, elem) in elems.into_iter().enumerate() {
            let pos: u64 = pos + (offset as u64);
            batch.insert(self.col, &pos.to_le_bytes()[..], &elem.serialize()?)?;
        }
        batch.commit()?;
        Ok(())
    }
}
