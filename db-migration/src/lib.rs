use ckb_db::RocksDB;
use ckb_error::{Error, InternalErrorKind};
use ckb_logger::{error, info};
use console::Term;
pub use indicatif::{HumanDuration, MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::collections::BTreeMap;
use std::rc::Rc;

pub const VERSION_KEY: &[u8] = b"db-version";

fn internal_error(reason: String) -> Error {
    InternalErrorKind::Database.reason(reason).into()
}

#[derive(Default)]
pub struct Migrations {
    migrations: BTreeMap<String, Box<dyn Migration>>,
}

impl Migrations {
    pub fn new() -> Self {
        Migrations {
            migrations: BTreeMap::new(),
        }
    }

    pub fn add_migration(&mut self, migration: Box<dyn Migration>) {
        self.migrations
            .insert(migration.version().to_string(), migration);
    }

    pub fn migrate(&self, mut db: RocksDB) -> Result<RocksDB, Error> {
        let db_version = db
            .get_pinned_default(VERSION_KEY)
            .map_err(|err| {
                internal_error(format!("failed to get the version of database: {}", err))
            })?
            .map(|version_bytes| {
                String::from_utf8(version_bytes.to_vec()).expect("version bytes to utf8")
            });

        match db_version {
            Some(ref v) => {
                info!("Current database version {}", v);
                if let Some(m) = self.migrations.values().last() {
                    if m.version() < v.as_str() {
                        error!(
                            "Database downgrade detected. \
                            The database schema version is newer than client schema version,\
                            please upgrade to the newer version"
                        );
                        return Err(internal_error(
                            "Database downgrade is not supported".to_string(),
                        ));
                    }
                }

                let mpb = Rc::new(MultiProgress::new());
                let migrations: BTreeMap<_, _> = self
                    .migrations
                    .iter()
                    .filter(|(mv, _)| mv.as_str() > v.as_str())
                    .collect();
                let migrations_count = migrations.len();
                for (idx, (_, m)) in migrations.iter().enumerate() {
                    let mpbc = Rc::clone(&mpb);
                    let pb = move |count: u64| -> ProgressBar {
                        let pb = mpbc.add(ProgressBar::new(count));
                        pb.set_draw_target(ProgressDrawTarget::to_term(Term::stdout(), None));
                        pb.set_prefix(&format!("[{}/{}]", idx + 1, migrations_count));
                        pb
                    };
                    db = m.migrate(db, Box::new(pb))?;
                    db.put_default(VERSION_KEY, m.version()).map_err(|err| {
                        internal_error(format!("failed to migrate the database: {}", err))
                    })?;
                }
                mpb.join_and_clear().expect("MultiProgress join");
                Ok(db)
            }
            None => {
                if let Some(m) = self.migrations.values().last() {
                    info!("Init database version {}", m.version());
                    db.put_default(VERSION_KEY, m.version()).map_err(|err| {
                        internal_error(format!("failed to migrate the database: {}", err))
                    })?;
                }
                Ok(db)
            }
        }
    }
}

pub trait Migration {
    fn migrate(
        &self,
        _db: RocksDB,
        _pb: Box<dyn FnMut(u64) -> ProgressBar>,
    ) -> Result<RocksDB, Error>;

    /// returns migration version, use `date +'%Y%m%d%H%M%S'` timestamp format
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
    fn migrate(
        &self,
        db: RocksDB,
        _pb: Box<dyn FnMut(u64) -> ProgressBar>,
    ) -> Result<RocksDB, Error> {
        Ok(db)
    }

    fn version(&self) -> &str {
        &self.version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_app_config::DBConfig;

    #[test]
    fn test_default_migration() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("test_default_migration")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            let r = migrations.migrate(RocksDB::open(&config, 1)).unwrap();
            assert_eq!(
                b"20191116225943".to_vec(),
                r.get_pinned_default(VERSION_KEY).unwrap().unwrap().to_vec()
            );
        }
        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            migrations.add_migration(Box::new(DefaultMigration::new("20191127101121")));
            let r = migrations.migrate(RocksDB::open(&config, 1)).unwrap();
            assert_eq!(
                b"20191127101121".to_vec(),
                r.get_pinned_default(VERSION_KEY).unwrap().unwrap().to_vec()
            );
        }
    }

    #[test]
    fn test_customized_migration() {
        struct CustomizedMigration;
        const COLUMN: &str = "0";
        const VERSION: &str = "20191127101121";

        impl Migration for CustomizedMigration {
            fn migrate(
                &self,
                db: RocksDB,
                _pb: Box<dyn FnMut(u64) -> ProgressBar>,
            ) -> Result<RocksDB, Error> {
                let txn = db.transaction();
                // append 1u8 to each value of column `0`
                let migration = |key: &[u8], value: &[u8]| -> Result<(), Error> {
                    let mut new_value = value.to_vec();
                    new_value.push(1);
                    txn.put(COLUMN, key, &new_value)?;
                    Ok(())
                };
                db.traverse(COLUMN, migration)?;
                txn.commit()?;
                Ok(db)
            }

            fn version(&self) -> &str {
                VERSION
            }
        }

        let tmp_dir = tempfile::Builder::new()
            .prefix("test_customized_migration")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            let db = migrations.migrate(RocksDB::open(&config, 1)).unwrap();
            let txn = db.transaction();
            txn.put(COLUMN, &[1, 1], &[1, 1, 1]).unwrap();
            txn.put(COLUMN, &[2, 2], &[2, 2, 2]).unwrap();
            txn.commit().unwrap();
        }
        {
            let mut migrations = Migrations::default();
            migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
            migrations.add_migration(Box::new(CustomizedMigration));
            let db = migrations.migrate(RocksDB::open(&config, 1)).unwrap();
            assert!(
                vec![1u8, 1, 1, 1].as_slice()
                    == db.get_pinned(COLUMN, &[1, 1]).unwrap().unwrap().as_ref()
            );
            assert!(
                vec![2u8, 2, 2, 1].as_slice()
                    == db.get_pinned(COLUMN, &[2, 2]).unwrap().unwrap().as_ref()
            );
            assert_eq!(
                VERSION.as_bytes(),
                db.get_pinned_default(VERSION_KEY)
                    .unwrap()
                    .unwrap()
                    .to_vec()
                    .as_slice()
            );
        }
    }
}
