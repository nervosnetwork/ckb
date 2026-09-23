//! Kill real processes across file -> index -> CF publication and recovery.
use ckb_db::{CollectionOptions, RocksDB};
use ckb_db_schema::*;
use ckb_freezer::{Freezer, FreezerController, FreezerServiceConfig};
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::{BlockBuilder, BlockExt, BlockView, EpochNumberWithFraction},
    packed,
    prelude::*,
};
use fail::FailScenario;
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const GENERATION: &[u8] = b"freezer/cf-generation";
const BLOCKS: u64 = 24;
const ARCHIVED: u64 = 16;

fn block(number: u64) -> BlockView {
    let txs = (0..3u32).map(|index| {
        packed::Transaction::new_builder()
            .raw(
                packed::RawTransaction::new_builder()
                    .version(number as u32 * 10 + index)
                    .build(),
            )
            .witnesses(vec![packed::Bytes::from(vec![
                index as u8;
                if number.is_multiple_of(2) {
                    16 * 1024
                } else {
                    0
                }
            ])])
            .build()
            .into_view()
    });
    BlockBuilder::default()
        .number(number)
        .epoch(EpochNumberWithFraction::new(1, 0, 100))
        .transactions(txs)
        .extension(Some(vec![number as u8; 192].into()))
        .build()
}

fn open(path: &Path) -> ChainDB {
    ChainDB::new_with_freezer(
        RocksDB::open_in(path.join("db"), COLUMNS),
        Freezer::open_in(path.join("archive")).unwrap(),
        Default::default(),
    )
}

fn prepare(path: &Path) {
    let store = open(path);
    let txn = store.begin_transaction();
    for number in 1..=BLOCKS {
        let block = block(number);
        txn.insert_block(&block).unwrap();
        txn.attach_block(&block).unwrap();
        txn.insert_block_ext(
            &block.hash(),
            &BlockExt {
                received_at: 0,
                total_difficulty: Default::default(),
                total_uncles_count: 0,
                verified: Some(number != ARCHIVED),
                txs_fees: vec![],
                cycles: None,
                txs_sizes: None,
            },
        )
        .unwrap();
    }
    txn.commit().unwrap();
    drop(txn);
    let mut batch = store.new_write_batch();
    batch
        .put(COLUMN_CELL, b"live-cell", b"state is preserved")
        .unwrap();
    // Unknown and unverified payloads are never eligible for archive collection.
    batch
        .put(COLUMN_BLOCK_BODY, b"unknown-key", b"unknown value")
        .unwrap();
    store.write_sync(&batch).unwrap();
}

fn append(store: &ChainDB) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let controller = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        FreezerServiceConfig::default(),
    )
    .unwrap();
    runtime.block_on(async {
        controller
            .append((1..=ARCHIVED).map(|number| block(number).data()).collect())
            .await
            .unwrap();
        controller.shutdown().await.unwrap();
    });
}

fn collect(store: &ChainDB) {
    store
        .collect_archive(
            &CollectionOptions {
                batch_bytes: 256,
                writer_wait: Duration::from_secs(2),
                reconcile_timeout: Duration::from_secs(2),
                ..Default::default()
            },
            || {},
        )
        .unwrap();
}

#[test]
fn slow_archive_sync_keeps_committed_cold_and_hot_reads_available() {
    use std::sync::{Mutex, mpsc};
    let _scenario = FailScenario::setup();
    let directory = tempfile::tempdir().unwrap();
    prepare(directory.path());
    let store = Arc::new(open(directory.path()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    let controller = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        Default::default(),
    )
    .unwrap();
    runtime
        .block_on(controller.append(vec![block(1).data()]))
        .unwrap();
    store.recover_archive().unwrap();
    collect(&store);
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    fail::cfg_callback("freezer-before-commit-sync", move || {
        entered.send(()).unwrap();
        // A timeout makes a test failure terminate instead of hanging the suite.
        released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
    })
    .unwrap();
    let append = {
        let controller = controller.clone();
        runtime.spawn(async move { controller.append(vec![block(2).data()]).await })
    };
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(controller.number(), 2);
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block(2).hash().as_slice())
            .is_none()
    );
    let (finished, reads) = mpsc::channel();
    let reader = {
        let store = Arc::clone(&store);
        thread::spawn(move || {
            for _ in 0..100 {
                for number in [1, 2, BLOCKS] {
                    let expected = block(number);
                    assert_eq!(store.get_block(&expected.hash()).unwrap(), expected);
                    for tx in expected.transactions() {
                        assert_eq!(store.get_transaction(&tx.hash()).unwrap().0, tx);
                    }
                }
            }
            finished.send(()).unwrap();
        })
    };
    let completed_while_sync_waited = reads.recv_timeout(Duration::from_secs(5));
    release.send(()).unwrap();
    runtime.block_on(append).unwrap().unwrap();
    fail::remove("freezer-before-commit-sync");
    reader.join().unwrap();
    completed_while_sync_waited.expect("archive sync must not lock out reads");
    assert_eq!(store.recover_archive().unwrap(), 3);
    collect(&store);
    runtime.block_on(controller.shutdown()).unwrap();
    drop((controller, store));
    let reopened = open(directory.path());
    for number in 1..=BLOCKS {
        let expected = block(number);
        assert_eq!(reopened.get_block(&expected.hash()).unwrap(), expected);
    }
}

