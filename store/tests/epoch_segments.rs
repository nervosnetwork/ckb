//! Compare physical maintenance on identical blocks, and exercise the manifest
//! across primary restarts. No foreground traffic during sequence-number checks.
use ckb_db::{
    RocksDB,
    internal::{DB, ffi},
};
use ckb_db_schema::{COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_EXTENSION, COLUMNS, freezer};
use ckb_store::{ChainDB, ChainStore, Freezer, FreezerController};
use ckb_types::{
    core::{BlockBuilder, BlockExt, BlockView, EpochNumberWithFraction, TransactionBuilder},
    packed,
    prelude::*,
};
use std::path::Path;

fn block(n: u64, epoch: u64, nonce: u128) -> BlockView {
    BlockBuilder::default()
        .number(n)
        .nonce(nonce)
        .epoch(EpochNumberWithFraction::new(epoch, 0, 1))
        .transactions((0..16u32).map(|i| {
            TransactionBuilder::default()
                .version(n as u32 * 100 + i)
                .witness(packed::Bytes::from(vec![i as u8; 1024]))
                .build()
        }))
        .extension(Some(vec![n as u8, nonce as u8].into()))
        .build()
}

fn store(root: &Path) -> ChainDB {
    let db = ChainDB::new_with_freezer(
        RocksDB::open_in(root.join("db"), COLUMNS),
        Freezer::open_in(root.join("archive")).unwrap(),
        Default::default(),
    );
    db.recover_archive().unwrap();
    db
}

fn ext(verified: bool) -> BlockExt {
    BlockExt {
        received_at: 0,
        total_difficulty: Default::default(),
        total_uncles_count: 0,
        verified: Some(verified),
        txs_fees: vec![],
        cycles: None,
        txs_sizes: None,
    }
}

fn put(s: &ChainDB, b: &BlockView, verified: bool) {
    let tx = s.begin_block_transaction(b).unwrap();
    tx.insert_block(b).unwrap();
    tx.insert_block_ext(&b.hash(), &ext(verified)).unwrap();
    tx.commit().unwrap();
}
fn route(s: &ChainDB, b: &BlockView) -> Option<u64> {
    s.get(COLUMN_BLOCK_ARCHIVE, b.hash().as_slice())
        .and_then(|v| freezer::routed_segment(&v))
}
fn names(root: &Path) -> Vec<String> {
    DB::list_cf(&Default::default(), root.join("db")).unwrap()
}
fn sequence(s: &ChainDB) -> u64 {
    // The DB handle stays alive throughout this read-only native diagnostic.
    unsafe { ffi::rocksdb_get_latest_sequence_number(s.db().inner().base_db_ptr()) }
}
fn time_reads(s: &ChainDB, blocks: &[BlockView], label: &str) {
    // Seed changes order across samples, not across arms. No forced filesystem
    // eviction; measurements cover a warm local diagnostic, not disk latency.
    let mut rng = 7u64;
    for class in ["whole_block", "one_transaction"] {
        let mut samples = Vec::with_capacity(512);
        for i in 0..576 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            let block = &blocks[32 + ((rng >> 32) as usize % 32)];
            let tx = block.transaction(0).unwrap();
            let now = std::time::Instant::now();
            let ok = if class == "whole_block" {
                let actual = s.get_block(&block.hash()).unwrap();
                let elapsed = now.elapsed();
                if i >= 64 {
                    samples.push(elapsed.as_nanos());
                }
                actual.data() == block.data()
            } else {
                let actual = s.get_transaction(&tx.hash()).unwrap();
                let elapsed = now.elapsed();
                if i >= 64 {
                    samples.push(elapsed.as_nanos());
                }
                actual.0 == tx && actual.1 == block.hash()
            };
            assert!(ok);
        }
        println!("READ_NS arm={label} class={class} samples={samples:?}");
    }
}
fn writer(s: &ChainDB) -> FreezerController {
    FreezerController::start(
        s.freezer().unwrap().clone(),
        tokio::runtime::Handle::current(),
        Default::default(),
    )
    .unwrap()
}
fn assert_full(s: &impl ChainStore, b: &BlockView) {
    let h = b.hash();
    assert_eq!(s.get_block(&h).unwrap().data(), b.data());
    assert_eq!(s.get_block_body(&h), b.transactions());
    assert_eq!(s.get_cellbase(&h), b.transaction(0));
    assert_eq!(s.get_packed_block(&h), Some(b.data()));
    assert_eq!(s.get_block_txs_hashes(&h), b.tx_hashes());
    assert_eq!(s.get_block_extension(&h), b.extension());
}

