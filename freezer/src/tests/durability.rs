//! Recovery images model loss of unsynced writes, which SIGKILL alone cannot do.
//! The model preserves the previously synced prefix. It does not emulate a
//! filesystem, prove flush ordering, or cover corruption of persisted bytes.
use super::{append, assert_prefix, open};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

type Image = BTreeMap<String, Vec<u8>>;

fn image(path: &Path) -> Image {
    fs::read_dir(path)
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            if name == "FLOCK" {
                return None;
            }
            let bytes = fs::read(entry.path()).unwrap();
            Some((name, bytes))
        })
        .collect()
}

fn restore(path: &Path, image: &Image) {
    for (name, bytes) in image {
        fs::write(path.join(name), bytes).unwrap();
    }
}

#[test]
fn lost_reordered_or_torn_uncommitted_tails_preserve_the_durable_prefix() {
    let source = tempfile::tempdir().unwrap();
    let mut files = open(source.path());
    append(&mut files, 4);
    files.sync_all().unwrap();
    let committed = image(source.path());
    append(&mut files, 9);
    let pending = image(source.path());
    drop(files);

    // Independent data, index and new-name persistence includes index-ahead-of-
    // data, data-ahead-of-index, torn tails, and absent newly created segments.
    // These are deliberately conservative recovery inputs: a segment seal may
    // make more of a real filesystem's data durable than this model retains.
    for index_state in 0..3 {
        for data_state in 0..3 {
            for retain_new_names in [false, true] {
                let mut crash = committed.clone();
                for (name, bytes) in &pending {
                    if name == "COMMIT" {
                        continue;
                    }
                    let old_len = committed.get(name).map_or(0, Vec::len);
                    if !committed.contains_key(name) && !retain_new_names {
                        continue;
                    }
                    let state = if name == "INDEX" {
                        index_state
                    } else {
                        data_state
                    };
                    let surviving = match state {
                        0 => old_len,
                        1 => old_len + (bytes.len() - old_len) / 2,
                        _ => bytes.len(),
                    };
                    let mut tail = bytes[..surviving].to_vec();
                    if state == 1 {
                        // Committed bytes are intact; an uncommitted write can
                        // leave arbitrary bytes beyond that boundary.
                        tail[old_len..].fill(0xa5);
                    }
                    crash.insert(name.clone(), tail);
                }
                crash.insert("COMMIT.tmp".into(), vec![0xff; 47]);
                let recovered = tempfile::tempdir().unwrap();
                restore(recovered.path(), &crash);
                let mut files = open(recovered.path());
                assert_prefix(&files, 4);
                append(&mut files, 13);
                files.sync_all().unwrap();
                drop(files);
                assert_prefix(&open(recovered.path()), 13);
            }
        }
    }
}

#[test]
fn either_commit_name_after_an_unsynced_rename_recovers_a_complete_batch() {
    let source = tempfile::tempdir().unwrap();
    let mut files = open(source.path());
    append(&mut files, 4);
    files.sync_all().unwrap();
    let old_commit = fs::read(source.path().join("COMMIT")).unwrap();
    append(&mut files, 9);
    files.sync_all().unwrap();
    let persisted = image(source.path());
    drop(files);

    // Before the COMMIT rename's directory sync, the data, INDEX and data-file
    // names have already been synced. Model either the old or the new COMMIT
    // name surviving; neither may yield a partial batch.
    for keep_new_commit in [false, true] {
        for keep_temporary in [false, true] {
            let mut crash = persisted.clone();
            if !keep_new_commit {
                crash.insert("COMMIT".into(), old_commit.clone());
            }
            if keep_temporary {
                crash.insert("COMMIT.tmp".into(), persisted["COMMIT"].clone());
            }
            let recovered = tempfile::tempdir().unwrap();
            restore(recovered.path(), &crash);
            let mut files = open(recovered.path());
            assert_prefix(&files, if keep_new_commit { 9 } else { 4 });
            append(&mut files, 13);
            files.sync_all().unwrap();
            drop(files);
            assert_prefix(&open(recovered.path()), 13);
        }
    }
}
