use crate::generation::{GENERATION_KEY, PAYLOAD_COLUMNS, physical_name};
use crate::{CollectionAbort, CollectionOptions, DBIterator, IteratorMode, RocksDB};
use ckb_db_schema::{COLUMN_META, COLUMNS, Col, META_ARCHIVE_COLLECTED};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

type Rows = BTreeMap<Vec<u8>, Vec<u8>>;

#[test]
fn retirement_reclaims_wal_even_if_publication_was_already_flushed() {
    check_wal_retirement(true);
}

#[test]
fn migration_retirement_reclaims_wal() {
    check_wal_retirement(false);
}

fn check_wal_retirement(collect: bool) {
    use rocksdb::ops::{Flush, FlushWal};
    use std::fs;

    for atomic_flush in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let options_file = directory.path().join("options");
        fs::write(
            &options_file,
            format!(
                "[Version]\noptions_file_version=1.1\n\
                 [DBOptions]\nmax_total_wal_size=4194304\natomic_flush={atomic_flush}\n\
                 [CFOptions \"default\"]\nwrite_buffer_size=67108864\n"
            ),
        )
        .unwrap();
        let config = ckb_app_config::DBConfig {
            path: directory.path().join("db"),
            options_file: Some(options_file),
            ..Default::default()
        };
        let mut db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
        let column = if collect { "2" } else { "1" };
        db.put_default(GENERATION_KEY, 0u64.to_le_bytes()).unwrap();
        let txn = db.transaction();
        txn.put(column, b"old", &vec![7; 2 << 20]).unwrap();
        txn.commit().unwrap();
        drop(txn);
        // Rotate the WAL while a payload CF still depends on its predecessor.
        db.inner.flush().unwrap();
        if collect {
            let old = db.get_snapshot();
            db.collect_payload(
                &CollectionOptions::default(),
                |_, _, _, _| Ok(false),
                // The default CF is empty by retirement. A flush without first
                // refreshing its marker would leave the obsolete WAL pinned.
                || db.inner.flush().unwrap(),
            )
            .unwrap();
            assert_eq!(
                old.get_pinned(column, b"old").unwrap().unwrap().len(),
                2 << 20
            );
        } else {
            db.drop_cf(column).unwrap();
            db.create_cf(column).unwrap();
        }
        let value = vec![9; 256 << 10];
        for key in 0..32u8 {
            let txn = db.transaction();
            txn.put(column, &[key], &value).unwrap();
            txn.commit().unwrap();
        }
        db.inner.flush_wal(false).unwrap();
        // Allow the current WAL and an in-flight flush above the soft limit.
        super::assert_wal_bounded(&config.path, 6 << 20, || {
            db.put_default(b"pressure-probe", []).unwrap();
        });
        drop(db);
        let db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
        assert_eq!(db.generation().id, u64::from(collect));
        assert!(db.get_pinned(column, b"old").unwrap().is_none());
        for key in 0..32u8 {
            assert_eq!(
                db.get_pinned(column, &[key]).unwrap().unwrap().as_ref(),
                value
            );
        }
    }
}

#[test]
fn range_deletion_captures_the_same_key_it_deletes() {
    use std::cell::Cell;

    struct Key<'a>(&'a Cell<usize>);
    impl AsRef<[u8]> for Key<'_> {
        fn as_ref(&self) -> &[u8] {
            let calls = self.0.get();
            self.0.set(calls + 1);
            if calls == 0 { b"first" } else { b"second" }
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    let mut batch = db.new_write_batch();
    batch.put("2", b"first", b"one").unwrap();
    batch.put("2", b"second", b"two").unwrap();
    db.write(&batch).unwrap();
    drop(batch);
    let before = db.get_snapshot();
    let conversions = Cell::new(0);
    db.collect_payload(
        &CollectionOptions::default(),
        |_, col, key, _| {
            if col == "2" && key == b"first" {
                let mut batch = db.new_write_batch();
                batch.delete_range(col, std::iter::once(Key(&conversions)))?;
                db.write(&batch)?;
                assert!(db.get_pinned(col, b"first")?.is_none());
                assert_eq!(db.get_pinned(col, b"second")?.unwrap().as_ref(), b"two");
            }
            Ok(true)
        },
        || {},
    )
    .unwrap();
    assert_eq!(conversions.get(), 1);
    assert!(db.get_pinned("2", b"first").unwrap().is_none());
    assert_eq!(
        db.get_pinned("2", b"second").unwrap().unwrap().as_ref(),
        b"two"
    );
    assert_eq!(
        before.get_pinned("2", b"first").unwrap().unwrap().as_ref(),
        b"one"
    );
    drop((before, db));
    let reopened = RocksDB::open_in(directory.path(), COLUMNS);
    assert!(reopened.get_pinned("2", b"first").unwrap().is_none());
    assert_eq!(
        reopened
            .get_pinned("2", b"second")
            .unwrap()
            .unwrap()
            .as_ref(),
        b"two"
    );
}

