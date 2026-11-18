//! Rebuild into a fresh database. The source is opened read-only while its
//! database and archive locks are held. Only a verified pair receives the ready name.
use crate::archive::{ArchiveIndex, restore_payload};
use anyhow::{Context, Result, bail, ensure};
use ckb_app_config::DBConfig;
use ckb_db::internal::{
    ColumnFamily, DB, FullOptions, IteratorMode, Options, ReadOnlyDB, WriteBatch,
    ops::{FlushWal, GetColumnFamilys, GetPinned, GetPinnedCF, IterateCF, Open, OpenCF, WriteOps},
};
use ckb_db_schema::{
    COLUMN_BLOCK_ARCHIVE, COLUMN_BLOCK_BODY, COLUMN_BLOCK_EXT, COLUMN_BLOCK_EXTENSION,
    COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_META,
    COLUMN_NUMBER_HASH, COLUMNS, META_ARCHIVE_NEXT_RECORD, freezer,
};
use ckb_freezer::{FreezerReader, FreezerServiceConfig};
use ckb_types::{packed, prelude::*};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

/// Rows in the closed and independently reopened destination.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct RebuildStats {
    /// Rows copied byte-for-byte, including opaque keys and the default CF.
    pub copied: u64,
    /// Named hot duplicates omitted after comparison with the committed archive.
    pub omitted: u64,
}

/// Build `output/ready/{db,ancient}`. The calling process must have no primary
/// open on this source. The CLI runs offline and excludes concurrent processes.
pub fn stopped_pair(config: &DBConfig, ancient: &Path, output: &Path) -> Result<RebuildStats> {
    let db_path = config.path.canonicalize()?;
    let ancient = ancient.canonicalize()?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .canonicalize()?;
    ensure!(
        !parent.starts_with(&db_path) && !parent.starts_with(&ancient),
        "output must be outside both source directories"
    );
    let archive_lock =
        File::open(ancient.join("FLOCK")).context("expected a stopped CKBFZ006 archive")?;
    archive_lock
        .try_lock_shared()
        .context("source archive is in use")?;
    let _db_lock = lock_database(&db_path)?;
    let reader = FreezerReader::open(&ancient)?;
    fs::create_dir(output).context("rebuild requires a fresh output directory")?;
    File::open(parent)?.sync_all()?;
    let scratch = tempfile::tempdir_in(output)?;
    let staging = output.join("building");
    let new_db = staging.join("db");
    fs::create_dir(&staging)?;
    fs::create_dir(&new_db)?;
    let incomplete = new_db.join(ckb_db::db::INCOMPLETE_REBUILD);
    File::create(&incomplete)?.sync_all()?;
    File::open(&new_db)?.sync_all()?;

    let full = source_options(&db_path, config)?;
    let mut opts = full.db_opts.clone();
    opts.set_db_log_dir(scratch.path());
    let mut names = DB::list_cf(&opts, &db_path)?;
    names.sort();
    let mut fixed = (0..COLUMNS).map(|c| c.to_string()).collect::<Vec<_>>();
    fixed.push("default".into());
    fixed.sort();
    let mut descriptors = full
        .cf_descriptors
        .iter()
        .map(|cf| cf.name().to_owned())
        .collect::<Vec<_>>();
    descriptors.sort();
    ensure!(
        names == fixed && names == descriptors,
        "expected the fixed CKB column set and matching physical options"
    );
    let source = ReadOnlyDB::open_cf_descriptors(&opts, &db_path, full.cf_descriptors.clone())?;
    ensure!(
        source.get_pinned(freezer::SEGMENTS_KEY)?.is_none()
            && freezer::generation(source.get_pinned(freezer::GENERATION_KEY)?.as_deref())? == 0,
        "rebuild requires a fixed, generation-zero source"
    );
    let cursor = source
        .get_pinned_cf(column(&source, COLUMN_META)?, META_ARCHIVE_NEXT_RECORD)?
        .context("database has no committed archive cursor")?;
    ensure!(
        cursor.as_ref() == reader.number().to_le_bytes(),
        "recover the paired store before rebuilding"
    );
    drop(cursor);

    // Keep the proof keys on disk, so RAM is bounded by one decoded block and one write batch.
    let mut tmp_opts = Options::default();
    tmp_opts.create_if_missing(true);
    let omissions = DB::open(&tmp_opts, scratch.path().join("omissions"))?;
    let omitted = prove_duplicates(&source, &reader, &omissions)?;
    let new_ancient = staging.join("ancient");
    copy_archive(&ancient, &new_ancient)?;
    let new_reader = FreezerReader::open(&new_ancient)?;
    ensure!(
        new_reader.number() == reader.number(),
        "staged archive cursor differs"
    );
    let budget = FreezerServiceConfig::default().io_bytes as usize;
    for record in 1..reader.number() {
        ensure!(
            reader.retrieve_limited(record, budget)?
                == new_reader.retrieve_limited(record, budget)?,
            "staged archive differs at record {record}"
        );
    }

    let mut target_opts = full.db_opts.clone();
    target_opts.create_if_missing(true);
    target_opts.create_missing_column_families(true);
    let target = DB::open_cf_descriptors(&target_opts, &new_db, full.cf_descriptors.clone())?;
    let mut batch = WriteBatch::default();
    let mut bytes = 0;
    let mut copied = 0;
    for name in &names {
        let cf = column(&target, name)?;
        copied += retained_rows(&source, name, &omissions, |key, value| {
            batch.put_cf(cf, key, value)?;
            bytes += key.len() + value.len();
            if bytes >= 4 << 20 || batch.len() >= 4096 {
                target.write(&batch)?;
                batch.clear()?;
                bytes = 0;
            }
            Ok(())
        })?;
    }
    target.write(&batch)?;
    target.flush_wal(true)?;
    drop(target);
    fail::fail_point!("freezer-rebuild-after-copy", |_| anyhow::bail!(
        "interrupted after rebuild copy"
    ));

    // A single ordered merge detects missing, extra, and unequal rows, including
    // an extra final key or a failed iterator. Reopen first to check durable state.
    let target = ReadOnlyDB::open_cf_descriptors(&opts, &new_db, full.cf_descriptors)?;
    for name in &names {
        let mut actual = target.iterator_cf(column(&target, name)?, IteratorMode::Start)?;
        retained_rows(&source, name, &omissions, |key, value| {
            let (other_key, other_value) = actual.next().context("short rebuilt column")?;
            ensure!(
                &*other_key == key && &*other_value == value,
                "rebuilt column {name} differs"
            );
            Ok(())
        })?;
        ensure!(actual.next().is_none(), "extra rebuilt rows in {name}");
        actual.status()?;
    }
    drop((target, new_reader));
    let receipt = staging.join("READY");
    fs::write(
        &receipt,
        format!(
            "CKB freezer rebuild v1\narchive_next={}\ncopied_rows={copied}\nomitted_rows={omitted}\n",
            reader.number()
        ),
    )?;
    File::open(receipt)?.sync_all()?;
    fs::remove_file(incomplete)?;
    File::open(&new_db)?.sync_all()?;
    File::open(&staging)?.sync_all()?;
    fail::fail_point!("freezer-rebuild-before-publish", |_| anyhow::bail!(
        "interrupted before pair publication"
    ));
    fs::rename(&staging, output.join("ready"))?;
    File::open(output)?.sync_all()?;
    Ok(RebuildStats { copied, omitted })
}

