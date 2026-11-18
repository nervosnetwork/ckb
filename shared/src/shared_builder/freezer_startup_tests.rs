use super::*;
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_META, META_ARCHIVE_NEXT_RECORD};

fn open(path: &Path, enabled: bool, ancient: Option<PathBuf>) -> Result<ChainDB, Error> {
    build_store(
        RocksDB::open_in(path, COLUMNS),
        StoreConfig {
            freezer_enable: enabled,
            ..Default::default()
        },
        ancient,
    )
}

#[test]
fn rejected_freezer_configuration_leaves_no_background_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let ancient = directory.path().join("ancient");
    let config = DBConfig {
        path: directory.path().join("db"),
        ..Default::default()
    };
    let (mut handle, mut stopped, runtime) = ckb_async_runtime::new_global_runtime(Some(2));
    let builder = SharedBuilder::new(
        "test",
        directory.path(),
        &config,
        Some(ancient.clone()),
        handle.clone(),
        Consensus::default(),
    )
    .unwrap();
    drop(
        build_store(
            builder.db.clone(),
            StoreConfig {
                freezer_enable: true,
                ..Default::default()
            },
            Some(ancient),
        )
        .unwrap(),
    );
    assert!(
        builder
            .store_config(StoreConfig::default())
            .build()
            .is_err()
    );
    handle.drop_guard();
    runtime.block_on(async {
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), stopped.recv())
                .await
                .expect("failed startup retained a background runtime guard"),
            None
        );
    });
}

#[test]
fn first_activation_is_permanent_before_any_block_is_archived() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let ancient = directory.path().join("ancient");
    let legacy = RocksDB::open_in(&path, COLUMNS - 1);
    let mut batch = legacy.new_write_batch();
    batch
        .put(COLUMN_BLOCK_BODY, b"retained", b"original")
        .unwrap();
    legacy.write_sync(&batch).unwrap();
    drop((batch, legacy));

    let disabled = open(&path, false, Some(ancient.clone())).unwrap();
    assert!(disabled.freezer().is_none());
    assert!(
        disabled
            .get(COLUMN_META, META_ARCHIVE_NEXT_RECORD)
            .is_none()
    );
    assert!(!ancient.exists());
    drop(disabled);

    let enabled = open(&path, true, Some(ancient.clone())).unwrap();
    assert_eq!(enabled.freezer().unwrap().number(), 1);
    assert_eq!(
        enabled
            .get(COLUMN_META, META_ARCHIVE_NEXT_RECORD)
            .unwrap()
            .as_ref(),
        1u64.to_le_bytes()
    );
    assert_eq!(
        enabled
            .get(COLUMN_BLOCK_BODY, b"retained")
            .unwrap()
            .as_ref(),
        b"original"
    );
    drop(enabled);

    for location in [Some(ancient.clone()), None] {
        let error = open(&path, false, location).err().unwrap();
        assert_eq!(
            error
                .downcast_ref::<ckb_error::InternalError>()
                .unwrap()
                .kind(),
            InternalErrorKind::Config
        );
        assert!(
            error
                .to_string()
                .contains("Freezer cannot be disabled once enabled")
        );
    }
    let reopened = open(&path, true, Some(ancient)).unwrap();
    assert_eq!(reopened.freezer().unwrap().number(), 1);
    assert_eq!(
        reopened
            .get(COLUMN_BLOCK_BODY, b"retained")
            .unwrap()
            .as_ref(),
        b"original"
    );
    let missing = ChainDB::new(reopened.into_inner(), StoreConfig::default());
    assert!(missing.recover_archive().is_err());
}

#[test]
fn an_archive_without_a_database_cursor_cannot_be_bypassed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let ancient = directory.path().join("ancient");
    drop(Freezer::open(ancient.clone()).unwrap());
    let error = open(&path, false, Some(ancient.clone())).err().unwrap();
    assert!(
        error
            .to_string()
            .contains("Freezer cannot be disabled once enabled")
    );
    let store = open(&path, true, Some(ancient)).unwrap();
    assert_eq!(store.freezer().unwrap().number(), 1);
    assert!(store.get(COLUMN_META, META_ARCHIVE_NEXT_RECORD).is_some());
}

#[test]
fn a_legacy_archive_index_cannot_be_ignored_to_disable_freezer() {
    let directory = tempfile::tempdir().unwrap();
    let ancient = directory.path().join("ancient");
    std::fs::create_dir(&ancient).unwrap();
    std::fs::write(ancient.join("INDEX"), b"legacy archive").unwrap();
    let error = open(&directory.path().join("db"), false, Some(ancient))
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("Freezer cannot be disabled once enabled")
    );
}

#[test]
fn activation_requires_a_path_and_reopen_requires_the_original_archive() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    assert!(open(&path, true, None).is_err());
    let untouched = open(&path, false, None).unwrap();
    assert!(
        untouched
            .get(COLUMN_META, META_ARCHIVE_NEXT_RECORD)
            .is_none()
    );
    drop(untouched);

    let ancient = directory.path().join("ancient");
    drop(open(&path, true, Some(ancient.clone())).unwrap());
    let wrong = directory.path().join("wrong");
    assert!(open(&path, true, Some(wrong.clone())).is_err());
    assert!(!wrong.exists());
    std::fs::rename(ancient.join("COMMIT"), ancient.join("saved-commit")).unwrap();
    assert!(open(&path, true, Some(ancient.clone())).is_err());
    assert!(!ancient.join("COMMIT").exists());
    std::fs::rename(ancient.join("saved-commit"), ancient.join("COMMIT")).unwrap();
    drop(open(&path, true, Some(ancient)).unwrap());
}
