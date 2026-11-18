//! Old hot schemas and stopped DB/archive backups must retain the same chain data.
use super::archive::{assert_body, block, hot_fixture};
use crate::{ChainDB, ChainStore};
use ckb_db::RocksDB;
use ckb_db_schema::*;
use ckb_freezer::{Freezer, FreezerFilesBuilder};
use ckb_types::{core::BlockView, packed, prelude::*};
use std::{collections::BTreeMap, fs, path::Path};

fn copy_directory(from: &Path, to: &Path) {
    fs::create_dir(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn open_pair(path: &Path) -> ChainDB {
    ChainDB::new_with_freezer(
        RocksDB::open_in(path.join("db"), COLUMNS),
        Freezer::open_in(path.join("archive")).unwrap(),
        Default::default(),
    )
}

/// Copy both halves only after their database, archive and read handles close.
fn copy_stopped_pair(from: &Path, to: &Path) {
    fs::create_dir(to).unwrap();
    for component in ["db", "archive"] {
        copy_directory(&from.join(component), &to.join(component));
    }
}

fn assert_collected(store: &ChainDB, block: &BlockView) {
    let hash = block.hash();
    for column in [
        COLUMN_BLOCK_UNCLE,
        COLUMN_BLOCK_PROPOSAL_IDS,
        COLUMN_BLOCK_EXTENSION,
    ] {
        assert!(store.get(column, hash.as_slice()).is_none(), "hot {column}");
    }
    let key = packed::NumberHash::new_builder()
        .number(block.number())
        .block_hash(hash.clone())
        .build();
    assert!(store.get(COLUMN_NUMBER_HASH, key.as_slice()).is_none());
    for index in 0..block.transactions().len() {
        let key = packed::TransactionKey::new_builder()
            .block_hash(hash.clone())
            .index(index)
            .build();
        assert!(store.get(COLUMN_BLOCK_BODY, key.as_slice()).is_none());
    }
    assert!(store.get(COLUMN_BLOCK_ARCHIVE, hash.as_slice()).is_some());
    assert_body(store, block);
}

fn directory_image(path: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(
                directory_image(&entry.path())
                    .into_iter()
                    .map(|(name, bytes)| {
                        (
                            std::path::PathBuf::from(entry.file_name()).join(name),
                            bytes,
                        )
                    }),
            );
        } else {
            files.insert(entry.file_name().into(), fs::read(entry.path()).unwrap());
        }
    }
    files
}

/// Both source handles must be closed. Open a restored copy, never the backup.
fn verify_stopped_backup(path: &Path, archived: &BlockView) {
    let backup = path.join("backup");
    copy_stopped_pair(path, &backup);
    let image = directory_image(&backup);
    let restored = path.join("restored");
    copy_stopped_pair(&backup, &restored);
    for location in [path, restored.as_path()] {
        let store = open_pair(location);
        let end = store.freezer().unwrap().number();
        assert_eq!(store.recover_archive().unwrap(), end);
        assert_eq!(
            store.recover_archive().unwrap(),
            end,
            "recovery is idempotent"
        );
        assert_collected(&store, archived);
    }
    assert_eq!(
        directory_image(&backup),
        image,
        "restore must preserve the backup"
    );
}