#[cfg(unix)]
fn lock_database(path: &Path) -> Result<File> {
    use rustix::fs::{FlockOperation, fcntl_lock};
    let lock = File::open(path.join("LOCK"))?;
    // RocksDB's POSIX writer takes an exclusive F_SETLK on this file.
    fcntl_lock(&lock, FlockOperation::NonBlockingLockShared)
        .context("source database is in use")?;
    Ok(lock)
}

#[cfg(not(unix))]
fn lock_database(_: &Path) -> Result<File> {
    bail!("stopped-pair rebuild requires POSIX database locking and directory sync")
}

fn column<'a>(db: &'a impl GetColumnFamilys, name: &str) -> Result<&'a ColumnFamily> {
    db.cf_handle(name)
        .with_context(|| format!("missing column {name}"))
}

fn source_options(path: &Path, config: &DBConfig) -> Result<FullOptions> {
    let mut latest: Option<(u64, PathBuf)> = None;
    for item in fs::read_dir(path)? {
        let item = item?;
        let name = item.file_name();
        if let Some(n) = name
            .to_str()
            .and_then(|s| s.strip_prefix("OPTIONS-"))
            .and_then(|s| s.parse().ok())
            && latest.as_ref().is_none_or(|(old, _)| n > *old)
        {
            latest = Some((n, item.path()));
        }
    }
    let (_, file) = latest.context("missing physical RocksDB OPTIONS file")?;
    for file in std::iter::once(&file).chain(config.options_file.as_ref()) {
        for line in fs::read_to_string(file)?.lines() {
            if let Some((key, value)) = line.trim().split_once('=') {
                match key.trim() {
                    "db_paths" | "cf_paths" | "wal_dir" | "db_log_dir"
                        if !value.trim().is_empty() =>
                    {
                        bail!("external storage paths are unsupported: {key}")
                    }
                    "comparator" if value.trim() != "leveldb.BytewiseComparator" => {
                        bail!("unsupported comparator {value}")
                    }
                    _ => (),
                }
            }
        }
    }
    FullOptions::load_from_file(file, config.cache_size, false).map_err(Into::into)
}

