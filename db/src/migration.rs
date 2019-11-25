use crate::{db::VERSION_KEY, internal_error, Result};
use ckb_logger::info;
use rocksdb::{
    ops::{Get, Put},
    OptimisticTransactionDB,
};

#[derive(Default)]
pub struct Migrations {
    migrations: Vec<Box<dyn Migration>>,
}

impl Migrations {
    pub fn add_migration(&mut self, migration: Box<dyn Migration>) {
        self.migrations.push(migration);
    }

    pub fn migrate(&self, db: &OptimisticTransactionDB) -> Result<()> {
        let db_version = db
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
                    m.migrate(db)
                }),
            None => {
                if let Some(m) = self.migrations.last() {
                    info!("Init database version {}", m.version());
                    db.put(VERSION_KEY, m.version()).map_err(|err| {
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
    fn migrate(&self, db: &OptimisticTransactionDB) -> Result<()>;
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
    fn migrate(&self, db: &OptimisticTransactionDB) -> Result<()> {
        db.put(VERSION_KEY, &self.version)
            .map_err(|err| internal_error(format!("failed to migrate the database: {}", err)))
    }

    fn version(&self) -> &str {
        &self.version
    }
}