#[test]
fn cold_edits_after_snapshot_copy_are_restored_before_generation_publication() {
    use std::sync::{Mutex, mpsc};
    let _scenario = FailScenario::setup();
    let directory = tempfile::tempdir().unwrap();
    prepare(directory.path());
    let store = Arc::new(open(directory.path()));
    append(&store);
    store.recover_archive().unwrap();
    let expected = block(2);
    let hash = expected.hash();
    let before = store.get_snapshot();
    let (copied, copy_done) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    fail::cfg_callback("freezer-gc-after-copy", move || {
        copied.send(()).unwrap();
        released.lock().unwrap().recv().unwrap();
    })
    .unwrap();
    let collector = {
        let store = Arc::clone(&store);
        thread::spawn(move || store.collect_archive(&CollectionOptions::default(), || {}))
    };
    copy_done.recv_timeout(Duration::from_secs(5)).unwrap();
    let transaction = store.begin_transaction();
    transaction
        .delete(COLUMN_BLOCK_EXTENSION, hash.as_slice())
        .unwrap();
    let key = packed::TransactionKey::new_builder()
        .block_hash(hash.clone())
        .index(0usize)
        .build();
    transaction
        .delete(COLUMN_BLOCK_BODY, key.as_slice())
        .unwrap();
    transaction.commit().unwrap();
    drop(transaction);
    release.send(()).unwrap();
    let stats = collector.join().unwrap().unwrap();
    fail::remove("freezer-gc-after-copy");
    assert!(stats.reconciled > 0);
    assert_eq!(before.get_block_body(&hash), expected.transactions());
    assert_eq!(before.get_block_extension(&hash), expected.extension());
    assert_eq!(store.get_block_body(&hash), expected.transactions()[1..]);
    assert!(store.get_cellbase(&hash).is_none());
    assert!(store.get_block_extension(&hash).is_none());
    assert!(
        store
            .get_transaction(&expected.transactions()[0].hash())
            .is_none()
    );
    assert_eq!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, hash.as_slice())
            .unwrap()
            .as_ref(),
        0u64.to_le_bytes()
    );
    drop((before, store));
    let store = open(directory.path());
    store.recover_archive().unwrap();
    assert_eq!(store.get_block_body(&hash), expected.transactions()[1..]);
    assert!(store.get_cellbase(&hash).is_none());
    assert!(store.get_block_extension(&hash).is_none());
}

fn generation(store: &ChainDB) -> u64 {
    store
        .db()
        .get_pinned_default(GENERATION)
        .unwrap()
        .map(|raw| u64::from_le_bytes(raw.as_ref().try_into().unwrap()))
        .unwrap_or(0)
}