#[tokio::test]
async fn normal_rollover_rewrites_no_hot_rows_and_drops_only_a_complete_segment() {
    let mut blocks: Vec<_> = (1..=32).map(|n| block(n, 15, 0)).collect();
    blocks.extend((33..=64).map(|n| block(n, 16, 0)));
    let epoch_first = std::env::var_os("CKB_EPOCH_FIRST").is_some();
    for segmented in [epoch_first, !epoch_first] {
        let dir = tempfile::tempdir().unwrap();
        let s = store(dir.path());
        if segmented {
            s.enable_epoch_segments().unwrap();
        }
        let first_start = sequence(&s);
        for b in &blocks[..32] {
            put(&s, b, true);
        }
        let first_puts = sequence(&s) - first_start;
        let old = s.get_snapshot();
        if segmented {
            let seq = sequence(&s);
            s.db().open_epoch_segment(16).unwrap();
            assert_eq!(
                sequence(&s) - seq,
                1,
                "only the durable manifest was written at rollover"
            );
            assert_eq!(
                s.db()
                    .get_snapshot()
                    .segment_iter(16, ckb_db::IteratorMode::Start)
                    .unwrap()
                    .count(),
                0
            );
        }
        let second_start = sequence(&s);
        for b in &blocks[32..] {
            put(&s, b, true);
        }
        let ingest_writes = first_puts + (sequence(&s) - second_start);
        let arm = if segmented { "epoch" } else { "e4" };
        println!("INGEST arm={arm} blocks=64 user_key_writes={ingest_writes}");
        if segmented {
            let sample = &blocks[32];
            let tx_key = packed::TransactionKey::new_builder()
                .block_hash(sample.hash())
                .index(0u32)
                .build();
            let len = s
                .db()
                .segment_get(16, tx_key.as_slice())
                .unwrap()
                .unwrap()
                .len();
            println!(
                "TX_VALUE block_bytes={} row_bytes={len}",
                sample.data().as_slice().len()
            );
        }
        for b in &blocks {
            let tx = s.begin_transaction();
            tx.attach_block(b).unwrap();
            tx.commit().unwrap();
        }
        time_reads(&s, &blocks, if segmented { "epoch" } else { "e4" });
        let w = writer(&s);
        w.append(blocks[..32].iter().map(BlockView::data).collect())
            .await
            .unwrap();
        s.recover_archive().unwrap();
        let seq = sequence(&s);
        if segmented {
            let r = s
                .collect_epoch_segments(&w, || 16, || false, || {})
                .await
                .unwrap();
            assert_eq!(r.dropped_segments, 1);
            // Exactly the retirement manifest and legacy WAL-checkpoint marker.
            // The published archive routes were already present at 'seq'.
            assert_eq!(sequence(&s) - seq, 2);
            assert!(!names(dir.path()).contains(&freezer::segment_name(0)));
            for b in &blocks[32..] {
                assert_eq!(route(&s, b), Some(16));
            }
            println!(
                "ARM=epoch retired=32 retained=32 hot_copy_bytes=0 open_db_writes=1 retire_db_writes=2"
            );
        } else {
            let r = s.collect_archive(&Default::default(), || {}).unwrap();
            assert!(r.copied > 0 && r.copied_bytes > 0);
            println!(
                "ARM=e4 retired=32 retained=32 hot_copy_entries={} hot_copy_bytes={} maintenance_db_writes={}",
                r.copied,
                r.copied_bytes,
                sequence(&s) - seq
            );
        }
        for b in &blocks {
            assert_full(&s, b);
        }
        for b in &blocks[..32] {
            assert_full(&old, b);
        }
        w.shutdown().await.unwrap();
        drop((old, w, s));
        let reopened = store(dir.path());
        for b in &blocks {
            assert_full(&reopened, b);
        }
    }
}

