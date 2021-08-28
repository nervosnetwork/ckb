use ckb_app_config::DBConfig;
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
        migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
        let db = RocksDB::open(&config, 1);
        migrations.init_db_version(&db).unwrap();
        let r = migrations.migrate(db).unwrap();
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
        migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
        migrations.add_migration(Box::new(DefaultMigration::new("20191127101121")));
        let r = migrations.migrate(RocksDB::open(&config, 1)).unwrap();
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
        migrations.add_migration(Box::new(DefaultMigration::new("20191116225943")));
        let db = RocksDB::open(&config, 1);
        migrations.init_db_version(&db).unwrap();
        let db = migrations.migrate(db).unwrap();

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
            db.get_pinned_default(MIGRATION_VERSION_KEY)
                .unwrap()
                .unwrap()
                .to_vec()
                .as_slice()
        );
    }
}
