use ckb_core::script::Script;
use numext_fixed_hash::H256;
use rocksdb::{ColumnFamily, IteratorMode, Options, DB};

use crate::ROCKSDB_COL_SCRIPT;

pub struct ScriptManager<'a> {
    cf: ColumnFamily<'a>,
    db: &'a DB,
}

impl<'a> ScriptManager<'a> {
    pub fn new(db: &'a DB) -> ScriptManager {
        let cf = db.cf_handle(ROCKSDB_COL_SCRIPT).unwrap_or_else(|| {
            db.create_cf(ROCKSDB_COL_SCRIPT, &Options::default())
                .unwrap_or_else(|_| panic!("Create ColumnFamily {} failed", ROCKSDB_COL_SCRIPT))
        });
        ScriptManager { cf, db }
    }

    pub fn add(&self, script: Script) -> Result<(), String> {
        let key_bytes = script.hash().to_vec();
        let value_bytes = bincode::serialize(&script).unwrap();
        self.db.put_cf(self.cf, key_bytes, value_bytes)?;
        Ok(())
    }

    pub fn remove(&self, hash: &H256) -> Result<Script, String> {
        let script = self.get(hash)?;
        self.db.delete_cf(self.cf, hash.as_bytes())?;
        Ok(script)
    }

    pub fn get(&self, hash: &H256) -> Result<Script, String> {
        match self.db.get_cf(self.cf, hash.as_bytes())? {
            Some(db_vec) => Ok(bincode::deserialize(&db_vec).unwrap()),
            None => Err(format!("script not found: {:#x}", hash)),
        }
    }

    pub fn list(&self) -> Result<Vec<Script>, String> {
        let mut scripts = Vec::new();
        for (key_bytes, value_bytes) in self.db.iterator_cf(self.cf, IteratorMode::Start)? {
            let key = H256::from_slice(&key_bytes).unwrap();
            let script: Script = bincode::deserialize(&value_bytes).unwrap();
            assert_eq!(key, script.hash(), "script hash not match the script");
            scripts.push(script);
        }
        Ok(scripts)
    }
}
