use crate::migrations::SST_REBUILD_VERSION;
use ckb_db::{RocksDB, Schema};
use ckb_db_schema::MIGRATION_VERSION_KEY;
use ckb_error::Error;
use ckb_logger::info;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use super::{internal_error, io_err, open_old_db};

pub(super) fn clean_db_path(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            cleaned.push(component.as_os_str());
        }
    }
    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

pub(super) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Path to the sibling cutover marker written while promoting a rebuilt V1 database.
pub(crate) fn promoting_marker_path(db_path: &Path) -> PathBuf {
    let cleaned = clean_db_path(db_path);
    let parent = parent_dir(&cleaned);
    let db_name = cleaned.file_name().unwrap_or_else(|| OsStr::new("db"));
    parent.join(format!(
        "{}.sst-rebuild-promoting",
        db_name.to_string_lossy()
    ))
}

fn rebuild_lock_path(db_path: &Path) -> PathBuf {
    let cleaned = clean_db_path(db_path);
    let parent = parent_dir(&cleaned);
    let db_name = cleaned.file_name().unwrap_or_else(|| OsStr::new("db"));
    parent.join(format!("{}.sst-rebuild.lock", db_name.to_string_lossy()))
}

/// Advisory file lock coordinating database startup (`open_or_create_db` / `ckb migrate`)
/// with `--sst-rebuild` cutover and crash recovery.
///
/// The sibling lock file is intentionally kept on disk after release so concurrent processes
/// always lock the same inode without an unlink race.
pub struct SstRebuildLock {
    _file: File,
}

impl SstRebuildLock {
    pub(super) fn acquire_exclusive(db_path: &Path) -> Result<Self, Error> {
        let path = rebuild_lock_path(db_path);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_err("open SST rebuild lock", &path, e))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(internal_error(format!(
                "another CKB process or SST rebuild migration is already running (lock held on {})",
                path.display()
            ))),
            Err(TryLockError::Error(e)) => Err(io_err("lock SST rebuild lock", &path, e)),
        }
    }

    /// Acquire a non-blocking shared lock on the sibling SST rebuild lock file.
    ///
    /// Callers opening or probing the active database hold this lock until RocksDB's primary
    /// `LOCK` has been acquired, preventing startup from racing with cutover directory renames.
    pub fn acquire_shared(db_path: &Path) -> Result<Self, Error> {
        let path = rebuild_lock_path(db_path);
        let parent = parent_dir(&path);
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| io_err("create parent directory for SST rebuild lock", parent, e))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| io_err("open SST rebuild lock", &path, e))?;
        match file.try_lock_shared() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(internal_error(format!(
                "an SST rebuild migration is currently in progress for {} (lock held on {}); \
                 wait for it to finish before opening the database",
                db_path.display(),
                path.display()
            ))),
            Err(TryLockError::Error(e)) => Err(io_err("lock SST rebuild lock", &path, e)),
        }
    }
}

pub(super) struct PromotingMarker {
    pub(super) migrating_name: String,
    pub(super) backup_name: String,
}

impl PromotingMarker {
    pub(super) fn write_atomic(&self, marker_path: &Path) -> Result<(), Error> {
        let parent = parent_dir(marker_path);
        let tmp_name = format!(
            "{}.tmp",
            marker_path
                .file_name()
                .unwrap_or_else(|| OsStr::new("sst-rebuild-promoting"))
                .to_string_lossy()
        );
        let tmp_path = parent.join(tmp_name);
        let content = format!("{}\n{}\n", self.migrating_name, self.backup_name);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)
            .map_err(|e| io_err("create cutover marker", &tmp_path, e))?;
        file.write_all(content.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| io_err("persist cutover marker", &tmp_path, e))?;
        drop(file);
        fs::rename(&tmp_path, marker_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            io_err("publish cutover marker", marker_path, e)
        })?;
        sync_dir(parent)
    }

    fn read(marker_path: &Path) -> Result<Self, Error> {
        let content = fs::read_to_string(marker_path)
            .map_err(|e| io_err("read cutover marker", marker_path, e))?;
        let mut lines = content.lines();
        let next_name = |lines: &mut std::str::Lines<'_>, field: &str| -> Result<String, Error> {
            lines
                .next()
                .filter(|name| is_single_normal_component(name))
                .map(str::to_owned)
                .ok_or_else(|| {
                    internal_error(format!(
                        "cutover marker {} is missing a valid {field} directory name",
                        marker_path.display()
                    ))
                })
        };
        Ok(Self {
            migrating_name: next_name(&mut lines, "migrating")?,
            backup_name: next_name(&mut lines, "backup")?,
        })
    }
}

fn is_single_normal_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