#[tokio::test]
async fn late_forks_block_retirement_until_verified_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    s.enable_epoch_segments().unwrap();
    let a = block(9, 15, 1);
    let fork = block(9, 15, 2);
    let late = block(9, 15, 3);
    let later = block(9, 15, 4);
    put(&s, &a, true);
    put(&s, &fork, false);
    s.db().open_epoch_segment(16).unwrap();
    put(&s, &late, true);
    assert_eq!(route(&s, &late), Some(16));
    assert_eq!(route(&s, &fork), Some(0));
    let w = writer(&s);
    let r = s
        .collect_epoch_segments(&w, || 16, || false, || {})
        .await
        .unwrap();
    assert_eq!(r.blocked_segments.len(), 1);
    assert_eq!(r.blocked_segments[0].0, 0);
    assert_eq!(r.dropped_segments, 0);
    assert!(names(dir.path()).contains(&freezer::segment_name(0)));
    assert!(s.get_archived_block(&a.hash()).is_some());
    assert_eq!(s.get_archived_block(&fork.hash()), None);
    w.shutdown().await.unwrap();
    drop((s, w));
    let s = store(dir.path());
    assert_eq!(s.db().epoch_segments().windows(), &[0, 16]);
    let tx = s.begin_transaction();
    tx.insert_block_ext(&fork.hash(), &ext(true)).unwrap();
    tx.commit().unwrap();
    drop(tx);
    let w = writer(&s);
    let r = s
        .collect_epoch_segments(&w, || 16, || false, || {})
        .await
        .unwrap();
    assert_eq!(r.dropped_segments, 1);
    assert!(!names(dir.path()).contains(&freezer::segment_name(0)));
    put(&s, &later, true);
    assert_eq!(route(&s, &later), Some(16));
    for b in [&a, &fork, &late, &later] {
        assert_full(&s, b);
    }
    w.shutdown().await.unwrap();
    drop((w, s));
    let s = store(dir.path());
    assert_eq!(s.db().epoch_segments().windows(), &[16]);
    for b in [&a, &fork, &late, &later] {
        assert_full(&s, b);
    }
}

#[tokio::test]
async fn transaction_and_batch_edits_replace_the_hash_route_without_mutating_sealed_payloads() {
    let d = tempfile::tempdir().unwrap();
    let s = store(d.path());
    s.enable_epoch_segments().unwrap();
    let a = block(1, 0, 1);
    let b = block(1, 0, 2);
    put(&s, &a, true);
    put(&s, &b, true);
    let old = s.get_snapshot();
    s.db().open_epoch_segment(16).unwrap();
    let tx = s.begin_transaction();
    tx.delete(COLUMN_BLOCK_EXTENSION, a.hash().as_slice())
        .unwrap();
    assert!(tx.get_block_extension(&a.hash()).is_none());
    drop(tx);
    assert_full(&s, &a);
    let tx = s.begin_transaction();
    tx.delete(COLUMN_BLOCK_EXTENSION, a.hash().as_slice())
        .unwrap();
    tx.commit().unwrap();
    drop(tx);
    let mut wb = s.new_write_batch();
    wb.delete(COLUMN_BLOCK_EXTENSION, b.hash().as_slice())
        .unwrap();
    s.write_sync(&wb).unwrap();
    drop(wb);
    assert_full(&old, &a);
    assert_full(&old, &b);
    drop(old);
    assert!(
        s.get_block_extension(&a.hash()).is_none() && s.get_block_extension(&b.hash()).is_none()
    );
    let w = writer(&s);
    let r = s
        .collect_epoch_segments(&w, || 16, || false, || {})
        .await
        .unwrap();
    assert_eq!(r.blocked_segments, [(0, "fixed override")]);
    w.shutdown().await.unwrap();
    drop((s, w));
    let s = store(d.path());
    assert_eq!(s.db().epoch_segments().windows(), &[0, 16]);
    assert!(
        s.get_block_extension(&a.hash()).is_none() && s.get_block_extension(&b.hash()).is_none()
    );
}

