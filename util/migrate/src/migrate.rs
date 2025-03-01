//! migrate helper

use crate::migrations;
use ckb_db::{ReadOnlyDB, RocksDB};
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_db_schema::{COLUMN_META, COLUMNS};
use ckb_error::Error;
use ckb_types::core::hardfork::HardForks;
use std::cmp::Ordering;
use std::path::PathBuf;
use std::sync::Arc;

const INIT_DB_VERSION: &str = "20191127135521";

/// migrate helper
pub struct Migrate {
    migrations: Migrations,
    path: PathBuf,
}

impl Migrate {
    /// Construct new migrate
    pub fn new<P: Into<PathBuf>>(path: P, hardforks: HardForks) -> Self {
        let mut migrations = Migrations::default();
        migrations.add_migration(Arc::new(DefaultMigration::new(INIT_DB_VERSION)));
        migrations.add_migration(Arc::new(migrations::ChangeMoleculeTableToStruct)); // since v0.35.0
        migrations.add_migration(Arc::new(migrations::CellMigration)); // since v0.37.0
        migrations.add_migration(Arc::new(migrations::AddNumberHashMapping)); // since v0.40.0
        migrations.add_migration(Arc::new(migrations::AddExtraDataHash)); // since v0.43.0
        migrations.add_migration(Arc::new(migrations::AddBlockExtensionColumnFamily)); // since v0.100.0
        migrations.add_migration(Arc::new(migrations::AddChainRootMMR)); // TODO(light-client) update the comment: which version?
        migrations.add_migration(Arc::new(migrations::AddBlockFilterColumnFamily)); // since v0.105.0
        migrations.add_migration(Arc::new(migrations::AddBlockFilterHash)); // since v0.108.0
        migrations.add_migration(Arc::new(migrations::BlockExt2019ToZero::new(hardforks))); // since v0.111.1

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
    pub fn check(&self, db: &ReadOnlyDB, include_background: bool) -> Ordering {
        self.migrations.check(db, include_background)
    }

    /// Check whether database requires expensive migrations.
    pub fn require_expensive(&self, db: &ReadOnlyDB, include_background: bool) -> bool {
        self.migrations.expensive(db, include_background)
    }

    /// Check whether the pending migrations are all background migrations.
    pub fn can_run_in_background(&self, db: &ReadOnlyDB) -> bool {
        self.migrations.can_run_in_background(db)
    }

    /// Open bulk load db.
    pub fn open_bulk_load_db(&self) -> Result<Option<RocksDB>, Error> {
        RocksDB::prepare_for_bulk_load_open(&self.path, COLUMNS)
    }

    /// Perform migrate.
    pub fn migrate(self, db: RocksDB, run_in_background: bool) -> Result<RocksDB, Error> {
        self.migrations.migrate(db, run_in_background)
    }

    /// Perform init_db_version.
    pub fn init_db_version(self, db: &RocksDB) -> Result<(), Error> {
        self.migrations.init_db_version(db)
    }
}