pub(super) fn sync_dir(dir: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(|e| io_err("sync directory", dir, e))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn try_atomic_exchange_dirs(a: &Path, b: &Path) -> io::Result<bool> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a_c = CString::new(a.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    let b_c = CString::new(b.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;

    #[cfg(target_os = "linux")]
    #[allow(clippy::unnecessary_cast)]
    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD as libc::c_long,
            a_c.as_ptr(),
            libc::AT_FDCWD as libc::c_long,
            b_c.as_ptr(),
            libc::RENAME_EXCHANGE as libc::c_ulong,
        )
    };
    #[cfg(target_os = "macos")]
    let ret = unsafe { libc::renamex_np(a_c.as_ptr(), b_c.as_ptr(), libc::RENAME_SWAP) };

    if ret == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    let fallback = matches!(
        err.raw_os_error(),
        Some(libc::ENOSYS | libc::EINVAL | libc::ENOTSUP | libc::EXDEV)
    ) || err.raw_os_error() == Some(libc::EOPNOTSUPP);
    if fallback { Ok(false) } else { Err(err) }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn try_atomic_exchange_dirs(_a: &Path, _b: &Path) -> io::Result<bool> {
    Ok(false)
}

pub(super) fn promote_directories(
    old_path: &Path,
    migrating_path: &Path,
    backup_path: &Path,
) -> Result<(), Error> {
    let parent = parent_dir(old_path);
    if try_atomic_exchange_dirs(old_path, migrating_path)
        .map_err(|e| io_err("atomically swap", old_path, e))?
    {
        sync_dir(parent)?;
        info!("SST rebuild: moving swapped-out old database to backup");
        fs::rename(migrating_path, backup_path)
            .map_err(|e| io_err("move swapped-out old DB to backup", backup_path, e))?;
        return sync_dir(parent);
    }

    info!("SST rebuild: moving old database to backup");
    fs::rename(old_path, backup_path)
        .map_err(|e| io_err("move old DB to backup", backup_path, e))?;
    info!("SST rebuild: promoting migrated database");
    if let Err(err) = fs::rename(migrating_path, old_path) {
        let _ = fs::rename(backup_path, old_path);
        return Err(io_err("promote migrated DB to", old_path, err));
    }
    sync_dir(parent)
}

fn verify_v1_ready(path: &Path, target_options_file: Option<&Path>) -> Result<(), Error> {
    let db =
        RocksDB::open_existing_for_validation_with_options(path, Schema::V1, target_options_file)?;
    if db.get_pinned_default(MIGRATION_VERSION_KEY)?.as_deref()
        != Some(SST_REBUILD_VERSION.as_bytes())
    {
        return Err(internal_error(format!(
            "rebuilt database {} does not have expected migration version {SST_REBUILD_VERSION}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn recover_interrupted_promotion<F>(
    old_path: &Path,
    source_options_file: Option<&Path>,
    target_options_file: Option<&Path>,
    post_swap: F,
) -> Result<(), Error>
where
    F: FnOnce() -> Result<(), Error>,
{
    let marker_path = promoting_marker_path(old_path);
    let parent = parent_dir(old_path);
    let marker = PromotingMarker::read(&marker_path)?;
    let migrating_path = parent.join(&marker.migrating_name);
    let backup_path = parent.join(&marker.backup_name);

    let old_schema = RocksDB::detect_schema(old_path)?;
    let migrating_schema = RocksDB::detect_schema(&migrating_path)?;
    let backup_schema = RocksDB::detect_schema(&backup_path)?;

    match (old_schema, migrating_schema, backup_schema) {
        (Some(Schema::V1), Some(Schema::Legacy), None) => {
            verify_v1_ready(old_path, target_options_file)?;
            info!(
                "SST rebuild recovery: moving swapped-out legacy DB {} to {}",
                migrating_path.display(),
                backup_path.display()
            );
            fs::rename(&migrating_path, &backup_path)
                .map_err(|e| io_err("finish moving legacy DB to backup", &backup_path, e))?;
            sync_dir(parent)?;
        }
        (Some(Schema::V1), None, Some(Schema::Legacy)) => {
            verify_v1_ready(old_path, target_options_file)?;
            info!(
                "SST rebuild recovery: V1 DB is already active at {}; finishing post-swap steps",
                old_path.display()
            );
        }
        (None, Some(Schema::V1), Some(Schema::Legacy)) => {
            verify_v1_ready(&migrating_path, target_options_file)?;
            if old_path.exists() {
                fs::remove_dir(old_path)
                    .map_err(|e| io_err("replace empty dir during recovery", old_path, e))?;
            }
            info!(
                "SST rebuild recovery: promoting {} to {}",
                migrating_path.display(),
                old_path.display()
            );
            fs::rename(&migrating_path, old_path)
                .map_err(|e| io_err("promote migrated DB to", old_path, e))?;
            sync_dir(parent)?;
        }
        (Some(Schema::Legacy), Some(Schema::V1), None) => {
            verify_v1_ready(&migrating_path, target_options_file)?;
            drop(open_old_db(old_path, source_options_file)?);
            info!("SST rebuild recovery: completing interrupted cutover");
            promote_directories(old_path, &migrating_path, &backup_path)?;
        }
        _ => {
            return Err(internal_error(format!(
                "cannot automatically recover interrupted SST rebuild cutover (marker {}): \
                 active={} ({old_schema:?}), migrating={} ({migrating_schema:?}), backup={} ({backup_schema:?})",
                marker_path.display(),
                old_path.display(),
                migrating_path.display(),
                backup_path.display(),
            )));
        }
    }

    post_swap()?;
    fs::remove_file(&marker_path).map_err(|e| io_err("remove cutover marker", &marker_path, e))?;
    sync_dir(parent)?;
    info!(
        "SST rebuild recovery completed successfully; old database backup is {}",
        backup_path.display()
    );
    Ok(())
}
