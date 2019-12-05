use crate::{
    db::{RocksDB, VERSION_KEY},
    internal_error, Result,
};
use ckb_logger::info;
use rocksdb::ops::{Get, Put};

#[derive(Default)]
pub struct Migrations {
    migrations: Vec<Box<dyn Migration>>,
}

impl Migrations {
    pub fn add_migration(&mut self, migration: Box<dyn Migration>) {
        self.migrations.push(migration);
    }

    pub fn migrate(&self, db: &RocksDB) -> Result<()> {
        let db_version = db
            .inner
            .get(VERSION_KEY)
            .map_err(|err| {
                internal_error(format!("failed to get the version of database: {}", err))
            })?
            .map(|version_bytes| unsafe { String::from_utf8_unchecked(version_bytes.to_vec()) });

        match db_version {
            Some(v) => self
                .migrations
                .iter()
                .filter(|m| m.version() > v.as_str())
                .try_for_each(|m| {
                    info!("Migrating database version to {}", m.version());
                    m.migrate(db)?;
                    db.inner.put(VERSION_KEY, m.version()).map_err(|err| {
                        internal_error(format!("failed to migrate the database: {}", err))
                    })
                }),
            None => {
                if let Some(m) = self.migrations.last() {
                    info!("Init database version {}", m.version());
                    db.inner.put(VERSION_KEY, m.version()).map_err(|err| {
                        internal_error(format!("failed to migrate the database: {}", err))
                    })
                } else {
                    Ok(())
                }
            }
        }
    }
}

pub trait Migration {
    fn migrate(&self, db: &RocksDB) -> Result<()>;
    /// returns migration version, use `yyyymmddhhmmss` timestamp format
    fn version(&self) -> &str;
}

pub struct DefaultMigration {
    version: String,
}

impl DefaultMigration {
    pub fn new(version: &str) -> Self {
        Self {
            version: version.to_string(),
        }
    }
}

impl Migration for DefaultMigration {
    fn migrate(&self, _db: &RocksDB) -> Result<()> {
        Ok(())
    }

    fn version(&self) -> &str {
        &self.version
    }
}
