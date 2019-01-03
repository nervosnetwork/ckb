use crate::batch::{Batch, Col, Operation};
use crate::kvdb::{ErrorKind, KeyValueDB, Result};
use ckb_util::RwLock;
use fnv::FnvHashMap;
use std::ops::Range;

pub type MemoryKey = Vec<u8>;
pub type MemoryValue = Vec<u8>;
pub type MemoryTable = FnvHashMap<Col, FnvHashMap<MemoryKey, MemoryValue>>;

#[derive(Default, Debug)]
pub struct MemoryKeyValueDB {
    db: RwLock<MemoryTable>,
}

impl MemoryKeyValueDB {
    pub fn open(cols: usize) -> MemoryKeyValueDB {
        let mut table = FnvHashMap::with_capacity_and_hasher(cols, Default::default());
        table.insert(None, FnvHashMap::default());
        for idx in 0..cols {
            table.insert(Some(idx as u32), FnvHashMap::default());
        }
        MemoryKeyValueDB {
            db: RwLock::new(table),
        }
    }
}

impl KeyValueDB for MemoryKeyValueDB {
    fn cols(&self) -> u32 {
        self.db.read().len() as u32 - 1
    }

    fn write(&self, batch: Batch) -> Result<()> {
        let mut db = self.db.write();
        batch.operations.into_iter().for_each(|op| match op {
            Operation::Insert { col, key, value } => {
                if let Some(map) = db.get_mut(&col) {
                    map.insert(key, value);
                }
            }
            Operation::Delete { col, key } => {
                if let Some(map) = db.get_mut(&col) {
                    map.remove(&key);
                }
            }
        });
        Ok(())
    }

    fn read(&self, col: Col, key: &[u8]) -> Result<Option<MemoryValue>> {
        let db = self.db.read();

        match db.get(&col) {
            None => Err(ErrorKind::DBError(format!("column {:?} not found ", col))),
            Some(map) => Ok(map.get(key).cloned()),
        }
    }

    fn len(&self, col: Col, key: &[u8]) -> Result<Option<usize>> {
        let db = self.db.read();

        match db.get(&col) {
            None => Err(ErrorKind::DBError(format!("column {:?} not found ", col))),
            Some(map) => Ok(map.get(key).map(|data| data.len())),
        }
    }

    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>> {
        let db = self.db.read();

        match db.get(&col) {
            None => Err(ErrorKind::DBError(format!("column {:?} not found ", col))),
            Some(map) => Ok(map
                .get(key)
                .and_then(|data| data.get(range.start..range.end))
                .map(|slice| slice.to_vec())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read() {
        let db = MemoryKeyValueDB::open(2);
        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![0, 0, 0]);
        batch.insert(Some(1), vec![1, 1], vec![1, 1, 1]);
        db.write(batch).unwrap();

        assert_eq!(Some(vec![0, 0, 0]), db.read(None, &vec![0, 0]).unwrap());
        assert_eq!(None, db.read(None, &vec![1, 1]).unwrap());

        assert_eq!(None, db.read(Some(1), &vec![0, 0]).unwrap());
        assert_eq!(Some(vec![1, 1, 1]), db.read(Some(1), &vec![1, 1]).unwrap());

        //return err when col doesn't exist
        assert!(db.read(Some(2), &vec![0, 0]).is_err());
    }

    #[test]
    fn write_and_len() {
        let db = MemoryKeyValueDB::open(2);
        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![5, 4, 3, 2]);
        batch.insert(Some(1), vec![1, 1], vec![1, 2, 3, 4, 5]);
        db.write(batch).unwrap();

        assert_eq!(Some(4), db.len(None, &vec![0, 0]).unwrap());

        assert_eq!(Some(5), db.len(Some(1), &vec![1, 1]).unwrap());
        assert_eq!(None, db.len(Some(1), &vec![2, 2]).unwrap());
        assert!(db.len(Some(2), &vec![1, 1]).is_err());
    }

    #[test]
    fn write_and_partial_read() {
        let db = MemoryKeyValueDB::open(2);
        let mut batch = Batch::default();
        batch.insert(None, vec![0, 0], vec![5, 4, 3, 2]);
        batch.insert(Some(1), vec![1, 1], vec![1, 2, 3, 4, 5]);
        db.write(batch).unwrap();

        assert_eq!(
            Some(vec![2, 3, 4]),
            db.partial_read(Some(1), &vec![1, 1], &(1..4)).unwrap()
        );
        assert_eq!(
            None,
            db.partial_read(Some(1), &vec![0, 0], &(1..4)).unwrap()
        );
        // return None when invalid range is passed
        assert_eq!(
            None,
            db.partial_read(Some(1), &vec![1, 1], &(2..8)).unwrap()
        );
        // range must be increasing
        assert_eq!(
            None,
            db.partial_read(Some(1), &vec![1, 1], &(3..0)).unwrap()
        );
        //return err when col doesn't exist
        assert!(db.partial_read(Some(2), &vec![0, 0], &(0..1)).is_err());

        assert_eq!(
            Some(vec![4, 3, 2]),
            db.partial_read(None, &vec![0, 0], &(1..4)).unwrap()
        );
    }
}