#[test]
fn a_native_partial_column_batch_preserves_old_reads_until_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let before = db.get_snapshot();
    // Body creation succeeds, then the non-body batch creates column 3 before
    // encountering an actual native name collision at column 7.
    let collision = db
        .inner
        .create_owned_cf("freezer.1.7", &rocksdb::Options::default())
        .unwrap();
    assert!(
        db.collect_payload(
            &CollectionOptions::default(),
            |_, _, _, _| Ok(true),
            || panic!("partial creation cannot publish")
        )
        .is_err()
    );
    assert_eq!(db.generation().id, 0);
    assert!(db.transaction().put("10", b"blocked", b"write").is_err());
    for col in PAYLOAD_COLUMNS {
        assert_eq!(rows(&db, col).len(), 16);
        assert!(before.get_pinned(col, b"hot/0").unwrap().is_some());
    }
    drop((collision, before, db));
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    assert_eq!(db.generation().id, 0);
    db.collect_payload(&CollectionOptions::default(), |_, _, _, _| Ok(true), || {})
        .unwrap();
    assert_eq!(db.generation().id, 1);
    for col in PAYLOAD_COLUMNS {
        assert_eq!(rows(&db, col).len(), 16);
    }
}

#[test]
fn collection_preserves_distinct_options_from_a_custom_file() {
    fn latest_options(path: &std::path::Path) -> String {
        let path = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("OPTIONS-")
            })
            .max()
            .unwrap();
        std::fs::read_to_string(path).unwrap()
    }
    fn section<'a>(text: &'a str, column: &str) -> &'a str {
        let header = format!("[CFOptions \"{column}\"]");
        text.split_once(header.as_str())
            .unwrap()
            .1
            .split("\n[")
            .next()
            .unwrap()
    }
    fn buffer_size(text: &str) -> &str {
        text.lines()
            .find_map(|line| line.trim().strip_prefix("write_buffer_size="))
            .unwrap()
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let db = RocksDB::open_in(&path, COLUMNS);
    populate(&db);
    drop(db);
    let original = latest_options(&path);
    let old_section = section(&original, "3");
    let custom_section = old_section
        .lines()
        .map(|line| {
            if line.trim().starts_with("write_buffer_size=") {
                "  write_buffer_size=4194304"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let custom = original.replacen(
        &format!("[CFOptions \"3\"]{old_section}"),
        &format!("[CFOptions \"3\"]{custom_section}"),
        1,
    );
    let options_file = directory.path().join("custom-options");
    std::fs::write(&options_file, custom).unwrap();
    let config = ckb_app_config::DBConfig {
        path: path.clone(),
        options_file: Some(options_file),
        ..Default::default()
    };
    let db = RocksDB::open_with_check(&config, COLUMNS).unwrap();
    assert!(!db.routing.batch_default_payload_cfs);
    db.collect_payload(&CollectionOptions::default(), |_, _, _, _| Ok(true), || {})
        .unwrap();
    let after = latest_options(&path);
    assert_eq!(buffer_size(section(&after, "freezer.1.3")), "4194304");
    assert_eq!(
        buffer_size(section(&after, "freezer.1.7")),
        buffer_size(section(&original, "7"))
    );
    for col in PAYLOAD_COLUMNS {
        assert_eq!(rows(&db, col).len(), 16);
    }
}

fn rows(db: &RocksDB, col: Col) -> Rows {
    let mut iter = db.iter(col, IteratorMode::Start).unwrap();
    let values = iter
        .by_ref()
        .map(|(k, v)| (k.into_vec(), v.into_vec()))
        .collect();
    iter.status().unwrap();
    values
}

fn populate(db: &RocksDB) {
    let txn = db.transaction();
    for col in PAYLOAD_COLUMNS {
        for i in 0..8 {
            for kind in ["hot", "cold"] {
                let key = format!("{kind}/{i}");
                txn.put(col, key.as_bytes(), key.as_bytes()).unwrap();
            }
        }
    }
    txn.put("10", b"live-cell", b"unchanged").unwrap();
    txn.commit().unwrap();
}

#[test]
fn old_views_survive_three_collections_and_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    db.put_default(b"external-metadata", b"retained").unwrap();
    let old = db.get_snapshot();
    let pin = old.get_pinned("2", b"cold/0").unwrap().unwrap();
    let mut iter = old.iter("2", IteratorMode::Start).unwrap();
    let published = AtomicBool::new(false);
    for expected in 1..=3 {
        let stats = db
            .collect_payload(
                &CollectionOptions::default(),
                |_, _, key, _| Ok(!key.starts_with(b"cold")),
                || {
                    published.store(true, Ordering::Relaxed);
                    assert_eq!(
                        db.get_snapshot()
                            .get_pinned("2", b"hot/0")
                            .unwrap()
                            .unwrap()
                            .as_ref(),
                        b"hot/0"
                    );
                },
            )
            .unwrap();
        assert_eq!(stats.generation, expected);
        assert_eq!(stats.retired_columns, 5);
        assert_eq!(stats.archived, if expected == 1 { 40 } else { 0 });
        assert_eq!(stats.copied, 40);
        assert!(published.load(Ordering::Relaxed));
        for col in PAYLOAD_COLUMNS {
            assert_eq!(rows(&db, col).len(), 8);
            assert_eq!(
                old.get_pinned(col, b"cold/0").unwrap().unwrap().as_ref(),
                b"cold/0"
            );
        }
    }
    assert_eq!(pin.as_ref(), b"cold/0");
    assert_eq!(iter.by_ref().count(), 16);
    iter.status().unwrap();
    assert_eq!(
        db.get_pinned("10", b"live-cell").unwrap().unwrap().as_ref(),
        b"unchanged"
    );
    assert_eq!(
        db.get_pinned_default(b"external-metadata")
            .unwrap()
            .unwrap()
            .as_ref(),
        b"retained"
    );
    drop((pin, iter));
    drop(old);
    drop(db);
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    assert_eq!(db.generation().id, 3);
    assert_eq!(
        db.get_pinned(COLUMN_META, META_ARCHIVE_COLLECTED)
            .unwrap()
            .unwrap()
            .as_ref(),
        1u64.to_le_bytes()
    );
    for col in PAYLOAD_COLUMNS {
        assert_eq!(rows(&db, col).len(), 8);
    }
}

#[test]
fn concurrent_changes_and_rollbacks_are_reconciled_from_latest_source() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let (start, started) = mpsc::channel();
    let (finish, finished) = mpsc::channel();
    let writer_db = db.clone();
    let writer = std::thread::spawn(move || {
        started.recv().unwrap();
        let txn = writer_db.transaction();
        for col in PAYLOAD_COLUMNS {
            txn.put(col, b"cold/0", b"restored-and-edited").unwrap();
            txn.delete(col, b"hot/1").unwrap();
            txn.put(col, b"inserted", b"new").unwrap();
        }
        txn.set_savepoint();
        txn.put("2", b"hot/2", b"rolled-back").unwrap();
        txn.rollback_to_savepoint().unwrap();
        txn.put("10", b"live-cell", b"foreground").unwrap();
        txn.commit().unwrap();
        drop(txn);
        let txn = writer_db.transaction();
        txn.put("3", b"never-committed", b"rollback").unwrap();
        txn.rollback().unwrap();
        drop(txn);
        finish.send(()).unwrap();
    });
    let mut started = false;
    let stats = db
        .collect_payload(
            &CollectionOptions {
                batch_bytes: 32,
                ..Default::default()
            },
            |snapshot, col, key, _| {
                if !started {
                    started = true;
                    start.send(()).unwrap();
                    finished.recv_timeout(Duration::from_secs(10)).unwrap();
                    assert_eq!(
                        snapshot
                            .get_pinned(col, b"cold/0")
                            .unwrap()
                            .unwrap()
                            .as_ref(),
                        b"cold/0"
                    );
                }
                Ok(!key.starts_with(b"cold"))
            },
            || {},
        )
        .unwrap();
    writer.join().unwrap();
    assert_eq!(stats.reconciled, 17);
    for col in PAYLOAD_COLUMNS {
        assert_eq!(
            db.get_pinned(col, b"cold/0").unwrap().unwrap().as_ref(),
            b"restored-and-edited"
        );
        assert!(db.get_pinned(col, b"hot/1").unwrap().is_none());
        assert_eq!(
            db.get_pinned(col, b"inserted").unwrap().unwrap().as_ref(),
            b"new"
        );
        assert_eq!(rows(&db, col).len(), 9);
    }
    assert_eq!(
        db.get_pinned("2", b"hot/2").unwrap().unwrap().as_ref(),
        b"hot/2"
    );
    assert!(db.get_pinned("3", b"never-committed").unwrap().is_none());
    assert_eq!(
        db.get_pinned("10", b"live-cell").unwrap().unwrap().as_ref(),
        b"foreground"
    );
}

#[test]
fn source_errors_and_limits_abort_without_switching_or_disabling_writes() {
    for failure in ["writer", "copy", "dirty", "reconcile"] {
        let directory = tempfile::tempdir().unwrap();
        let db = RocksDB::open_in(directory.path(), COLUMNS);
        populate(&db);
        let held = (failure == "writer").then(|| db.new_write_batch());
        let options = CollectionOptions {
            max_dirty_bytes: if failure == "dirty" { 1 } else { 1 << 20 },
            max_retired_generations: 2,
            writer_wait: Duration::from_millis(1),
            reconcile_timeout: if failure == "reconcile" {
                Duration::ZERO
            } else {
                Duration::from_secs(1)
            },
            batch_bytes: 16,
        };
        let mut changed = false;
        let result = db.collect_payload(
            &options,
            |_, col, _, _| {
                if failure == "copy" {
                    return Err(crate::internal_error("injected source read failure"));
                }
                if failure == "dirty" && !changed {
                    changed = true;
                    let txn = db.transaction();
                    txn.put(col, b"hot/0", b"update").unwrap();
                    txn.commit().unwrap();
                }
                Ok(true)
            },
            || panic!("aborted generation must not publish"),
        );
        let error = result.unwrap_err();
        let expected = match failure {
            "writer" => Some(CollectionAbort::WriterWait),
            "dirty" => Some(CollectionAbort::DirtyKeys),
            "reconcile" => Some(CollectionAbort::Reconciliation),
            _ => None,
        };
        assert_eq!(CollectionAbort::from_error(&error), expected);
        assert_eq!(db.generation().id, 0);
        assert!(db.get_pinned_default(GENERATION_KEY).unwrap().is_none());
        for col in PAYLOAD_COLUMNS {
            assert_eq!(rows(&db, col).len(), 16);
        }
        drop(held);
        let txn = db.transaction();
        txn.put("2", b"after-abort", b"writable").unwrap();
        txn.commit().unwrap();
        drop(txn);
        drop(db);
        let db = RocksDB::open_in(directory.path(), COLUMNS);
        assert_eq!(db.generation().id, 0);
        assert_eq!(
            db.get_pinned("2", b"after-abort")
                .unwrap()
                .unwrap()
                .as_ref(),
            b"writable"
        );
    }
}

#[test]
fn opening_discards_unpublished_partial_generations_but_rejects_missing_active_cf() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let orphan = db
        .inner
        .create_owned_cf(&physical_name(1, "2"), &rocksdb::Options::default())
        .unwrap();
    drop((orphan, db));
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    assert_eq!(db.generation().id, 0);
    assert_eq!(rows(&db, "2").len(), 16);
    // This name must be available again after recovery removed the old orphan.
    let orphan = db
        .inner
        .create_owned_cf(&physical_name(1, "2"), &rocksdb::Options::default())
        .unwrap();
    db.put_default(GENERATION_KEY, 1u64.to_le_bytes()).unwrap();
    drop((orphan, db));
    let config = ckb_app_config::DBConfig {
        path: directory.path().to_path_buf(),
        ..Default::default()
    };
    assert!(RocksDB::open_with_check(&config, COLUMNS).is_err());
}

