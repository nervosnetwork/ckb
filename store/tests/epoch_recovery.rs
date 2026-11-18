//! Stop a real child after a physical create or after durable unpublication;
//! the opener must follow the manifest, never the highest CF name on disk.
use ckb_db::{RocksDB, internal::DB};
use ckb_db_schema::{COLUMNS, freezer};
use ckb_store::{ChainDB, ChainStore, Freezer, FreezerController};
use ckb_types::{
    core::{BlockBuilder, BlockExt},
    prelude::*,
};
use std::{path::Path, process::Command};

fn open(root: &Path) -> ChainDB {
    let s = ChainDB::new_with_freezer(
        RocksDB::open_in(root.join("db"), COLUMNS),
        Freezer::open_in(root.join("archive")).unwrap(),
        Default::default(),
    );
    s.recover_archive().unwrap();
    s
}
fn old_block() -> ckb_types::core::BlockView {
    BlockBuilder::default()
        .number(1u64)
        .epoch(ckb_types::core::EpochNumberWithFraction::new(0, 1, 2))
        .build()
}
fn populate(store: &ChainDB) {
    let b = old_block();
    let tx = store.begin_transaction();
    tx.insert_block(&b).unwrap();
    tx.insert_block_ext(
        &b.hash(),
        &BlockExt {
            received_at: 0,
            total_difficulty: Default::default(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
            cycles: None,
            txs_sizes: None,
        },
    )
    .unwrap();
    tx.commit().unwrap();
}

#[test]
#[ignore = "subprocess helper for epoch recovery"]
fn recovery_child() {
    let root = std::env::var("CKB_EPOCH_CRASH_DIR").unwrap();
    let point = std::env::var("CKB_EPOCH_CRASH_POINT").unwrap();
    let store = open(Path::new(&root));
    let _scenario = fail::FailScenario::setup();
    fail::cfg_callback(&point, || std::process::exit(86)).unwrap();
    if point == "epoch-after-create" {
        store.db().open_epoch_segment(16).unwrap();
    } else {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let writer = FreezerController::start(
                store.freezer().unwrap().clone(),
                tokio::runtime::Handle::current(),
                Default::default(),
            )
            .unwrap();
            store
                .collect_epoch_segments(&writer, || 16, || false, || {})
                .await
                .unwrap();
        });
    }
    panic!("child did not reach {point}");
}

#[test]
fn restart_resolves_unpublished_create_and_published_retirement_without_losing_payloads() {
    for point in ["epoch-after-create", "epoch-after-retire-marker"] {
        let tmp = tempfile::tempdir().unwrap();
        let s = open(tmp.path());
        s.enable_epoch_segments().unwrap();
        populate(&s);
        if point == "epoch-after-retire-marker" {
            s.db().open_epoch_segment(16).unwrap();
        }
        drop(s);
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "recovery_child"])
            .env("CKB_EPOCH_CRASH_DIR", tmp.path())
            .env("CKB_EPOCH_CRASH_POINT", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(86));
        let names = || DB::list_cf(&Default::default(), tmp.path().join("db")).unwrap();
        assert!(
            names().contains(&freezer::segment_name(0))
                && names().contains(&freezer::segment_name(16))
        );
        let s = open(tmp.path());
        let b = old_block();
        assert_eq!(s.get_block(&b.hash()).unwrap().data(), b.data());
        let (live, unpublished) = if point == "epoch-after-create" {
            (0, 16)
        } else {
            (16, 0)
        };
        assert_eq!(s.db().epoch_segments().windows(), &[live]);
        assert!(!names().contains(&freezer::segment_name(unpublished)));
        assert_eq!(
            s.get_archived_block(&b.hash()).is_some(),
            point == "epoch-after-retire-marker"
        );
    }
}

#[cfg(unix)]
#[test]
#[ignore = "subprocess helper for rebuild interruptions"]
fn rebuild_child() {
    let root = std::path::PathBuf::from(std::env::var_os("CKB_REBUILD_SOURCE").unwrap());
    let point = std::env::var("CKB_REBUILD_POINT").unwrap();
    let _scenario = fail::FailScenario::setup();
    fail::cfg_callback(&point, || std::process::exit(86)).unwrap();
    let config = ckb_app_config::DBConfig {
        path: root.join("db"),
        ..Default::default()
    };
    ckb_store::rebuild::stopped_pair(&config, &root.join("archive"), &root.join(point)).unwrap();
}

#[cfg(unix)]
#[test]
fn incomplete_rebuilds_never_publish_either_half_and_retry_preserves_original() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let s = open(root);
    populate(&s);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let writer = FreezerController::start(
            s.freezer().unwrap().clone(),
            tokio::runtime::Handle::current(),
            Default::default(),
        )
        .unwrap();
        writer.append(vec![old_block().data()]).await.unwrap();
        assert_eq!(s.recover_archive().unwrap(), 2);
        writer.shutdown().await.unwrap();
    });
    drop(s);
    for point in [
        "freezer-rebuild-after-copy",
        "freezer-rebuild-before-publish",
    ] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "rebuild_child", "--ignored"])
            .env("CKB_REBUILD_SOURCE", root)
            .env("CKB_REBUILD_POINT", point)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(86));
        assert!(!root.join(point).join("ready").exists());
        if point == "freezer-rebuild-after-copy" {
            let config = ckb_app_config::DBConfig {
                path: root.join(point).join("building/db"),
                ..Default::default()
            };
            assert!(RocksDB::open_with_check(&config, COLUMNS).is_err());
        }
        assert_eq!(
            open(root).get_block(&old_block().hash()).unwrap().data(),
            old_block().data()
        );
    }
    let config = ckb_app_config::DBConfig {
        path: root.join("db"),
        ..Default::default()
    };
    let out = root.join("complete");
    let stats = ckb_store::rebuild::stopped_pair(&config, &root.join("archive"), &out).unwrap();
    assert_eq!(stats.omitted, 2);
    let pair = out.join("ready");
    let store = ChainDB::new_with_freezer(
        RocksDB::open_in(pair.join("db"), COLUMNS),
        Freezer::open_in(pair.join("ancient")).unwrap(),
        Default::default(),
    );
    assert_eq!(store.recover_archive().unwrap(), 2);
    assert_eq!(
        store.get_block(&old_block().hash()).unwrap().data(),
        old_block().data()
    );
    assert!(
        store
            .get(
                ckb_db_schema::COLUMN_BLOCK_UNCLE,
                old_block().hash().as_slice()
            )
            .is_none()
    );
    assert!(
        open(root)
            .get(
                ckb_db_schema::COLUMN_BLOCK_UNCLE,
                old_block().hash().as_slice()
            )
            .is_some()
    );
}