#[test]
fn recovery_distinguishes_a_stale_archive_from_unindexed_committed_records() {
    let (directory, store, first) = hot_fixture(421);
    let second = block(422);
    let transaction = store.begin_transaction();
    transaction.insert_block(&second).unwrap();
    transaction.attach_block(&second).unwrap();
    transaction
        .insert_block_ext(&second.hash(), &store.get_block_ext(&first.hash()).unwrap())
        .unwrap();
    transaction.commit().unwrap();
    drop(transaction);
    super::db::archive(&store, std::slice::from_ref(&first));
    drop(store);
    let older = directory.path().join("older");
    copy_stopped_pair(directory.path(), &older);

    let store = open_pair(directory.path());
    super::db::archive(&store, std::slice::from_ref(&second));
    store.collect_archive(&Default::default(), || {}).unwrap();
    for block in [&first, &second] {
        assert_collected(&store, block);
    }
    drop(store);

    let stale = directory.path().join("stale-archive");
    fs::create_dir(&stale).unwrap();
    copy_directory(&directory.path().join("db"), &stale.join("db"));
    copy_directory(&older.join("archive"), &stale.join("archive"));
    let store = open_pair(&stale);
    let error = store.recover_archive().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("shorter than its committed database index"),
        "{error}"
    );
    drop(store);

    // A durable archive may legitimately lead the DB publication cursor after a
    // crash. Matching hot data authorizes indexing its unprocessed suffix.
    let ahead = directory.path().join("unindexed-suffix");
    fs::create_dir(&ahead).unwrap();
    copy_directory(&older.join("db"), &ahead.join("db"));
    copy_directory(&directory.path().join("archive"), &ahead.join("archive"));
    let store = open_pair(&ahead);
    assert_eq!(store.recover_archive().unwrap(), 3);
    store.collect_archive(&Default::default(), || {}).unwrap();
    for block in [&first, &second] {
        assert_collected(&store, block);
    }
    drop(store);
    let store = open_pair(&ahead);
    assert_eq!(store.recover_archive().unwrap(), 3);
    for block in [&first, &second] {
        assert_collected(&store, block);
    }
}

#[test]
fn stopped_pair_restores_collected_payloads_without_changing_the_backup() {
    let (directory, store, archived) = hot_fixture(421);
    super::db::archive(&store, std::slice::from_ref(&archived));
    assert!(
        store
            .collect_archive(&Default::default(), || {})
            .unwrap()
            .archived
            > 0
    );
    assert_collected(&store, &archived);
    drop(store);
    verify_stopped_backup(directory.path(), &archived);
}

#[test]
fn incomplete_or_unrelated_archives_cannot_supply_restored_cold_payloads() {
    let (directory, store, archived) = hot_fixture(421);
    super::db::archive(&store, std::slice::from_ref(&archived));
    store.collect_archive(&Default::default(), || {}).unwrap();
    assert_collected(&store, &archived);
    drop(store);

    // No reader and a new empty archive both fail the durable activation cursor.
    for with_empty_archive in [false, true] {
        let target = tempfile::tempdir().unwrap();
        copy_directory(&directory.path().join("db"), &target.path().join("db"));
        let db = RocksDB::open_in(target.path().join("db"), COLUMNS);
        let restored = if with_empty_archive {
            ChainDB::new_with_freezer(
                db,
                Freezer::open_in(target.path().join("archive")).unwrap(),
                Default::default(),
            )
        } else {
            ChainDB::new(db, Default::default())
        };
        let error = restored.recover_archive().unwrap_err();
        let expected = if with_empty_archive {
            "shorter"
        } else {
            "requires its archive"
        };
        assert!(error.to_string().contains(expected), "{error}");
    }

    // A same-length foreign archive passes the count check, but cannot pass the
    // exact-hash read boundary. Startup does not authenticate the entire pairing.
    let target = tempfile::tempdir().unwrap();
    copy_directory(&directory.path().join("db"), &target.path().join("db"));
    let foreign = archived.as_advanced_builder().nonce(1u128).build();
    assert_eq!(foreign.number(), archived.number());
    assert_ne!(foreign.hash(), archived.hash());
    let mut files = FreezerFilesBuilder::new(target.path().join("archive"))
        .build()
        .unwrap();
    files.append(1, foreign.data().as_slice()).unwrap();
    files.sync_all().unwrap();
    drop(files);
    let restored = open_pair(target.path());
    assert_eq!(restored.recover_archive().unwrap(), 2);
    let error = restored
        .freezer()
        .unwrap()
        .read_block(1, &archived.hash())
        .err()
        .unwrap();
    assert!(error.to_string().contains("hash mismatch"), "{error}");
    assert_eq!(
        restored.freezer().unwrap().retrieve(1).unwrap().unwrap(),
        foreign.data().as_slice()
    );

    // Loss inside the committed archive is rejected before recovery can trim it.
    for name in ["COMMIT", "INDEX", "blk000000"] {
        let target = tempfile::tempdir().unwrap();
        copy_directory(
            &directory.path().join("archive"),
            &target.path().join("archive"),
        );
        fs::remove_file(target.path().join("archive").join(name)).unwrap();
        let damaged = directory_image(&target.path().join("archive"));
        assert!(
            Freezer::open_in(target.path().join("archive")).is_err(),
            "{name}"
        );
        assert_eq!(
            directory_image(&target.path().join("archive")),
            damaged,
            "{name}"
        );
    }
}