#[test]
fn a_pinned_value_applies_retirement_backpressure_until_released() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let pin = db.get_pinned("2", b"cold/0").unwrap().unwrap();
    let options = CollectionOptions {
        max_retired_generations: 1,
        ..Default::default()
    };
    db.collect_payload(&options, |_, _, _, _| Ok(true), || {})
        .unwrap();
    assert_eq!(db.pinned_retired_generations(), 1);
    let error = db
        .collect_payload(&options, |_, _, _, _| Ok(true), || {})
        .unwrap_err();
    assert_eq!(
        CollectionAbort::from_error(&error),
        Some(CollectionAbort::RetainedReaders)
    );
    assert_eq!(db.generation().id, 1);
    assert_eq!(pin.as_ref(), b"cold/0");
    drop(pin);
    assert_eq!(db.pinned_retired_generations(), 0);
    assert_eq!(
        db.collect_payload(&options, |_, _, _, _| Ok(true), || {})
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn a_writer_started_during_copy_prevents_final_publication_until_it_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let mut held = None;
    let options = CollectionOptions {
        writer_wait: Duration::from_millis(1),
        ..Default::default()
    };
    let result = db.collect_payload(
        &options,
        |_, _, _, _| {
            held.get_or_insert_with(|| db.new_write_batch());
            Ok(true)
        },
        || panic!("live writer prevents publication"),
    );
    assert!(result.is_err());
    assert_eq!(db.generation().id, 0);
    let mut batch = held.take().unwrap();
    batch
        .put("2", b"late", b"old generation is still writable")
        .unwrap();
    db.write_sync(&batch).unwrap();
    drop(batch);
    db.collect_payload(&options, |_, _, _, _| Ok(true), || {})
        .unwrap();
    assert_eq!(
        db.get_pinned("2", b"late").unwrap().unwrap().as_ref(),
        b"old generation is still writable"
    );
}

