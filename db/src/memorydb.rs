// for unit test
use crate::{Col, DbBatch, ErrorKind, KeyValueDB, Result};
use ckb_util::RwLock;
use fnv::FnvHashMap;
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
        let mut table = FnvHashMap::with_capacity_and_hasher(cols, Default::default());
        table.insert(None, FnvHashMap::default());
        for idx in 0..cols {
            table.insert(Some(idx as u32), FnvHashMap::default());
        }
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
            None => Err(ErrorKind::DBError(format!("column {:?} not found ", col))),
            Some(map) => Ok(map.get(key).cloned()),
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
        batch.insert(None, &[0, 0], &[0, 0, 0]);
        batch.insert(Some(1), &[1, 1], &[1, 1, 1]);
        batch.commit();

        assert_eq!(Some(vec![0, 0, 0]), db.read(None, &[0, 0]).unwrap());
        assert_eq!(None, db.read(None, &[1, 1]).unwrap());

        assert_eq!(None, db.read(Some(1), &[0, 0]).unwrap());
        assert_eq!(Some(vec![1, 1, 1]), db.read(Some(1), &[1, 1]).unwrap());
    }

    #[test]
    fn write_and_partial_read() {
        let db = MemoryKeyValueDB::open(2);
        let mut batch = db.batch().unwrap();
        batch.insert(None, &[0, 0], &[5, 4, 3, 2]);
        batch.insert(Some(1), &[1, 1], &[1, 2, 3, 4, 5]);
        batch.commit();

        assert_eq!(
            Some(vec![2, 3, 4]),
            db.partial_read(Some(1), &[1, 1], &(1..4)).unwrap()
        );
        assert_eq!(None, db.partial_read(Some(1), &[0, 0], &(1..4)).unwrap());
        // return None when invalid range is passed
        assert_eq!(None, db.partial_read(Some(1), &[1, 1], &(2..8)).unwrap());
        // range must be increasing
        assert_eq!(None, db.partial_read(Some(1), &[1, 1], &(3..0)).unwrap());

        assert_eq!(
            Some(vec![4, 3, 2]),
            db.partial_read(None, &[0, 0], &(1..4)).unwrap()
        );
    }
}