fn verify(store: &ChainDB) {
    let snapshot = store.get_snapshot();
    for number in 1..=BLOCKS {
        let expected = block(number);
        let hash = expected.hash();
        assert_eq!(snapshot.get_block(&hash).unwrap(), expected);
        assert_eq!(snapshot.get_packed_block(&hash).unwrap(), expected.data());
        assert_eq!(snapshot.get_block_body(&hash), expected.transactions());
        assert_eq!(snapshot.get_block_txs_hashes(&hash), expected.tx_hashes());
        assert_eq!(snapshot.get_block_extension(&hash), expected.extension());
        assert_eq!(
            snapshot.get_cellbase(&hash),
            expected.transactions().first().cloned()
        );
        for transaction in expected.transactions() {
            let (read, info) = snapshot
                .get_transaction_with_info(&transaction.hash())
                .unwrap();
            assert_eq!(read, transaction);
            assert_eq!(info.block_hash, hash);
        }
    }
    assert_eq!(
        store.get(COLUMN_CELL, b"live-cell").unwrap().as_ref(),
        b"state is preserved"
    );
    assert_eq!(
        store
            .get(COLUMN_BLOCK_BODY, b"unknown-key")
            .unwrap()
            .as_ref(),
        b"unknown value"
    );
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block(ARCHIVED).hash().as_slice())
            .is_none()
    );
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn kill_at(path: &Path, point: &str, hit: usize, recover: bool, label: &str) {
    let marker = path.join(format!("hit-{label}"));
    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "archive_crash_child", "--ignored", "--nocapture"])
            .env("CKB_ARCHIVE_CRASH_PATH", path)
            .env("CKB_ARCHIVE_CRASH_MARKER", &marker)
            .env("CKB_ARCHIVE_CRASH_POINT", point)
            .env("CKB_ARCHIVE_CRASH_HIT", hit.to_string())
            .env("CKB_ARCHIVE_CRASH_RECOVER", if recover { "1" } else { "0" })
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("{point}/{hit} exited before hit: {status}");
        }
        assert!(Instant::now() < deadline, "{point}/{hit} did not run");
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fs::read(&marker).unwrap(), b"hit");
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
}

#[test]
#[ignore = "subprocess helper, run only by the crash harness"]
fn archive_crash_child() {
    let _scenario = FailScenario::setup();
    let path = std::env::var_os("CKB_ARCHIVE_CRASH_PATH").unwrap();
    let point = std::env::var("CKB_ARCHIVE_CRASH_POINT").unwrap();
    let marker = std::path::PathBuf::from(std::env::var_os("CKB_ARCHIVE_CRASH_MARKER").unwrap());
    let hit: usize = std::env::var("CKB_ARCHIVE_CRASH_HIT")
        .unwrap()
        .parse()
        .unwrap();
    let recover = std::env::var("CKB_ARCHIVE_CRASH_RECOVER").unwrap() == "1";
    let configure = || {
        let seen = Arc::new(AtomicUsize::new(0));
        let marker = marker.clone();
        fail::cfg_callback(&point, move || {
            if seen.fetch_add(1, Ordering::Relaxed) + 1 == hit {
                let temporary = marker.with_extension("pending");
                fs::write(&temporary, b"hit").unwrap();
                fs::rename(temporary, &marker).unwrap();
                loop {
                    thread::park();
                }
            }
        })
        .unwrap();
    };
    if recover {
        configure();
    }
    let store = open(Path::new(&path));
    if !recover {
        configure();
        append(&store);
    }
    store.recover_archive().unwrap();
    collect(&store);
    panic!("configured point was not reached");
}

#[test]
fn kill_across_files_index_copy_publication_and_partial_retirement() {
    let _scenario = FailScenario::setup();
    let cases = [
        ("freezer-before-head-sync", 1, false),
        ("freezer-after-commit-sync", 1, false),
        ("freezer-after-commit-rename", 1, false),
        ("freezer-after-commit-directory-sync", 1, false),
        ("freezer-index-before-publish", 1, false),
        ("freezer-index-after-publish", 1, false),
        ("freezer-gc-after-create", 1, false),
        ("freezer-gc-after-create", 2, false),
        ("freezer-gc-after-copy-batch", 1, false),
        ("freezer-gc-after-copy-batch", 3, false),
        ("freezer-gc-after-copy", 1, false),
        ("freezer-gc-after-prepare-sync", 1, false),
        ("freezer-gc-after-reconcile", 1, false),
        ("freezer-gc-before-publish", 1, false),
        ("freezer-gc-after-publish-sync", 1, true),
        ("freezer-gc-after-route", 1, true),
        ("freezer-gc-before-drop", 1, true),
        ("freezer-gc-before-drop", 3, true),
        ("freezer-gc-before-drop", 5, true),
        ("freezer-gc-after-drop", 1, true),
        ("freezer-gc-after-drop", 3, true),
        ("freezer-gc-after-drop", 5, true),
    ];
    for (point, hit, published) in cases {
        let directory = tempfile::tempdir().unwrap();
        prepare(directory.path());
        kill_at(directory.path(), point, hit, false, "initial");
        let store = open(directory.path());
        assert_eq!(generation(&store), u64::from(published), "{point}/{hit}");
        verify(&store);
        let number = store.freezer().unwrap().number();
        assert!(number == 1 || number == ARCHIVED + 1);
        if number == 1 {
            append(&store);
        }
        assert_eq!(store.recover_archive().unwrap(), ARCHIVED + 1);
        verify(&store);
        collect(&store);
        verify(&store);
        drop(store);
        verify(&open(directory.path()));
        println!("kill {point}/{hit}: recovered, recollected, and reopened all {BLOCKS} blocks");
    }
}

