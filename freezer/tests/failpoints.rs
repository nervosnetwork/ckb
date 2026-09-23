//! Recover committed prefixes across interrupted archive writes and cleanup.
use ckb_freezer::FreezerFilesBuilder;
use fail::FailScenario;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const WRITE_POINTS: &[&str] = &[
    "freezer-before-seal-sync",
    "freezer-after-seal-sync",
    "freezer-before-create-file",
    "freezer-after-create-file",
    "freezer-before-data-write",
    "freezer-after-data-write",
    "freezer-before-index-write",
    "freezer-after-index-write",
    "freezer-before-head-sync",
    "freezer-after-head-sync",
    "freezer-before-index-sync",
    "freezer-after-index-sync",
    "freezer-before-directory-sync",
    "freezer-after-data-directory-sync",
    "freezer-before-commit-write",
    "freezer-after-commit-write",
    "freezer-before-commit-sync",
    "freezer-after-commit-sync",
    "freezer-before-commit-rename",
    "freezer-after-commit-rename",
    "freezer-before-commit-directory-sync",
    "freezer-after-commit-directory-sync",
];
const RECOVERY_POINTS: &[&str] = &[
    "freezer-after-recovery-index-truncate",
    "freezer-after-recovery-head-truncate",
    "freezer-after-recovery-remove-files",
    "freezer-after-recovery-sync",
];

fn builder(path: &Path) -> FreezerFilesBuilder {
    FreezerFilesBuilder::new(path.to_path_buf())
        .max_file_size(300)
        .open_files_limit(2)
}

fn data(number: u64) -> Vec<u8> {
    let mut state = number + 1;
    (0..if number.is_multiple_of(2) {
        40 * 1024
    } else {
        256
    })
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn prepare(path: &Path, pending: bool) {
    let mut files = builder(path).build().unwrap();
    for number in 1..4 {
        files.append(number, &data(number)).unwrap();
    }
    files.sync_all().unwrap();
    if pending {
        for number in 4..9 {
            files.append(number, &data(number)).unwrap();
        }
    }
}

#[test]
fn size_metric_tracks_only_committed_data_and_recovers_after_reopen() {
    let _scenario = FailScenario::setup();
    ckb_metrics::METRICS_SERVICE_ENABLED.get_or_init(|| true);
    let size = &ckb_metrics::handle().unwrap().ckb_freezer_size;
    let directory = tempfile::tempdir().unwrap();
    let mut files = builder(directory.path()).build().unwrap();
    assert_eq!(size.get(), 0);

    for number in 1..4 {
        files.append(number, &data(number)).unwrap();
    }
    assert_eq!(size.get(), 0, "pending appends are not committed bytes");
    files.sync_all().unwrap();
    let committed_size = size.get();
    // Small segments force rotation; count all segment bytes and the index.
    let disk_size: u64 = fs::read_dir(directory.path())
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_str().unwrap();
            name == "INDEX" || name.starts_with("blk")
        })
        .map(|entry| entry.metadata().unwrap().len())
        .sum();
    assert_eq!(committed_size as u64, disk_size);
    assert!(committed_size > 0);

    files.append(4, &data(4)).unwrap();
    fail::cfg("freezer-before-commit-write", "return").unwrap();
    assert!(files.sync_all().is_err());
    fail::remove("freezer-before-commit-write");
    assert_eq!(size.get(), committed_size);
    drop(files);

    size.set(-1);
    let mut files = builder(directory.path()).build().unwrap();
    assert_eq!(files.number(), 4);
    assert_eq!(size.get(), committed_size);
    files.append(4, &data(4)).unwrap();
    files.sync_all().unwrap();
    let final_size = size.get();
    assert!(final_size > committed_size);
    files.sync_all().unwrap();
    assert_eq!(size.get(), final_size, "sync without appends is idempotent");
    drop(files);
    let _files = builder(directory.path()).build().unwrap();
    assert_eq!(size.get(), final_size);
}

