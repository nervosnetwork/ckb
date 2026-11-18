use super::*;
use crate::SharedBuilder;
use ckb_app_config::{DBConfig, StoreConfig};
use ckb_types::core::{
    BlockBuilder, BlockExt, BlockView, EpochNumberWithFraction, TransactionBuilder,
};

// The metrics registry is global, so these archival scenarios must not overlap.
static METRICS_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn child(parent: &BlockView, nonce: u128) -> BlockView {
    BlockBuilder::default()
        .number(parent.number() + 1)
        .parent_hash(parent.hash())
        .nonce(nonce)
        .epoch(EpochNumberWithFraction::new(1, 0, 100))
        .build()
}

fn insert(shared: &Shared, blocks: &[BlockView], verified: bool) {
    let txn = shared.store().begin_transaction();
    for block in blocks {
        txn.insert_block(block).unwrap();
        txn.attach_block(block).unwrap();
        txn.insert_block_ext(
            &block.hash(),
            &BlockExt {
                received_at: 0,
                total_difficulty: Default::default(),
                total_uncles_count: 0,
                verified: Some(verified),
                txs_fees: vec![],
                cycles: None,
                txs_sizes: None,
            },
        )
        .unwrap();
    }
    txn.commit().unwrap();
    drop(txn);
    shared.refresh_snapshot();
}

#[test]
fn enabling_epoch_segments_preserves_legacy_payloads_and_publishes_exact_archive_routes() {
    exercise_legacy_activation(true);
}

#[test]
fn enabling_freezer_on_a_populated_legacy_database_starts_online_cf_rotation() {
    exercise_legacy_activation(false);
}

