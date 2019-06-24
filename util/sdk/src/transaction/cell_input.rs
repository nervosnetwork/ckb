use ckb_core::transaction::CellInput;
use rocksdb::{ColumnFamily, IteratorMode, Options, DB};

use crate::ROCKSDB_COL_CELL_INPUT;

pub struct CellInputManager<'a> {
    cf: ColumnFamily<'a>,
    db: &'a DB,
}

impl<'a> CellInputManager<'a> {
    pub fn new(db: &'a DB) -> CellInputManager {
        let cf = db.cf_handle(ROCKSDB_COL_CELL_INPUT).unwrap_or_else(|| {
            db.create_cf(ROCKSDB_COL_CELL_INPUT, &Options::default())
                .unwrap_or_else(|_| panic!("Create ColumnFamily {} failed", ROCKSDB_COL_CELL_INPUT))
        });
        CellInputManager { cf, db }
    }

    pub fn add(&self, name: &str, cell_input: CellInput) -> Result<(), String> {
        let key_bytes = name.as_bytes().to_vec();
        let value_bytes = bincode::serialize(&cell_input).unwrap();
        self.db.put_cf(self.cf, key_bytes, value_bytes)?;
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<CellInput, String> {
        let cell_input = self.get(name)?;
        self.db.delete_cf(self.cf, name.as_bytes())?;
        Ok(cell_input)
    }

    pub fn get(&self, name: &str) -> Result<CellInput, String> {
        match self.db.get_cf(self.cf, name.as_bytes())? {
            Some(db_vec) => Ok(bincode::deserialize(&db_vec).unwrap()),
            None => Err(format!("cell input ({}) key not exists", name)),
        }
    }

    pub fn list(&self) -> Result<Vec<(String, CellInput)>, String> {
        let mut pairs = Vec::new();
        for (key_bytes, value_bytes) in self.db.iterator_cf(self.cf, IteratorMode::Start)? {
            let name = String::from_utf8(key_bytes.to_vec()).unwrap();
            let cell_input: CellInput = bincode::deserialize(&value_bytes).unwrap();
            pairs.push((name, cell_input));
        }
        Ok(pairs)
    }
}