#[test]
fn io_errors_stop_writes_and_preserve_the_committed_prefix() {
    let scenario = FailScenario::setup();
    let errors = WRITE_POINTS
        .iter()
        .copied()
        .filter(|name| name.contains("-before-"))
        .chain([
            "freezer-partial-data-write",
            "freezer-partial-index-write",
            "freezer-partial-commit-write",
        ]);
    for point in errors {
        let directory = tempfile::tempdir().unwrap();
        prepare(directory.path(), false);
        let mut files = builder(directory.path()).build().unwrap();
        fail::cfg(point, "return").unwrap();
        let result = (|| {
            for number in 4..9 {
                files.append(number, &data(number))?;
            }
            files.sync_all()
        })();
        let error = result.expect_err(point);
        assert!(
            error.to_string().contains("injected"),
            "{point} was not reached: {error}"
        );
        assert_eq!(files.committed_number(), 4, "{point}");
        for number in 1..4 {
            assert_eq!(
                files.retrieve(number).unwrap(),
                Some(data(number)),
                "{point}"
            );
        }
        fail::remove(point);
        assert!(
            files
                .append(files.number(), b"no retry")
                .unwrap_err()
                .to_string()
                .contains("reopen")
        );
        assert!(files.sync_all().is_err());
        drop(files);
        let mut files = builder(directory.path()).build().unwrap();
        let recovered = if point == "freezer-before-commit-directory-sync" {
            9
        } else {
            4
        };
        assert_eq!(files.number(), recovered, "{point}");
        for number in files.number()..12 {
            files.append(number, &data(number)).unwrap();
        }
        files.sync_all().unwrap();
        files.verify().unwrap();
    }
    scenario.teardown();
}

#[test]
fn controller_reports_commit_failure_and_failed_shutdown() {
    use ckb_freezer::{Freezer, FreezerController, FreezerServiceConfig};
    use ckb_types::{
        core::{BlockBuilder, EpochNumberWithFraction},
        prelude::*,
    };
    let _scenario = FailScenario::setup();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let block = |number| {
        BlockBuilder::default()
            .number(number)
            .epoch(EpochNumberWithFraction::new(1, 0, 100))
            .build()
            .data()
    };
    for point in [
        "freezer-partial-data-write",
        "freezer-before-head-sync",
        "freezer-before-commit-directory-sync",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let controller = FreezerController::start(
            Freezer::open_in(directory.path()).unwrap(),
            runtime.handle().clone(),
            FreezerServiceConfig::default(),
        )
        .unwrap();
        runtime.block_on(async {
            controller.append(vec![block(10)]).await.unwrap();
            fail::cfg(point, "return").unwrap();
            let error = controller.append(vec![block(20)]).await.unwrap_err();
            assert!(error.to_string().contains("injected"), "{point}: {error}");
            fail::remove(point);
            assert_eq!(controller.number(), 2);
            assert_eq!(
                controller.retrieve(1).unwrap(),
                Some(block(10).as_slice().to_vec())
            );
            assert!(controller.append(vec![block(30)]).await.is_err());
            assert!(controller.shutdown().await.is_err());
        });
        drop(controller);
        let reopened = Freezer::open_in(directory.path()).unwrap();
        let expected = if point == "freezer-before-commit-directory-sync" {
            3
        } else {
            2
        };
        assert_eq!(reopened.number(), expected, "{point}");
        if expected == 3 {
            assert_eq!(
                reopened.retrieve(2).unwrap(),
                Some(block(20).as_slice().to_vec())
            );
        }
    }
}

#[test]
fn cancelled_started_read_is_drained_before_shutdown_returns() {
    use ckb_freezer::{Freezer, FreezerController, FreezerServiceConfig};
    use ckb_types::core::{BlockBuilder, EpochNumberWithFraction};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let _scenario = FailScenario::setup();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let controller = FreezerController::start(
        Freezer::open_in(directory.path()).unwrap(),
        runtime.handle().clone(),
        FreezerServiceConfig::default(),
    )
    .unwrap();
    runtime
        .block_on(controller.append(vec![BlockBuilder::default()
        .epoch(EpochNumberWithFraction::new(1, 0, 100)).build().data()]))
        .unwrap();
    let (release, released) = mpsc::channel();
    let started = Arc::new(AtomicBool::new(false));
    fail::cfg_callback("freezer-before-read-data", {
        let started = Arc::clone(&started);
        let released = Mutex::new(released);
        move || {
            started.store(true, Ordering::Release);
            let _ = released.lock().unwrap().recv();
        }
    })
    .unwrap();
    runtime.block_on(async {
        let reader = controller.clone();
        let read = tokio::spawn(async move { reader.retrieve_many(vec![1], 1024).await });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !started.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "read failpoint not reached");
            tokio::task::yield_now().await;
        }
        read.abort();
        assert!(read.await.unwrap_err().is_cancelled());
        // Poll shutdown once: it closes admission, then must wait for this read.
        let mut stopped = std::pin::pin!(controller.shutdown());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(stopped.as_mut(), &mut context).is_pending());
        assert!(controller.retrieve_many(vec![0], 1024).await.is_err());
        release.send(()).unwrap();
        stopped.await.unwrap();
    });
    fail::remove("freezer-before-read-data");
}