fn copy_archive(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        ensure!(
            item.file_type()?.is_file(),
            "unexpected archive entry {:?}",
            item.file_name()
        );
        let file = target.join(item.file_name());
        fs::copy(item.path(), &file)?;
        File::open(file)?.sync_all()?;
    }
    File::open(target)?.sync_all()?;
    Ok(())
}

fn proof_key(col: &str, key: &[u8]) -> Vec<u8> {
    [col.as_bytes(), &[0], key].concat()
}

fn prove_duplicates(source: &ReadOnlyDB, reader: &FreezerReader, proofs: &DB) -> Result<u64> {
    let routes = column(source, COLUMN_BLOCK_ARCHIVE)?;
    let mut iter = source.iterator_cf(routes, IteratorMode::Start)?;
    let mut count = 0;
    for (key, _) in iter.by_ref() {
        let hash = packed::Byte32::from_slice(&key)?;
        let raw = source
            .get_pinned_cf(routes, &key)?
            .context("missing route")?;
        let Some(index) = ArchiveIndex::new(raw)? else {
            continue;
        };
        let bytes = reader
            .retrieve_limited(
                index.record(),
                FreezerServiceConfig::default().io_bytes as usize,
            )?
            .context("indexed archive record missing")?;
        let block = packed::Block::from_compatible_slice(&bytes)?;
        ensure!(
            block.as_reader().count_extra_fields() <= 1 && block.calc_header_hash() == hash,
            "invalid indexed block {hash}"
        );
        index.check_witnesses(&block.calc_tx_witness_hashes())?;
        let block = index.block_view(block)?;
        let ext = source.get_pinned_cf(column(source, COLUMN_BLOCK_EXT)?, &key)?;
        let Some(ext) = ext else { continue };
        if block.number() == 0
            || packed::BlockExtReader::from_compatible_slice(&ext)?
                .verified()
                .to_opt()
                .map(bool::from)
                != Some(true)
        {
            continue;
        }
        let header: packed::HeaderView = block.header().into();
        ensure!(
            source
                .get_pinned_cf(column(source, COLUMN_BLOCK_HEADER)?, &key)?
                .as_deref()
                == Some(header.as_slice()),
            "header mismatch for {hash}"
        );
        let mut known = WriteBatch::default();
        restore_payload(&block, |col, key, value| -> Result<()> {
            ensure!(
                source.get_pinned_cf(column(source, col)?, key)?.as_deref() == Some(value),
                "hot payload mismatch in {col} for {hash}"
            );
            if col != COLUMN_NUMBER_HASH {
                known.put(proof_key(col, key), [])?;
            }
            Ok(())
        })?;
        if block.extension().is_none() {
            ensure!(
                source
                    .get_pinned_cf(column(source, COLUMN_BLOCK_EXTENSION)?, &key)?
                    .is_none(),
                "unexpected hot extension for {hash}"
            );
        }
        count += known.len() as u64;
        proofs.write(&known)?;
    }
    iter.status()?;
    Ok(count)
}

fn retained_rows(
    source: &ReadOnlyDB,
    name: &str,
    omissions: &DB,
    mut row: impl FnMut(&[u8], &[u8]) -> Result<()>,
) -> Result<u64> {
    let mut count = 0;
    let payload = matches!(
        name,
        COLUMN_BLOCK_BODY | COLUMN_BLOCK_UNCLE | COLUMN_BLOCK_PROPOSAL_IDS | COLUMN_BLOCK_EXTENSION
    );
    let mut iter = source.iterator_cf(column(source, name)?, IteratorMode::Start)?;
    for (key, value) in iter.by_ref() {
        if payload && omissions.get_pinned(proof_key(name, &key))?.is_some() {
            continue;
        }
        row(&key, &value)?;
        count += 1;
    }
    iter.status()?;
    Ok(count)
}
