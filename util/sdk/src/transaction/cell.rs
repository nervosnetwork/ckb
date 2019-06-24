use ckb_core::transaction::{CellOutPoint, CellOutput};
use numext_fixed_hash::H256;
use rocksdb::{ColumnFamily, IteratorMode, Options, DB};

use crate::{ROCKSDB_COL_CELL, ROCKSDB_COL_CELL_ALIAS};

pub struct CellManager<'a> {
    cf: ColumnFamily<'a>,
    db: &'a DB,
}

impl<'a> CellManager<'a> {
    pub fn new(db: &'a DB) -> CellManager {
        let cf = db.cf_handle(ROCKSDB_COL_CELL).unwrap_or_else(|| {
            db.create_cf(ROCKSDB_COL_CELL, &Options::default())
                .unwrap_or_else(|_| panic!("Create ColumnFamily {} failed", ROCKSDB_COL_CELL))
        });
        CellManager { cf, db }
    }

    pub fn add(&self, name: &str, cell: CellOutput) -> Result<(), String> {
        let key_bytes = name.as_bytes().to_vec();
        let value_bytes = bincode::serialize(&cell).unwrap();
        self.db.put_cf(self.cf, key_bytes, value_bytes)?;
        Ok(())
    }

    pub fn remove(&self, name: &str) -> Result<CellOutput, String> {
        let cell = self.get(name)?;
        self.db.delete_cf(self.cf, name.as_bytes())?;
        Ok(cell)
    }

    pub fn get(&self, name: &str) -> Result<CellOutput, String> {
        match self.db.get_cf(self.cf, name.as_bytes())? {
            Some(db_vec) => Ok(bincode::deserialize(&db_vec).unwrap()),
            None => Err(format!("cell not exists: {}", name)),
        }
    }

    pub fn get_by_cell_out_point(
        &self,
        cell_out_point: &CellOutPoint,
    ) -> Result<CellOutput, String> {
        let name = from_local_cell_out_point(cell_out_point)?;
        self.get(&name)
    }

    pub fn list(&self) -> Result<Vec<(String, CellOutput)>, String> {
        let mut pairs = Vec::new();
        for (key_bytes, value_bytes) in self.db.iterator_cf(self.cf, IteratorMode::Start)? {
            let name = String::from_utf8(key_bytes.to_vec()).unwrap();
            let cell: CellOutput = bincode::deserialize(&value_bytes).unwrap();
            pairs.push((name, cell));
        }
        Ok(pairs)
    }
}

pub fn to_local_cell_out_point(name: &str) -> CellOutPoint {
    let mut tx_hash = H256::zero();
    for (i, byte) in name.as_bytes().iter().enumerate() {
        tx_hash.set_byte(i, *byte);
    }
    let index = std::u32::MAX;
    CellOutPoint { tx_hash, index }
}

pub fn from_local_cell_out_point(cell_out_point: &CellOutPoint) -> Result<String, String> {
    if cell_out_point.index == std::u32::MAX {
        let mut name_bytes = Vec::new();
        for byte in cell_out_point.tx_hash.as_bytes() {
            name_bytes.push(*byte);
        }
        String::from_utf8(name_bytes).map_err(|err| err.to_string())
    } else {
        Err("Local cell's CellOutPoint.index must be std::u32::MAX".to_owned())
    }
}

pub struct CellAliasManager<'a> {
    cf: ColumnFamily<'a>,
    db: &'a DB,
}

impl<'a> CellAliasManager<'a> {
    pub fn new(db: &'a DB) -> CellAliasManager {
        let cf = db.cf_handle(ROCKSDB_COL_CELL_ALIAS).unwrap_or_else(|| {
            db.create_cf(ROCKSDB_COL_CELL_ALIAS, &Options::default())
                .unwrap_or_else(|_| panic!("Create ColumnFamily {} failed", ROCKSDB_COL_CELL_ALIAS))
        });
        CellAliasManager { cf, db }
    }

    pub fn add(&self, alias: &CellOutPoint, name: &str) -> Result<(), String> {
        let key_bytes = bincode::serialize(alias).unwrap();
        let value_bytes = name.as_bytes().to_vec();
        self.db
            .put_cf(self.cf, key_bytes.clone(), value_bytes.clone())?;
        self.db.put_cf(self.cf, value_bytes, key_bytes)?;
        Ok(())
    }

    pub fn get_by_name(&self, name: &str) -> Result<CellOutPoint, String> {
        let key_bytes = name.as_bytes().to_vec();
        self.db
            .get_cf(self.cf, key_bytes)?
            .map(|value_bytes| bincode::deserialize(&value_bytes).unwrap())
            .ok_or_else(|| format!("name not found: name={:?}", name))
    }

    pub fn get(&self, alias: &CellOutPoint) -> Result<String, String> {
        let key_bytes = bincode::serialize(alias).unwrap();
        self.db
            .get_cf(self.cf, key_bytes)?
            .map(|value_bytes| String::from_utf8(value_bytes.to_vec()).unwrap())
            .ok_or_else(|| format!("name not found: alias={:?}", alias))
    }

    pub fn remove_by_name(&self, name: &str) -> Result<CellOutPoint, String> {
        let alias = self.get_by_name(name)?;
        self.remove(&alias)?;
        let key_bytes = name.as_bytes();
        self.db.delete_cf(self.cf, key_bytes)?;
        Ok(alias)
    }

    fn remove(&self, alias: &CellOutPoint) -> Result<(), String> {
        let key_bytes = bincode::serialize(alias).unwrap();
        self.db.delete_cf(self.cf, key_bytes)?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<(CellOutPoint, String)>, String> {
        let mut pairs = Vec::new();
        for (key_bytes, value_bytes) in self.db.iterator_cf(self.cf, IteratorMode::Start)? {
            let alias_opt: Option<CellOutPoint> = bincode::deserialize(&key_bytes).ok();
            if let Some(alias) = alias_opt {
                let name = String::from_utf8(value_bytes.to_vec()).unwrap();
                pairs.push((alias, name));
            } else {
                let alias = bincode::deserialize(&value_bytes).unwrap();
                let name = String::from_utf8(key_bytes.to_vec()).unwrap();
                pairs.push((alias, name));
            }
        }
        Ok(pairs)
    }
}