#[test]
fn maintenance_open_resolves_the_published_physical_layout() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    db.collect_payload(
        &CollectionOptions::default(),
        |_, _, key, _| Ok(!key.starts_with(b"cold")),
        || {},
    )
    .unwrap();
    drop(db);
    let db = RocksDB::prepare_for_bulk_load_open(directory.path(), COLUMNS)
        .unwrap()
        .unwrap();
    assert_eq!(db.generation().id, 1);
    for col in PAYLOAD_COLUMNS {
        assert_eq!(rows(&db, col).len(), 8);
    }
}

#[test]
fn repeatedly_changing_one_key_only_uses_one_dirty_key_budget() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let key = b"hot/0";
    let options = CollectionOptions {
        max_dirty_bytes: key.len() + 96,
        ..Default::default()
    };
    let mut changed = false;
    let stats = db
        .collect_payload(
            &options,
            |_, _, _, _| {
                if !changed {
                    changed = true;
                    for value in [b"first".as_slice(), b"second", b"latest"] {
                        let txn = db.transaction();
                        txn.put("2", key, value).unwrap();
                        txn.commit().unwrap();
                    }
                }
                Ok(true)
            },
            || {},
        )
        .unwrap();
    assert_eq!(stats.reconciled, 1);
    assert_eq!(
        db.get_pinned("2", key).unwrap().unwrap().as_ref(),
        b"latest"
    );
}