#[test]
#[cfg(feature = "legacy-db-test")]
#[ignore = "requires CKB_LEGACY_WRITER and a new CKB_UPGRADE_DIR"]
fn old_engine_hot_data_survives_upgrade_archival_and_backup() {
    use ckb_db::{DBIterator, IteratorMode};
    use std::{io::Write, process::Command};

    let writer = std::env::var_os("CKB_LEGACY_WRITER").expect("old engine writer binary");
    let root = std::path::PathBuf::from(
        std::env::var_os("CKB_UPGRADE_DIR").expect("new upgrade evidence directory"),
    );
    fs::create_dir(&root).unwrap();
    let (_source_directory, source, archived) = hot_fixture(421);
    let mut batch = source.new_write_batch();
    batch
        .put(
            COLUMN_CELL,
            b"live-state",
            b"unchanged through upgrade and restore",
        )
        .unwrap();
    source.write_sync(&batch).unwrap();
    drop(batch);
    let seed = root.join("hot-rows");
    let mut output = fs::File::create(&seed).unwrap();
    let mut rows = Vec::new();
    // Export the original 19-column schema, before archive activation.
    for (number, column) in [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16",
        "17", "18",
    ]
    .into_iter()
    .enumerate()
    {
        let mut iter = source.db().iter(column, IteratorMode::Start).unwrap();
        for (key, value) in iter.by_ref() {
            output.write_all(&(number as u32).to_le_bytes()).unwrap();
            output.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
            output
                .write_all(&(value.len() as u32).to_le_bytes())
                .unwrap();
            output.write_all(&key).unwrap();
            output.write_all(&value).unwrap();
            rows.push((column, key, value));
        }
        iter.status().unwrap();
    }
    drop(output);
    for mode in ["sst", "wal"] {
        let directory = root.join(mode);
        fs::create_dir(&directory).unwrap();
        let path = directory.join("db");
        assert!(
            Command::new(&writer)
                .arg(&seed)
                .arg(&path)
                .arg(mode)
                .status()
                .unwrap()
                .success()
        );
        assert!(fs::read_dir(&path).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "sst")
        }));
        copy_directory(&path, &directory.join("legacy"));

        let upgraded = ChainDB::new(RocksDB::open_in(&path, COLUMNS), Default::default());
        for (column, key, expected) in &rows {
            assert_eq!(
                upgraded.get(column, key).unwrap().as_ref(),
                expected.as_ref(),
                "{mode}/{column}"
            );
        }
        assert_body(&upgraded, &archived);
        drop(upgraded);
        let store = open_pair(&directory);
        super::db::archive(&store, std::slice::from_ref(&archived));
        assert!(
            store
                .collect_archive(&Default::default(), || {})
                .unwrap()
                .archived
                > 0
        );
        assert_collected(&store, &archived);
        drop(store);
        verify_stopped_backup(&directory, &archived);
        for location in [&directory, &directory.join("restored")] {
            let store = open_pair(location);
            assert_eq!(
                store.get(COLUMN_CELL, b"live-state").unwrap().as_ref(),
                b"unchanged through upgrade and restore"
            );
        }
        println!(
            "{mode}: matched all {} legacy rows; archived, collected, restarted and restored the stopped pair",
            rows.len()
        );
    }
}
