//! Readers stay on a complete generation while publication blocks writers.
use ckb_db::{CollectionOptions, DBIterator, IteratorMode, RocksDB};
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMNS};
use std::{
    cell::Cell,
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[test]
fn reads_progress_during_publication_and_survive_retirement_or_uncertain_sync() {
    let _scenario = fail::FailScenario::setup();
    for (point, uncertain) in [
        ("freezer-gc-before-publish", false),
        ("freezer-gc-after-publish-sync", false),
        ("freezer-gc-before-publish", true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let db = RocksDB::open_in(directory.path(), COLUMNS);
        let txn = db.transaction();
        txn.put(COLUMN_BLOCK_BODY, b"cold", b"archived").unwrap();
        txn.put(COLUMN_BLOCK_BODY, b"hot", b"retained").unwrap();
        txn.commit().unwrap();
        drop(txn);
        let (reached, checkpoint) = mpsc::sync_channel(0);
        let (release, resume) = mpsc::sync_channel(0);
        let resume = Mutex::new(resume);
        fail::cfg_callback(point, move || {
            reached.send(()).unwrap();
            resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        })
        .unwrap();
        if uncertain {
            fail::cfg("freezer-gc-after-publish-sync", "return").unwrap();
        }

        thread::scope(|scope| {
            let db = &db;
            let collector = scope.spawn(|| {
                db.collect_payload(
                    &CollectionOptions::default(),
                    |_, _, key, _| Ok(key != b"cold"),
                    || {},
                )
            });
            checkpoint.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(db.collection_status().writers_paused);

            let (read_done, read_result) = mpsc::sync_channel(0);
            let (finish_read, read_finished) = mpsc::sync_channel(0);
            let reader = scope.spawn(move || {
                let snapshot = db.get_snapshot();
                let pin = db.get_pinned(COLUMN_BLOCK_BODY, b"cold").unwrap().unwrap();
                let mut iter = snapshot
                    .iter(COLUMN_BLOCK_BODY, IteratorMode::Start)
                    .unwrap();
                assert_eq!(pin.as_ref(), b"archived");
                read_done.send(()).unwrap();
                read_finished.recv_timeout(Duration::from_secs(10)).unwrap();
                // These were captured in the publication window, before the swap.
                assert_eq!(pin.as_ref(), b"archived");
                assert_eq!(
                    snapshot
                        .get_pinned(COLUMN_BLOCK_BODY, b"cold")
                        .unwrap()
                        .unwrap()
                        .as_ref(),
                    b"archived"
                );
                assert_eq!(iter.by_ref().count(), 2);
                iter.status().unwrap();
            });
            read_result
                .recv_timeout(Duration::from_secs(2))
                .expect("WAL sync must not block readers");

            let (attempted, attempt) = mpsc::sync_channel(0);
            let (written, write_result) = mpsc::channel();
            scope.spawn(move || {
                attempted.send(()).unwrap();
                let mut batch = db.new_write_batch();
                let result = batch
                    .put(COLUMN_BLOCK_BODY, b"later", b"foreground")
                    .and_then(|()| db.write_sync(&batch));
                written.send(result).unwrap();
            });
            attempt.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(matches!(
                write_result.recv_timeout(Duration::from_millis(50)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            release.send(()).unwrap();
            let collection = collector.join().unwrap();
            let write = write_result.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(collection.is_err(), uncertain);
            assert_eq!(write.is_err(), uncertain);
            assert_eq!(
                db.get_pinned(COLUMN_BLOCK_BODY, b"cold").unwrap().is_some(),
                uncertain
            );
            finish_read.send(()).unwrap();
            reader.join().unwrap();
        });
        fail::remove(point);
        fail::remove("freezer-gc-after-publish-sync");
        drop(db);
        // The publication reached durable storage even if its result was uncertain.
        let reopened = RocksDB::open_in(directory.path(), COLUMNS);
        assert!(
            reopened
                .get_pinned(COLUMN_BLOCK_BODY, b"cold")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            reopened
                .get_pinned(COLUMN_BLOCK_BODY, b"hot")
                .unwrap()
                .unwrap()
                .as_ref(),
            b"retained"
        );
        let txn = reopened.transaction();
        txn.put(COLUMN_BLOCK_BODY, b"after-reopen", b"writable")
            .unwrap();
        txn.commit().unwrap();
    }
}

const PAYLOAD_COLUMNS: [&str; 5] = ["2", "3", "7", "13", "15"];

#[test]
fn a_failed_retirement_checkpoint_requires_reopening() {
    let _scenario = fail::FailScenario::setup();
    let directory = tempfile::tempdir().unwrap();
    let db = RocksDB::open_in(directory.path(), COLUMNS);
    let txn = db.transaction();
    txn.put(COLUMN_BLOCK_BODY, b"hot", b"retained").unwrap();
    txn.commit().unwrap();
    drop(txn);
    fail::cfg("freezer-gc-retirement-checkpoint", "return").unwrap();
    let error = db
        .collect_payload(&CollectionOptions::default(), |_, _, _, _| Ok(true), || {})
        .unwrap_err();
    assert!(error.to_string().contains("checkpoint"), "{error}");
    assert!(
        db.transaction()
            .put(COLUMN_BLOCK_BODY, b"later", b"blocked")
            .is_err()
    );
    fail::remove("freezer-gc-retirement-checkpoint");
    drop(db);

    let db = RocksDB::open_in(directory.path(), COLUMNS);
    assert_eq!(
        db.get_pinned(COLUMN_BLOCK_BODY, b"hot")
            .unwrap()
            .unwrap()
            .as_ref(),
        b"retained"
    );
    let txn = db.transaction();
    txn.put(COLUMN_BLOCK_BODY, b"later", b"writable").unwrap();
    txn.commit().unwrap();
    drop(txn);
    assert_eq!(
        db.collect_payload(&CollectionOptions::default(), |_, _, _, _| Ok(true), || {})
            .unwrap()
            .generation,
        2
    );
}

fn verify_retained_values(db: &RocksDB) {
    for col in PAYLOAD_COLUMNS {
        for number in 0..8u8 {
            assert_eq!(
                db.get_pinned(col, &[number]).unwrap().unwrap().as_ref(),
                vec![number; 1024]
            );
        }
    }
}

#[test]
#[ignore = "subprocess helper for aborted_copy_cleanup_survives_process_kill"]
fn aborted_copy_child() {
    use ckb_db::internal::{DB, Options};

    let path = std::env::var_os("CKB_COLLECT_CRASH_PATH").unwrap();
    let marker = std::env::var_os("CKB_COLLECT_CRASH_MARKER").unwrap();
    let db = RocksDB::open_in(Path::new(&path), COLUMNS);
    let mut batch = db.new_write_batch();
    for col in PAYLOAD_COLUMNS {
        for number in 0..8u8 {
            batch.put(col, &[number], &vec![number; 1024]).unwrap();
        }
    }
    db.write_sync(&batch).unwrap();
    drop(batch);

    let cancelled = Cell::new(false);
    let mut copied = 0;
    let options = CollectionOptions {
        batch_bytes: 1,
        ..Default::default()
    };
    let error = db
        .collect_payload_with_cancel(
            &options,
            |_, _, _, _| {
                copied += 1;
                cancelled.set(copied == 4);
                Ok(true)
            },
            || cancelled.get(),
            || panic!("cancelled collection must not publish"),
        )
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert_eq!(copied, 4);
    assert!(
        DB::list_cf(&Options::default(), &path)
            .unwrap()
            .iter()
            .all(|name| !name.starts_with("freezer."))
    );
    verify_retained_values(&db);
    fs::write(marker, b"cleaned").unwrap();
    // The parent kills the process while the DB is open, without destructors.
    loop {
        thread::park();
    }
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn aborted_copy_cleanup_survives_process_kill() {
    let _scenario = fail::FailScenario::setup();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("db");
    let marker = directory.path().join("cleaned");
    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "aborted_copy_child", "--ignored", "--nocapture"])
            .env("CKB_COLLECT_CRASH_PATH", &path)
            .env("CKB_COLLECT_CRASH_MARKER", &marker)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() {
        assert!(child.0.try_wait().unwrap().is_none(), "child exited early");
        assert!(Instant::now() < deadline, "target CF cleanup timed out");
        thread::sleep(Duration::from_millis(2));
    }
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());

    let db = RocksDB::open_in(&path, COLUMNS);
    verify_retained_values(&db);
    let stats = db
        .collect_payload(&CollectionOptions::default(), |_, _, _, _| Ok(true), || {})
        .unwrap();
    assert_eq!(stats.generation, 1);
    verify_retained_values(&db);
    drop(db);
    verify_retained_values(&RocksDB::open_in(&path, COLUMNS));
}