// This test function is a child-process entry point, never a standalone test.
#[test]
#[ignore]
fn crash_child() {
    let path = std::env::var_os("CKB_FREEZER_CRASH_PATH").expect("child archive path");
    let marker = std::env::var_os("CKB_FREEZER_CRASH_MARKER").expect("child marker");
    let point = std::env::var("CKB_FREEZER_CRASH_POINT").expect("child failpoint");
    let mode = std::env::var("CKB_FREEZER_CRASH_MODE").expect("child mode");
    let scenario = FailScenario::setup();
    let install = || {
        fail::cfg_callback(&point, {
            let marker = marker.clone();
            move || {
                let temporary = Path::new(&marker).with_extension("tmp");
                fs::write(&temporary, b"hit").unwrap();
                fs::rename(temporary, &marker).unwrap();
                // Parent sends an actual process kill. No Rust destructor unwinds.
                loop {
                    thread::park();
                }
            }
        })
        .unwrap()
    };
    if mode != "append" {
        install();
    }
    let mut files = builder(Path::new(&path)).build().unwrap();
    if mode == "append" {
        install();
        for number in 4..9 {
            files.append(number, &data(number)).unwrap();
        }
        files.sync_all().unwrap();
    }
    scenario.teardown();
    panic!("configured crash point was not reached: {point}");
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn kill_at(path: &Path, marker: &Path, point: &str, mode: &str) {
    let mut child = KillOnDrop(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--ignored", "--nocapture"])
            .env("CKB_FREEZER_CRASH_PATH", path)
            .env("CKB_FREEZER_CRASH_MARKER", marker)
            .env("CKB_FREEZER_CRASH_POINT", point)
            .env("CKB_FREEZER_CRASH_MODE", mode)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("{point}: child exited without hitting failpoint: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "{point}: timed out waiting for failpoint"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(fs::read(marker).unwrap(), b"hit", "{point}");
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
}

#[test]
fn process_kill_at_every_write_and_commit_boundary() {
    let _scenario = FailScenario::setup();
    for &point in WRITE_POINTS {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        prepare(&archive, false);
        kill_at(&archive, &directory.path().join("hit"), point, "append");
        let mut files = builder(&archive).build().unwrap();
        let recovered = files.number();
        assert!(
            recovered == 4 || recovered == 9,
            "{point}: non-atomic commit {recovered}"
        );
        for number in 1..recovered {
            assert_eq!(
                files.retrieve(number).unwrap(),
                Some(data(number)),
                "{point}, item {number}"
            );
        }
        for number in recovered..14 {
            files.append(number, &data(number)).unwrap();
        }
        files.sync_all().unwrap();
        drop(files);
        let files = builder(&archive).build().unwrap();
        assert_eq!(files.number(), 14);
        for number in 1..14 {
            assert_eq!(files.retrieve(number).unwrap(), Some(data(number)));
        }
    }
}

#[test]
fn recovery_itself_can_be_killed_and_repeated() {
    let _scenario = FailScenario::setup();
    for &point in RECOVERY_POINTS {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        prepare(&archive, true);
        for round in 0..3 {
            kill_at(
                &archive,
                &directory.path().join(format!("hit-{round}")),
                point,
                "recover",
            );
        }
        let files = builder(&archive).build().unwrap();
        assert_eq!(files.number(), 4, "{point}");
        for number in 1..4 {
            assert_eq!(files.retrieve(number).unwrap(), Some(data(number)));
        }
    }
}

#[test]
fn interrupted_initialization_can_be_reopened() {
    let _scenario = FailScenario::setup();
    for &point in WRITE_POINTS.iter().filter(|name| name.contains("commit-")) {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        // Create the parent first so the injected point concerns the commit.
        fs::create_dir(&archive).unwrap();
        kill_at(&archive, &directory.path().join("hit"), point, "initialize");
        let files = builder(&archive).build().unwrap();
        assert_eq!(files.number(), 1, "{point}");
    }
}