#[test]
fn overflow_aborts_a_scan_even_when_every_row_is_archived() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let mut visited = 0;
    let options = CollectionOptions {
        max_dirty_bytes: 1,
        ..Default::default()
    };
    let error = db
        .collect_payload(
            &options,
            |_, _, _, _| {
                visited += 1;
                let txn = db.transaction();
                txn.put("2", b"hot/0", b"latest").unwrap();
                txn.commit().unwrap();
                Ok(false)
            },
            || panic!("overflow must not publish"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("dirty-key limit"));
    assert_eq!(visited, 1);
    assert_eq!(db.generation().id, 0);
    db.check_writable().unwrap();
}

#[test]
fn cancellation_cleans_up_targets_and_allows_a_new_collection() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    populate(&db);
    let cancelled = AtomicBool::new(false);
    let mut visited = 0;
    let error = db
        .collect_payload_with_cancel(
            &CollectionOptions::default(),
            |_, _, _, _| {
                visited += 1;
                cancelled.store(true, Ordering::Relaxed);
                Ok(true)
            },
            || cancelled.load(Ordering::Relaxed),
            || panic!("cancelled copy must not publish"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(
        CollectionAbort::from_error(&error),
        Some(CollectionAbort::Cancelled)
    );
    assert_eq!(visited, 1);
    assert_eq!(db.generation().id, 0);
    let txn = db.transaction();
    txn.put("2", b"after-cancel", b"writable").unwrap();
    txn.commit().unwrap();
    drop(txn);
    let stats = db
        .collect_payload(&CollectionOptions::default(), |_, _, _, _| Ok(true), || {})
        .unwrap();
    assert_eq!(stats.generation, 1);
    assert_eq!(
        db.get_pinned("2", b"after-cancel")
            .unwrap()
            .unwrap()
            .as_ref(),
        b"writable"
    );
}

#[test]
fn an_entry_larger_than_the_batch_flush_target_is_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    let value = vec![7; 4096];
    let txn = db.transaction();
    txn.put("2", b"large-entry", &value).unwrap();
    txn.commit().unwrap();
    drop(txn);
    let options = CollectionOptions {
        batch_bytes: 32,
        ..Default::default()
    };
    db.collect_payload(&options, |_, _, _, _| Ok(true), || {})
        .unwrap();
    assert_eq!(
        db.get_pinned("2", b"large-entry")
            .unwrap()
            .unwrap()
            .as_ref(),
        value
    );
}
