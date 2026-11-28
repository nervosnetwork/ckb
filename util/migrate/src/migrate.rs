//! migrate helper

use crate::migrations;
use crate::sst_rebuild::{
    DiskSpaceReport, SstRebuild, SstRebuildLock, disk_space_report, promoting_marker_path,
};
use ckb_db::{ReadOnlyDB, RocksDB, Schema};
use ckb_db_migration::{DefaultMigration, Migrations};
use ckb_db_schema::{COLUMN_META, legacy};
use ckb_error::Error;
use ckb_types::core::hardfork::HardForks;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const INIT_DB_VERSION: &str = "20191127135521";

/// migrate helper
pub struct Migrate {
    migrations: Migrations,
    path: PathBuf,
    options_file: Option<PathBuf>,
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
        migrations.add_migration(Arc::new(migrations::SstRebuildRequired));

        Migrate {
            migrations,
            path: path.into(),
            options_file: None,
        }
    }

    /// Attach the node's RocksDB options file for SST rebuild validation and opening.
    pub fn with_options_file(mut self, options_file: Option<PathBuf>) -> Self {
        self.options_file = options_file;
        self
    }

    /// Acquire a shared sibling lock coordinating startup probing/opening with `--sst-rebuild`.
    pub fn acquire_shared_lock(path: &Path) -> Result<SstRebuildLock, Error> {
        SstRebuildLock::acquire_shared(path)
    }

    /// Return whether an interrupted SST rebuild cutover marker exists for `path`.
    pub fn has_pending_sst_rebuild(path: &Path) -> bool {
        promoting_marker_path(path).exists()
    }

    /// Open read only db
    pub fn open_read_only_db(&self) -> Result<Option<ReadOnlyDB>, Error> {
        let Some(schema) = RocksDB::detect_schema(&self.path)? else {
            return Ok(None);
        };
        let meta_col = match schema {
            Schema::V1 => COLUMN_META,
            Schema::Legacy => legacy::COLUMN_META,
        };
        ReadOnlyDB::open_cf_with_options_file(
            &self.path,
            vec![meta_col],
            self.options_file.as_deref(),
        )
    }

    /// Validate that any configured `options_file` does not place `wal_dir` outside `self.path`.
    pub fn check_wal_dir(&self) -> Result<(), Error> {
        if let Some(ref file) = self.options_file {
            RocksDB::check_options_file_wal_dir(&self.path, file)?;
        }
        Ok(())
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
        let Some(schema) = RocksDB::detect_schema(&self.path)? else {
            return Ok(None);
        };
        RocksDB::prepare_for_bulk_load_open_with_options(
            &self.path,
            schema,
            self.options_file.as_deref(),
        )
    }

    /// Perform migrate.
    pub fn migrate(self, db: RocksDB, run_in_background: bool) -> Result<RocksDB, Error> {
        self.migrations.migrate(db, run_in_background)
    }

    /// Perform init_db_version.
    pub fn init_db_version(self, db: &RocksDB) -> Result<(), Error> {
        self.migrations.init_db_version(db)
    }

    /// Measure the database and the free space available for the SST rebuild.
    pub fn sst_rebuild_disk_space(&self) -> Result<DiskSpaceReport, Error> {
        disk_space_report(&self.path)
    }

    /// Perform fast read-only preflight checks (paths, options, schema, legacy version) before
    /// interactive prompts or measuring database size.
    pub fn preflight_sst_rebuild(&self) -> Result<(), Error> {
        let mut rebuild = SstRebuild::new(self.path.clone())?;
        if let Some(ref options_file) = self.options_file {
            rebuild = rebuild.with_options_file(options_file.clone());
        }
        rebuild.preflight()
    }

    /// Rebuild the database into the new RocksDB key schema using external SST ingestion.
    pub fn sst_rebuild(self, allow_insufficient_space: bool) -> Result<(), Error> {
        self.sst_rebuild_with_post_swap(allow_insufficient_space, || Ok(()))
    }

    /// Rebuild the database using external SST ingestion, running `post_swap` before clearing
    /// the crash-recovery cutover marker.
    pub fn sst_rebuild_with_post_swap<F>(
        self,
        allow_insufficient_space: bool,
        post_swap: F,
    ) -> Result<(), Error>
    where
        F: FnOnce() -> Result<(), Error>,
    {
        let mut rebuild = SstRebuild::new(self.path)?;
        if let Some(options_file) = self.options_file {
            rebuild = rebuild.with_options_file(options_file);
        }
        if allow_insufficient_space {
            rebuild = rebuild.allow_insufficient_space();
        }
        rebuild.run_with_post_swap(post_swap)
    }
}
