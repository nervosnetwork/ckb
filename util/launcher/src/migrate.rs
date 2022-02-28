//! migrate helper

use crate::migrations;
use ckb_db::{ReadOnlyDB, RocksDB};
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_db_schema::{COLUMNS, COLUMN_META};
use ckb_error::Error;
use std::cmp::Ordering;
use std::path::PathBuf;

const INIT_DB_VERSION: &str = "20191127135521";

/// migrate helper
pub struct Migrate {
    migrations: Migrations,
    path: PathBuf,
}

impl Migrate {
    /// Construct new migrate
    pub fn new<P: Into<PathBuf>>(path: P) -> Self {
        let mut migrations = Migrations::default();
        migrations.add_migration(Box::new(DefaultMigration::new(INIT_DB_VERSION)));
        migrations.add_migration(Box::new(migrations::ChangeMoleculeTableToStruct)); // since v0.35.0
        migrations.add_migration(Box::new(migrations::CellMigration)); // since v0.37.0
        migrations.add_migration(Box::new(migrations::AddNumberHashMapping)); // since v0.40.0
        migrations.add_migration(Box::new(migrations::AddExtraDataHash)); // since v0.43.0
        migrations.add_migration(Box::new(migrations::AddBlockExtensionColumnFamily)); // since v0.100.0
        migrations.add_migration(Box::new(migrations::AddChainRootMMR)); // TODO(light-client) update the comment: which version?
        migrations.add_migration(Box::new(migrations::AddBlockFilterColumnFamily)); // since v0.101.2

        Migrate {
            migrations,
            path: path.into(),
        }
    }

    /// Open read only db
    pub fn open_read_only_db(&self) -> Result<Option<ReadOnlyDB>, Error> {
        // open cf meta column for empty check
        ReadOnlyDB::open_cf(&self.path, vec![COLUMN_META])
    }

    /// Check if database's version is matched with the executable binary version.
    ///
    /// Returns
    /// - Less: The database version is less than the matched version of the executable binary.
    ///   Requires migration.
    /// - Equal: The database version is matched with the executable binary version.
    /// - Greater: The database version is greater than the matched version of the executable binary.
    ///   Requires upgrade the executable binary.
    pub fn check(&self, db: &ReadOnlyDB) -> Ordering {
        self.migrations.check(db)
    }

    /// Check whether database requires expensive migrations.
    pub fn require_expensive(&self, db: &ReadOnlyDB) -> bool {
        self.migrations.expensive(db)
    }

    /// Open bulk load db.
    pub fn open_bulk_load_db(&self) -> Result<Option<RocksDB>, Error> {
        RocksDB::prepare_for_bulk_load_open(&self.path, COLUMNS)
    }

    /// Perform migrate.
    pub fn migrate(self, db: RocksDB) -> Result<RocksDB, Error> {
        self.migrations.migrate(db)
    }

    /// Perform init_db_version.
    pub fn init_db_version(self, db: &RocksDB) -> Result<(), Error> {
        self.migrations.init_db_version(db)
    }
}