#[tokio::test]
async fn incorrect_same_height_record_or_witness_index_preserves_the_sealed_segment() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    s.enable_epoch_segments().unwrap();
    let a = block(9, 15, 1);
    let fork = block(9, 15, 2);
    put(&s, &a, true);
    put(&s, &fork, true);
    s.db().open_epoch_segment(16).unwrap();
    let w = writer(&s);
    w.append(vec![a.data(), fork.data()]).await.unwrap();
    s.recover_archive().unwrap();
    let route = |b: &BlockView| {
        s.get(COLUMN_BLOCK_ARCHIVE, b.hash().as_slice())
            .unwrap()
            .to_vec()
    };
    let original = route(&a);
    let mut wrong_witness = original.clone();
    wrong_witness[8] ^= 1;
    let set_route = |value: &[u8]| {
        let mut batch = s.db().new_write_batch();
        batch
            .put(COLUMN_BLOCK_ARCHIVE, a.hash().as_slice(), value)
            .unwrap();
        s.db().write_sync(&batch).unwrap();
    };
    for bad in [route(&fork), wrong_witness] {
        set_route(&bad);
        assert!(
            s.collect_epoch_segments(&w, || 16, || false, || {})
                .await
                .is_err()
        );
        assert!(names(dir.path()).contains(&freezer::segment_name(0)));
        assert!(
            s.db()
                .segment_get(0, a.hash().as_slice())
                .unwrap()
                .is_some()
        );
    }
    set_route(&original);
    let result = s
        .collect_epoch_segments(&w, || 16, || false, || {})
        .await
        .unwrap();
    assert_eq!(result.dropped_segments, 1);
    assert_full(&s, &a);
    assert_full(&s, &fork);
    w.shutdown().await.unwrap();
}

#[tokio::test]
async fn stray_transaction_without_metadata_prevents_retirement() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    s.enable_epoch_segments().unwrap();
    let b = block(1, 0, 1);
    put(&s, &b, true);
    let key = packed::TransactionKey::new_builder()
        .block_hash(b.hash())
        .index(16u32)
        .build();
    let tx = s.db().transaction();
    tx.put_segment(0, key.as_slice(), b"unexpected extra row")
        .unwrap();
    tx.commit().unwrap();
    drop(tx);
    s.db().open_epoch_segment(16).unwrap();
    let w = writer(&s);
    assert!(
        s.collect_epoch_segments(&w, || 16, || false, || {})
            .await
            .is_err()
    );
    assert!(names(dir.path()).contains(&freezer::segment_name(0)));
    assert_full(&s, &b);
    w.shutdown().await.unwrap();
}

#[tokio::test]
async fn receding_retention_boundary_holds_a_proven_segment() {
    let d = tempfile::tempdir().unwrap();
    let store = store(d.path());
    store.enable_epoch_segments().unwrap();
    let b = block(1, 0, 1);
    put(&store, &b, true);
    store.db().open_epoch_segment(16).unwrap();
    let w = writer(&store);
    let first = std::cell::Cell::new(true);
    let s = store
        .collect_epoch_segments(
            &w,
            || if first.replace(false) { 16 } else { 0 },
            || false,
            || {},
        )
        .await
        .unwrap();
    assert_eq!(s.dropped_segments, 0);
    assert_eq!(store.db().epoch_segments().windows(), [0, 16]);
    assert_full(&store, &b);
    assert_eq!(
        store
            .collect_epoch_segments(&w, || 16, || false, || {})
            .await
            .unwrap()
            .dropped_segments,
        1
    );
    w.shutdown().await.unwrap();
}

#[tokio::test]
async fn retirement_checks_cached_transaction_identity_against_sealed_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(dir.path());
    s.enable_epoch_segments().unwrap();
    let b = block(1, 0, 1);
    put(&s, &b, true);
    let w = writer(&s);
    w.append(vec![b.data()]).await.unwrap();
    s.recover_archive().unwrap();
    let key = packed::TransactionKey::new_builder()
        .block_hash(b.hash())
        .index(0u32)
        .build();
    let original: packed::TransactionView = b.transaction(0).unwrap().into();
    let inconsistent = original
        .as_builder()
        .witness_hash(packed::Byte32::default())
        .build();
    let tx = s.db().transaction();
    tx.put_segment(0, key.as_slice(), inconsistent.as_slice())
        .unwrap();
    tx.commit().unwrap();
    drop(tx);
    s.db().open_epoch_segment(16).unwrap();
    assert!(
        s.collect_epoch_segments(&w, || 16, || false, || {})
            .await
            .is_err()
    );
    assert_eq!(s.db().epoch_segments().windows(), [0, 16]);
    w.shutdown().await.unwrap();
}