fn exercise_legacy_activation(segmented: bool) {
    let _metrics_test = METRICS_TEST.lock().unwrap();
    ckb_metrics::METRICS_SERVICE_ENABLED.get_or_init(|| true);
    let metrics = ckb_metrics::handle().unwrap();
    let collections = metrics
        .ckb_freezer_collection_total
        .with_label_values(&["success"])
        .get();
    use ckb_db::RocksDB;
    use ckb_db_schema::{
        COLUMN_BLOCK_BODY, COLUMN_META, COLUMNS, META_ARCHIVE_COLLECTED, META_ARCHIVE_NEXT_RECORD,
    };
    use ckb_migrate::migrate::Migrate;

    let directory = tempfile::tempdir().unwrap();
    let config = DBConfig {
        path: directory.path().join("db"),
        ..Default::default()
    };
    let ancient = directory.path().join("ancient");
    let consensus = Consensus::default();
    let genesis_length = consensus.genesis_epoch_ext().length();
    let epoch_length = 1024;
    let archive_limit = genesis_length + 2 * epoch_length - 1;
    let tip_number = archive_limit + 101;

    // Persist a verified chain in the previous physical layout, without an
    // archive column, activation cursor, or archive files.
    let legacy = ChainDB::new(
        RocksDB::open_in(&config.path, COLUMNS - 1),
        StoreConfig::default(),
    );
    legacy.init(&consensus).unwrap();
    Migrate::new(&config.path, consensus.hardfork_switch().clone())
        .init_db_version(legacy.db())
        .unwrap();
    let mut blocks = vec![consensus.genesis_block().clone()];
    let txn = legacy.begin_transaction();
    for number in 1..=tip_number {
        let epoch = if number < genesis_length {
            EpochNumberWithFraction::new(0, number, genesis_length)
        } else if number <= archive_limit {
            EpochNumberWithFraction::new(
                1 + (number - genesis_length) / epoch_length,
                (number - genesis_length) % epoch_length,
                epoch_length,
            )
        } else {
            EpochNumberWithFraction::new(number - archive_limit + 2, 0, 1)
        };
        let block = child(blocks.last().unwrap(), u128::from(number))
            .as_advanced_builder()
            .epoch(epoch)
            .transaction(TransactionBuilder::default().version(number as u32).build())
            .extension(Some(vec![number as u8; 16].into()))
            .build();
        txn.insert_block(&block).unwrap();
        txn.attach_block(&block).unwrap();
        txn.insert_block_ext(
            &block.hash(),
            &BlockExt {
                received_at: 0,
                total_difficulty: (number + 1).into(),
                total_uncles_count: 0,
                verified: Some(true),
                txs_fees: vec![],
                cycles: None,
                txs_sizes: None,
            },
        )
        .unwrap();
        blocks.push(block);
    }
    for number in 1..=103 {
        let (start, length) = if number <= 2 {
            (genesis_length + (number - 1) * epoch_length, epoch_length)
        } else {
            (archive_limit + number - 2, 1)
        };
        let previous = blocks[(start - 1) as usize].hash();
        let mut epoch = consensus.genesis_epoch_ext().clone();
        epoch.set_number(number);
        epoch.set_start_number(start);
        epoch.set_length(length);
        epoch.set_last_block_hash_in_previous_epoch(previous.clone());
        txn.insert_epoch_ext(&previous, &epoch).unwrap();
        for block in &blocks[start as usize..=(start + length - 1).min(tip_number) as usize] {
            txn.insert_block_epoch_index(&block.hash(), &previous)
                .unwrap();
        }
        if number == 103 {
            txn.insert_current_epoch_ext(&epoch).unwrap();
        }
    }
    txn.insert_tip_header(&blocks.last().unwrap().header())
        .unwrap();
    txn.commit().unwrap();
    drop(txn);
    let mut batch = legacy.new_write_batch();
    batch
        .put(COLUMN_BLOCK_BODY, b"unknown-key", b"retained")
        .unwrap();
    legacy.write_sync(&batch).unwrap();
    drop((batch, legacy));
    assert!(!ancient.exists());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let open = |enabled| {
        SharedBuilder::new(
            "test",
            directory.path(),
            &config,
            Some(ancient.clone()),
            Handle::new(runtime.handle().clone(), None),
            consensus.clone(),
        )
        .unwrap()
        .store_config(StoreConfig {
            freezer_enable: enabled,
            freezer_epoch_segments: enabled && segmented,
            ..Default::default()
        })
        .build()
    };
    let (shared, package) = open(true).unwrap();
    assert_eq!(shared.store().freezer().unwrap().number(), 1);
    assert!(
        shared
            .store()
            .db()
            .get_pinned_default(b"freezer/cf-generation")
            .unwrap()
            .is_none()
    );
    let before = shared.store().get_snapshot();
    let controller = FreezerController::start(
        shared.store().freezer().unwrap().clone(),
        runtime.handle().clone(),
        Default::default(),
    )
    .unwrap();
    // Even a mature history stays hot until initial block download finishes.
    shared.freeze(&controller, &mut Default::default()).unwrap();
    assert_eq!(controller.number(), 1);
    assert_eq!(metrics.ckb_freezer_state.get(), 1);
    assert_eq!(metrics.ckb_freezer_backlog.get(), 0);
    shared.ibd_finished.store(true, Ordering::Release);
    // Advance the chain view across the exact safety boundary. Epoch 0 first
    // becomes eligible in epoch 101; epoch 3 must still be hot in epoch 103.
    for (current_epoch, next_record) in [
        (0, 1),
        (99, 1),
        (100, 1),
        (101, genesis_length),
        (103, archive_limit + 1),
    ] {
        let epoch = shared
            .store()
            .get_epoch_index(current_epoch)
            .and_then(|index| shared.store().get_epoch_ext(&index))
            .unwrap();
        let tip = &blocks[(epoch.start_number() + epoch.length() - 1) as usize];
        let proposals = shared.snapshot().proposals().clone();
        shared.store_snapshot(shared.new_snapshot(
            tip.header(),
            (tip.number() + 1).into(),
            epoch,
            proposals,
        ));
        shared.freeze(&controller, &mut Default::default()).unwrap();
        assert_eq!(controller.number(), next_record, "epoch {current_epoch}");
    }
    assert_eq!(controller.number(), archive_limit + 1);
    assert_eq!(metrics.ckb_freezer_number.get(), archive_limit as i64);
    assert_eq!(metrics.ckb_freezer_backlog.get(), 0);
    assert!(metrics.ckb_freezer_last_progress_timestamp.get() > 0);
    assert_eq!(
        metrics
            .ckb_freezer_collection_total
            .with_label_values(&["success"])
            .get(),
        collections + u64::from(!segmented)
    );
    assert_eq!(
        metrics.ckb_freezer_retired_generations.get(),
        i64::from(!segmented)
    );
    assert_eq!(
        shared
            .store()
            .get(COLUMN_META, META_ARCHIVE_NEXT_RECORD)
            .unwrap()
            .as_ref(),
        (archive_limit + 1).to_le_bytes()
    );
    if segmented {
        // Upgrade is non-destructive: the unpartitioned population remains in
        // place. Newly ingested hot blocks enter the current epoch window.
        assert!(
            shared
                .store()
                .get(COLUMN_META, META_ARCHIVE_COLLECTED)
                .is_none()
        );
        assert_eq!(shared.store().db().epoch_segments().windows(), &[96]);
        let names = ckb_db::internal::DB::list_cf(&Default::default(), &config.path).unwrap();
        assert!(names.iter().any(|name| name == "freezer.epoch.96"));
        for col in ["2", "3", "7", "13", "15"] {
            assert!(names.iter().any(|name| name == col));
        }
    } else {
        assert_eq!(
            shared
                .store()
                .get(COLUMN_META, META_ARCHIVE_COLLECTED)
                .unwrap()
                .as_ref(),
            (archive_limit + 1).to_le_bytes()
        );
        assert_eq!(
            shared
                .store()
                .db()
                .get_pinned_default(b"freezer/cf-generation")
                .unwrap()
                .unwrap()
                .as_ref(),
            1u64.to_le_bytes()
        );
        let names = ckb_db::internal::DB::list_cf(&Default::default(), &config.path).unwrap();
        for col in ["2", "3", "7", "13", "15"] {
            assert!(!names.iter().any(|name| name == col));
            assert!(names.contains(&format!("freezer.1.{col}")));
        }
    }
    for block in &blocks {
        assert_eq!(shared.store().get_block(&block.hash()).unwrap(), *block);
        assert_eq!(before.get_block(&block.hash()).unwrap(), *block);
    }
    assert_eq!(
        shared
            .store()
            .get(COLUMN_BLOCK_BODY, b"unknown-key")
            .unwrap()
            .as_ref(),
        b"retained"
    );
    assert!(
        shared
            .store()
            .get_archived_block(&blocks[archive_limit as usize].hash())
            .is_some()
    );
    assert!(
        shared
            .store()
            .get_archived_block(&blocks[(archive_limit + 1) as usize].hash())
            .is_none()
    );
    runtime.block_on(controller.shutdown()).unwrap();
    drop((controller, before, package, shared));

    assert!(open(false).is_err());
    let (reopened, _package) = open(true).unwrap();
    for block in &blocks {
        assert_eq!(reopened.store().get_block(&block.hash()).unwrap(), *block);
    }
    assert!(!reopened.store().archive_collection_due().unwrap());
    metrics.ckb_freezer_number.set(-1);
    let worker = reopened
        .spawn_freeze()
        .expect("an enabled archive always starts its worker");
    assert_eq!(metrics.ckb_freezer_number.get(), archive_limit as i64);
    assert_eq!(metrics.ckb_freezer_state.get(), 1);
    drop(worker);
    assert_eq!(metrics.ckb_freezer_state.get(), 0);
}

