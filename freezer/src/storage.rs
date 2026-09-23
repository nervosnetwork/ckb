use crate::format::{COMMIT_SIZE, Commit};
use fail::fail_point;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

pub(crate) fn data_path(path: &Path, id: u32) -> PathBuf {
    path.join(format!("blk{id:06}"))
}

pub(crate) fn create_directory(path: &Path) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut ancestor = path;
    while !ancestor.try_exists()? {
        missing.push(ancestor);
        ancestor = ancestor
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
    }
    fs::create_dir_all(path)?;
    for directory in missing.into_iter().rev() {
        sync_directory(directory)?;
        sync_directory(
            directory
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    fail_point!("freezer-before-directory-sync", |_| Err(io::Error::other(
        "injected directory sync failure"
    )));
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        // FlushFileBuffers requires GENERIC_WRITE and has no documented
        // directory-fsync equivalent. Match RocksDB's Windows directory policy;
        // regular files are still flushed and COMMIT replacement is write-through.
        // This is not evidence of POSIX-equivalent power-loss durability.
        let _ = path;
        Ok(())
    }
}

#[cfg(unix)]
fn replace_commit(path: &Path) -> io::Result<()> {
    fs::rename(path.join("COMMIT.tmp"), path.join("COMMIT"))
}

#[cfg(windows)]
fn replace_commit(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    // Canonicalization also produces extended-length paths on Windows. Both
    // names are in this directory, so no cross-volume copy fallback is allowed.
    let directory = fs::canonicalize(path)?;
    let wide = |name: &str| {
        directory
            .join(name)
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let source = wide("COMMIT.tmp");
    let target = wide("COMMIT");
    // SAFETY: both terminated UTF-16 buffers stay alive for the synchronous call.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn write_commit(path: &Path, commit: &Commit) -> io::Result<()> {
    let temporary = path.join("COMMIT.tmp");
    fail_point!("freezer-before-commit-write", |_| Err(io::Error::other(
        "injected commit write failure"
    )));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temporary)?;
    let encoded = commit.encode();
    fail_point!("freezer-partial-commit-write", |_| {
        file.write_all(&encoded[..encoded.len() / 2])?;
        Err(io::Error::other("injected partial commit write"))
    });
    file.write_all(&encoded)?;
    fail_point!("freezer-after-commit-write");
    fail_point!("freezer-before-commit-sync", |_| Err(io::Error::other(
        "injected commit sync failure"
    )));
    file.sync_all()?;
    fail_point!("freezer-after-commit-sync");
    drop(file);
    fail_point!("freezer-before-commit-rename", |_| Err(io::Error::other(
        "injected commit rename failure"
    )));
    replace_commit(path)?;
    fail_point!("freezer-after-commit-rename");
    fail_point!("freezer-before-commit-directory-sync", |_| Err(
        io::Error::other("injected commit directory sync failure")
    ));
    sync_directory(path)?;
    fail_point!("freezer-after-commit-directory-sync");
    Ok(())
}

pub(crate) fn open_commit(path: &Path) -> io::Result<Commit> {
    let file = match File::open(path.join("COMMIT")) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            for entry in fs::read_dir(path)? {
                let name = entry?.file_name();
                if name != "FLOCK" && name != "COMMIT.tmp" {
                    return Err(invalid_data(
                        "archive COMMIT is missing from a nonempty directory",
                    ));
                }
            }
            // Initialization commits the empty prefix before creating data/index
            // files. A missing COMMIT can never authorize erasing an old archive.
            let empty = Commit::default();
            write_commit(path, &empty)?;
            return Ok(empty);
        }
        Err(error) => return Err(error),
    };
    let mut raw = Vec::with_capacity(COMMIT_SIZE + 1);
    file.take(COMMIT_SIZE as u64 + 1).read_to_end(&mut raw)?;
    Commit::decode(&raw)
}

pub(crate) fn remove_uncommitted_files(path: &Path, head_id: u32) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let remove = name == "COMMIT.tmp"
            || name
                .strip_prefix("blk")
                .and_then(|id| id.parse::<u32>().ok())
                .is_some_and(|id| {
                    id > head_id
                        && data_path(path, id).file_name() == Some(entry.file_name().as_os_str())
                });
        if remove {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
pub(crate) fn read_exact_at(file: &File, data: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(data, offset)
}

#[cfg(windows)]
pub(crate) fn read_exact_at(file: &File, mut data: &mut [u8], mut offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !data.is_empty() {
        match file.seek_read(data, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short archive read",
                ));
            }
            Ok(length) => {
                offset += length as u64;
                data = &mut data[length..];
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
