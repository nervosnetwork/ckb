// for unit test
use crate::{Col, DbBatch, Error, KeyValueDB, Result};
use ckb_util::RwLock;
use fnv::FnvHashMap;
use std::iter::FromIterator;
use std::ops::Range;
use std::sync::Arc;

pub type MemoryKey = Vec<u8>;
pub type MemoryValue = Vec<u8>;
pub type MemoryTable = FnvHashMap<Col, FnvHashMap<MemoryKey, MemoryValue>>;

#[derive(Default, Debug)]
pub struct MemoryKeyValueDB {
    db: Arc<RwLock<MemoryTable>>,
}

impl MemoryKeyValueDB {
    pub fn open(cols: usize) -> MemoryKeyValueDB {
        let table = FnvHashMap::from_iter((0..cols).map(|idx| (idx as u32, FnvHashMap::default())));
        MemoryKeyValueDB {
            db: Arc::new(RwLock::new(table)),
        }
    }
}

impl KeyValueDB for MemoryKeyValueDB {
    type Batch = MemoryDbBatch;

    fn read(&self, col: Col, key: &[u8]) -> Result<Option<MemoryValue>> {
        let db = self.db.read();

        match db.get(&col) {
            None => Err(Error::DBError(format!("column {} not found ", col))),
            Some(map) => Ok(map.get(key).cloned()),
        }
    }

    fn partial_read(&self, col: Col, key: &[u8], range: &Range<usize>) -> Result<Option<Vec<u8>>> {
        let db = self.db.read();

        match db.get(&col) {
            None => Err(Error::DBError(format!("column {} not found ", col))),
            Some(map) => Ok(map
                .get(key)
                .and_then(|data| data.get(range.start..range.end))
                .map(|slice| slice.to_vec())),
        }
    }

    fn process_read<F, Ret>(&self, col: Col, key: &[u8], process: F) -> Result<Option<Ret>>
    where
        F: FnOnce(&[u8]) -> Result<Option<Ret>>,
    {
        let db = self.db.read();
        match db.get(&col) {
            None => Err(Error::DBError(format!("column {} not found ", col))),
            Some(map) => {
                if let Some(data) = map.get(key) {
                    process(data)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn traverse<F>(&self, col: Col, mut callback: F) -> Result<()>
    where
        F: FnMut(&[u8], &[u8]) -> Result<()>,
    {
        let db = self.db.read();

        match db.get(&col) {
            None => Err(Error::DBError(format!("column {} not found ", col)))?,
            Some(map) => {
                for (key, val) in map {
                    callback(key, val)?;
                }
            }
        }
        Ok(())
    }

    fn batch(&self) -> Result<Self::Batch> {
        Ok(Self::Batch {
            operations: Vec::new(),
            db: Arc::clone(&self.db),
        })
    }
}

pub struct MemoryDbBatch {
    operations: Vec<BatchOperation>,
    db: Arc<RwLock<MemoryTable>>,
}

enum BatchOperation {
    Insert {
        col: Col,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        col: Col,
        key: Vec<u8>,
    },
}

impl DbBatch for MemoryDbBatch {
    fn insert(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<()> {
        self.operations.push(BatchOperation::Insert {
            col,
            key: key.to_vec(),
            value: value.to_vec(),
        });
        Ok(())
    }

    fn delete(&mut self, col: Col, key: &[u8]) -> Result<()> {
        self.operations.push(BatchOperation::Delete {
            col,
            key: key.to_vec(),
        });
        Ok(())
    }

    fn commit(self) -> Result<()> {
        let mut db = self.db.write();
        self.operations.into_iter().for_each(|op| match op {
            BatchOperation::Insert { col, key, value } => {
                if let Some(map) = db.get_mut(&col) {
                    map.insert(key, value);
                }
            }
            BatchOperation::Delete { col, key } => {
                if let Some(map) = db.get_mut(&col) {
                    map.remove(&key);
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read() {
        let db = MemoryKeyValueDB::open(2);
        let mut batch = db.batch().unwrap();
        batch.insert(0, &[0, 0], &[0, 0, 0]).unwrap();
        batch.insert(1, &[1, 1], &[1, 1, 1]).unwrap();
        batch.commit().unwrap();

        assert_eq!(Some(vec![0, 0, 0]), db.read(0, &[0, 0]).unwrap());
        assert_eq!(None, db.read(0, &[1, 1]).unwrap());

        assert_eq!(None, db.read(1, &[0, 0]).unwrap());
        assert_eq!(Some(vec![1, 1, 1]), db.read(1, &[1, 1]).unwrap());
    }

    #[test]
    fn write_and_partial_read() {
        let db = MemoryKeyValueDB::open(2);
        let mut batch = db.batch().unwrap();
        batch.insert(0, &[0, 0], &[5, 4, 3, 2]).unwrap();
        batch.insert(1, &[1, 1], &[1, 2, 3, 4, 5]).unwrap();
        batch.commit().unwrap();

        assert_eq!(
            Some(vec![2, 3, 4]),
            db.partial_read(1, &[1, 1], &(1..4)).unwrap()
        );
        assert_eq!(None, db.partial_read(1, &[0, 0], &(1..4)).unwrap());
        // return None when invalid range is passed
        assert_eq!(None, db.partial_read(1, &[1, 1], &(2..8)).unwrap());
        // range must be increasing
        assert_eq!(None, db.partial_read(1, &[1, 1], &(3..0)).unwrap());

        assert_eq!(
            Some(vec![4, 3, 2]),
            db.partial_read(0, &[0, 0], &(1..4)).unwrap()
        );
    }
}
