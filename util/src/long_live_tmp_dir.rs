use once_cell::sync::OnceCell;
use std::sync::{Mutex, MutexGuard};
use tempfile::TempDir;

static TMP_DIRS: OnceCell<Mutex<TempDir>> = OnceCell::new();

/// open a temporary dir, the dir will be deleted when current process exit.
pub fn long_live_tmp_dir() -> MutexGuard<'static, TempDir> {
    TMP_DIRS
        .get_or_init(|| Mutex::new(tempfile::tempdir().unwrap()))
        .lock()
        .unwrap()
}