#[test]
fn archiving_resumes_after_crossing_the_frozen_frontier_on_a_shorter_fork() {
    exercise_shorter_fork(false);
}

#[test]
fn epoch_archiving_resumes_after_crossing_the_frozen_frontier_on_a_shorter_fork() {
    exercise_shorter_fork(true);
}

fn exercise_shorter_fork(segmented: bool) {
    let _metrics_test = METRICS_TEST.lock().unwrap();
    ckb_metrics::METRICS_SERVICE_ENABLED.get_or_init(|| true);
    let metrics = ckb_metrics::handle().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let handle = Handle::new(runtime.handle().clone(), None);
    let config = DBConfig {
        path: directory.path().join("db"),
        ..Default::default()
    };
    let (shared, _package) = SharedBuilder::new(
        "test",
        directory.path(),
        &config,
        Some(directory.path().join("archive")),
        handle,
        Consensus::default(),
    )
    .unwrap()
    .store_config(StoreConfig {
        freezer_enable: true,
        freezer_epoch_segments: segmented,
        ..Default::default()
    })
    .build()
    .unwrap();
    let controller = FreezerController::start(
        shared.store().freezer().unwrap().clone(),
        runtime.handle().clone(),
        Default::default(),
    )
    .unwrap();
    let mut blocks = vec![shared.consensus().genesis_block().clone()];
    for _ in 0..5 {
        blocks.push(child(blocks.last().unwrap(), 1));
    }
    insert(&shared, &blocks[1..], true);
    let old = shared.cloned_snapshot();
    shared.archive_through(&controller, &old, 5).unwrap();
    assert_eq!(controller.number(), 6);
    let collect = || {
        if segmented {
            runtime
                .block_on(shared.store().collect_epoch_segments(
                    &controller,
                    || 0,
                    || false,
                    || shared.refresh_snapshot(),
                ))
                .unwrap();
        } else {
            shared
                .store()
                .collect_archive(&Default::default(), || shared.refresh_snapshot())
                .unwrap();
        }
    };
    collect();
    for block in &blocks[1..] {
        assert_eq!(shared.store().get_block(&block.hash()).unwrap(), *block);
        assert_eq!(old.get_block(&block.hash()).unwrap(), *block);
    }
    let replacement = child(&blocks[2], 2);
    let txn = shared.store().begin_transaction();
    for block in blocks[3..].iter().rev() {
        txn.detach_block(block).unwrap();
    }
    txn.commit().unwrap();
    drop(txn);
    insert(&shared, std::slice::from_ref(&replacement), true);
    shared
        .archive_through(&controller, &shared.cloned_snapshot(), 3)
        .unwrap();
    assert_eq!(controller.number(), 7);
    assert!(
        shared
            .store()
            .get_archived_block(&replacement.hash())
            .is_some()
    );
    let next = child(&replacement, 2);
    insert(&shared, std::slice::from_ref(&next), false);
    shared
        .archive_through(&controller, &shared.cloned_snapshot(), 4)
        .unwrap();
    assert_eq!(controller.number(), 7); // an unverified block remains hot
    assert_eq!(metrics.ckb_freezer_number.get(), 3);
    assert_eq!(metrics.ckb_freezer_backlog.get(), 1);
    let previous_progress = metrics.ckb_freezer_last_progress_timestamp.get();
    insert(&shared, std::slice::from_ref(&next), true);
    shared
        .archive_through(&controller, &shared.cloned_snapshot(), 4)
        .unwrap();
    assert_eq!(controller.number(), 8);
    assert_eq!(metrics.ckb_freezer_number.get(), 4);
    assert_eq!(metrics.ckb_freezer_backlog.get(), 0);
    assert!(metrics.ckb_freezer_last_progress_timestamp.get() >= previous_progress);
    collect();
    assert_eq!(shared.snapshot().get_block(&next.hash()).unwrap(), next);
    assert_eq!(
        shared.store().get_block(&blocks[5].hash()).unwrap(),
        blocks[5]
    );
    assert_eq!(old.get_block(&blocks[5].hash()).unwrap(), blocks[5]);
    runtime.block_on(controller.shutdown()).unwrap();
}