#[test]
fn recovery_can_be_killed_repeatedly_for_unpublished_and_published_generations() {
    let _scenario = FailScenario::setup();
    for (seed, hit, published) in [
        ("freezer-gc-after-create", 2, false),
        ("freezer-gc-after-publish-sync", 1, true),
    ] {
        for point in [
            "freezer-gc-recovery-before-drop",
            "freezer-gc-recovery-after-drop",
        ] {
            let directory = tempfile::tempdir().unwrap();
            prepare(directory.path());
            kill_at(directory.path(), seed, hit, false, "seed");
            for round in 0..2 {
                kill_at(
                    directory.path(),
                    point,
                    1,
                    true,
                    &format!("recovery-{round}"),
                );
            }
            let store = open(directory.path());
            assert_eq!(generation(&store), u64::from(published));
            assert_eq!(store.recover_archive().unwrap(), ARCHIVED + 1);
            verify(&store);
            collect(&store);
            verify(&store);
            println!("kill recovery {point}, published={published}: passed twice");
        }
    }
}

#[test]
fn native_operation_uncertainty_requires_reopen_and_preserves_both_read_paths() {
    let _scenario = FailScenario::setup();
    for point in [
        "freezer-gc-after-create",
        "freezer-gc-after-publish-sync",
        "freezer-gc-after-route",
        "freezer-gc-after-drop",
    ] {
        let directory = tempfile::tempdir().unwrap();
        prepare(directory.path());
        let store = open(directory.path());
        append(&store);
        store.recover_archive().unwrap();
        let old = store.get_snapshot();
        fail::cfg(point, "return").unwrap();
        let error = store
            .collect_archive(&CollectionOptions::default(), || {})
            .unwrap_err();
        assert!(error.to_string().contains("injected"), "{point}: {error}");
        fail::remove(point);
        assert!(store.db().check_writable().is_err());
        let txn = store.db().transaction();
        assert!(txn.put(COLUMN_CELL, b"must-reopen", b"no write").is_err());
        drop(txn);
        verify(&store);
        assert_eq!(old.get_block(&block(1).hash()).unwrap(), block(1));
        drop((old, store));
        let store = open(directory.path());
        store.db().check_writable().unwrap();
        verify(&store);
        collect(&store);
        verify(&store);
    }
}

#[test]
fn index_pause_deadline_and_cancellation_preserve_resumable_progress() {
    use std::sync::atomic::AtomicBool;
    let _scenario = FailScenario::setup();
    let directory = tempfile::tempdir().unwrap();
    prepare(directory.path());
    let store = open(directory.path());
    append(&store);
    fail::cfg("freezer-index-paused", "sleep(150)").unwrap();
    assert_eq!(store.recover_archive().unwrap(), 1);
    fail::remove("freezer-index-paused");
    store.db().check_writable().unwrap();
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block(1).hash().as_slice())
            .is_none()
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    fail::cfg_callback("freezer-index-after-record", {
        let cancelled = Arc::clone(&cancelled);
        move || {
            cancelled.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(
        store
            .recover_archive_with_cancel(|| cancelled.load(Ordering::Relaxed))
            .unwrap(),
        2
    );
    fail::remove("freezer-index-after-record");
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block(1).hash().as_slice())
            .is_some()
    );
    assert!(
        store
            .get(COLUMN_BLOCK_ARCHIVE, block(2).hash().as_slice())
            .is_none()
    );
    assert_eq!(store.recover_archive().unwrap(), ARCHIVED + 1);
    verify(&store);
    collect(&store);
    verify(&store);
}
