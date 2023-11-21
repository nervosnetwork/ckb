use ckb_app_config::DBConfig;
use ckb_db::ReadOnlyDB;
use ckb_db::RocksDB;
use ckb_db_schema::MIGRATION_VERSION_KEY;
use ckb_error::Error;
use indicatif::ProgressBar;
use std::sync::Arc;

use crate::{DefaultMigration, Migration, Migrations};

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
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        let db = RocksDB::open(&config, 1);
        migrations.init_db_version(&db).unwrap();
        let r = migrations.migrate(db, false).unwrap();
        assert_eq!(
            b"20191116225943".to_vec(),
            r.get_pinned_default(MIGRATION_VERSION_KEY)
                .unwrap()
                .unwrap()
                .to_vec()
        );
    }
    {
        let mut migrations = Migrations::default();
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        migrations.add_migration(Arc::new(DefaultMigration::new("20191127101121")));
        let r = migrations
            .migrate(RocksDB::open(&config, 1), false)
            .unwrap();
        assert_eq!(
            b"20191127101121".to_vec(),
            r.get_pinned_default(MIGRATION_VERSION_KEY)
                .unwrap()
                .unwrap()
                .to_vec()
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
            _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
        ) -> Result<RocksDB, Error> {
            let txn = db.transaction();
            // append 1u8 to each value of column `0`
            let mut migration = |key: &[u8], value: &[u8]| -> Result<(), Error> {
                let mut new_value = value.to_vec();
                new_value.push(1);
                txn.put(COLUMN, key, &new_value)?;
                Ok(())
            };
            db.full_traverse(COLUMN, &mut migration)?;
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
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        let db = RocksDB::open(&config, 1);
        migrations.init_db_version(&db).unwrap();
        let db = migrations.migrate(db, false).unwrap();

        let txn = db.transaction();
        txn.put(COLUMN, &[1, 1], &[1, 1, 1]).unwrap();
        txn.put(COLUMN, &[2, 2], &[2, 2, 2]).unwrap();
        txn.commit().unwrap();
    }
    {
        let mut migrations = Migrations::default();
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        migrations.add_migration(Arc::new(CustomizedMigration));
        let db = migrations
            .migrate(RocksDB::open(&config, 1), false)
            .unwrap();
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
            db.get_pinned_default(MIGRATION_VERSION_KEY)
                .unwrap()
                .unwrap()
                .to_vec()
                .as_slice()
        );
    }
}

#[test]
fn test_background_migration() {
    use ckb_stop_handler::broadcast_exit_signals;

    pub struct BackgroundMigration {
        version: String,
    }

    impl BackgroundMigration {
        pub fn new(version: &str) -> Self {
            BackgroundMigration {
                version: version.to_string(),
            }
        }
    }

    impl Migration for BackgroundMigration {
        fn run_in_background(&self) -> bool {
            true
        }

        fn migrate(
            &self,
            db: RocksDB,
            _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
        ) -> Result<RocksDB, Error> {
            let db_tx = db.transaction();
            let v = self.version.as_bytes();
            db_tx.put("1", v, &[1])?;
            db_tx.commit()?;
            Ok(db)
        }

        fn version(&self) -> &str {
            self.version.as_str()
        }
    }

    pub struct RunStopMigration {
        version: String,
    }
    impl Migration for RunStopMigration {
        fn run_in_background(&self) -> bool {
            true
        }

        fn migrate(
            &self,
            db: RocksDB,
            _pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
        ) -> Result<RocksDB, Error> {
            let db_tx = db.transaction();
            loop {
                if self.stop_background() {
                    let v = self.version.as_bytes();
                    db_tx.put("1", v, &[2])?;
                    db_tx.commit()?;
                    return Ok(db);
                }
            }
        }

        fn version(&self) -> &str {
            self.version.as_str()
        }
    }

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
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        let db = RocksDB::open(&config, 12);
        migrations.init_db_version(&db).unwrap();
        let r = migrations.migrate(db, false).unwrap();
        assert_eq!(
            b"20191116225943".to_vec(),
            r.get_pinned_default(MIGRATION_VERSION_KEY)
                .unwrap()
                .unwrap()
                .to_vec()
        );
    }
    {
        let mut migrations = Migrations::default();
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        migrations.add_migration(Arc::new(BackgroundMigration::new("20231127101121")));

        let db = ReadOnlyDB::open_cf(&config.path, vec!["4"])
            .unwrap()
            .unwrap();

        assert!(migrations.can_run_in_background(&db));
        migrations.add_migration(Arc::new(DefaultMigration::new("20191127101121")));
        assert!(!migrations.can_run_in_background(&db));
    }

    {
        let mut migrations = Migrations::default();
        migrations.add_migration(Arc::new(DefaultMigration::new("20191116225943")));
        migrations.add_migration(Arc::new(BackgroundMigration::new("20231127101121")));
        migrations.add_migration(Arc::new(BackgroundMigration::new("20241127101122")));

        let db = ReadOnlyDB::open_cf(&config.path, vec!["4"])
            .unwrap()
            .unwrap();

        assert!(migrations.can_run_in_background(&db));
        let db = migrations
            .migrate(RocksDB::open(&config, 12), true)
            .unwrap();

        // wait for background migration to finish
        std::thread::sleep(std::time::Duration::from_millis(1000));
        assert_eq!(
            b"20241127101122".to_vec(),
            db.get_pinned_default(MIGRATION_VERSION_KEY)
                .unwrap()
                .unwrap()
                .to_vec()
        );

        // confirm the background migration is executed
        let db_tx = db.transaction();
        let v = db_tx
            .get_pinned("1", "20231127101121".as_bytes())
            .unwrap()
            .unwrap()
            .to_vec();
        assert_eq!(v, vec![1]);

        let v = db_tx
            .get_pinned("1", "20241127101122".as_bytes())
            .unwrap()
            .unwrap()
            .to_vec();
        assert_eq!(v, vec![1]);
    }

    {
        let mut migrations = Migrations::default();
        migrations.add_migration(Arc::new(RunStopMigration {
            version: "20251116225943".to_string(),
        }));

        let db = ReadOnlyDB::open_cf(&config.path, vec!["4"])
            .unwrap()
            .unwrap();

        assert!(migrations.can_run_in_background(&db));
        let db = migrations
            .migrate(RocksDB::open(&config, 12), true)
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(100));
        //send stop signal
        broadcast_exit_signals();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let db_tx = db.transaction();
        let v = db_tx
            .get_pinned("1", "20251116225943".as_bytes())
            .unwrap()
            .unwrap()
            .to_vec();
        assert_eq!(v, vec![2]);
    }
}