#[test]
fn worker_drops_the_first_window_only_after_its_last_epoch_and_fork_are_archivable() {
    use ckb_db_schema::freezer;
    let _metrics = METRICS_TEST.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let config = DBConfig {
        path: dir.path().join("db"),
        ..Default::default()
    };
    let (shared, _package) = SharedBuilder::new(
        "test",
        dir.path(),
        &config,
        Some(dir.path().join("archive")),
        Handle::new(runtime.handle().clone(), None),
        Consensus::default(),
    )
    .unwrap()
    .store_config(StoreConfig {
        freezer_enable: true,
        freezer_epoch_segments: true,
        ..Default::default()
    })
    .build()
    .unwrap();
    shared.ibd_finished.store(true, Ordering::Release);
    let store = shared.store();
    let writer = FreezerController::start(
        store.freezer().unwrap().clone(),
        runtime.handle().clone(),
        Default::default(),
    )
    .unwrap();
    let is_live = || {
        ckb_db::internal::DB::list_cf(&Default::default(), &config.path)
            .unwrap()
            .contains(&freezer::segment_name(0))
    };
    let mut parent = shared.consensus().genesis_block().clone();
    let verified = store.get_block_ext(&parent.hash()).unwrap();
    let mut last_members = Vec::new();
    // Use one synthetic block per epoch. Writes use the normal boundary opener
    // and the same freeze pass the background worker invokes after its timer.
    for n in 1u64..=116 {
        let b = child(&parent, 0)
            .as_advanced_builder()
            .epoch(EpochNumberWithFraction::new(n, 0, 1))
            .transaction(TransactionBuilder::default().version(n as u32).build())
            .build();
        let mut epoch = shared.consensus().genesis_epoch_ext().clone();
        epoch.set_number(n);
        epoch.set_start_number(n);
        epoch.set_length(1);
        epoch.set_last_block_hash_in_previous_epoch(parent.hash());
        let tx = store.begin_block_transaction(&b).unwrap();
        tx.insert_block(&b).unwrap();
        tx.insert_block_ext(&b.hash(), &verified).unwrap();
        tx.attach_block(&b).unwrap();
        tx.insert_tip_header(&b.header()).unwrap();
        tx.insert_epoch_ext(&parent.hash(), &epoch).unwrap();
        tx.insert_block_epoch_index(&b.hash(), &parent.hash())
            .unwrap();
        tx.insert_current_epoch_ext(&epoch).unwrap();
        if n == 15 {
            let side = b.as_advanced_builder().nonce(1u128).build();
            tx.insert_block(&side).unwrap();
            tx.insert_block_ext(&side.hash(), &verified).unwrap();
            last_members = vec![b.clone(), side];
        }
        tx.commit().unwrap();
        drop(tx);
        if n >= 115 {
            shared.store_snapshot(shared.new_snapshot(
                b.header(),
                n.into(),
                epoch,
                shared.snapshot().proposals().clone(),
            ));
            shared.freeze(&writer, &mut Default::default()).unwrap();
            assert_eq!(is_live(), n == 115);
            for member in &last_members {
                let archived = store.get_archived_block(&member.hash());
                assert_eq!(archived, (n == 116).then(|| member.data()));
                assert_eq!(
                    store.get_block(&member.hash()).unwrap().data(),
                    member.data()
                );
            }
        }
        parent = b;
    }
    runtime.block_on(writer.shutdown()).unwrap();
}

#[test]
fn unchanged_blocked_windows_defer_only_three_worker_polls() {
    let mut retry = RetirementPoll::default();
    let held = [(0, "pending"), (16, "override")];
    assert!(!retry.defer(&[0, 16]));
    retry.completed(&held);
    for _ in 0..3 {
        assert!(retry.defer(&[0, 16]));
    }
    assert!(!retry.defer(&[0, 16]));
    // If a scan errors or stops, no completion arms a new delay.
    assert!(!retry.defer(&[0, 16]));
    retry.completed(&held);
    assert!(!retry.defer(&[0, 16, 32]));
    assert!(!retry.defer(&[0, 16]));
    retry.completed(&[]);
    assert!(!retry.defer(&[]));
}
