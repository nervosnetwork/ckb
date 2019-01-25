use ckb_util::{Condvar, Mutex};
use ctrlc;
use std::path::PathBuf;
use std::sync::Arc;

pub fn wait_for_exit() {
    let exit = Arc::new((Mutex::new(()), Condvar::new()));

    // Handle possible exits
    let e = Arc::<(Mutex<()>, Condvar)>::clone(&exit);
    let _ = ctrlc::set_handler(move || {
        e.1.notify_all();
    });

    // Wait for signal
    let mut l = exit.0.lock();
    exit.1.wait(&mut l);
}

pub fn require_path_exists(path: PathBuf) -> Option<PathBuf> {
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

pub fn to_absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        let mut absulute_path = ::std::env::current_dir().expect("get current_dir");
        absulute_path.push(path);
        absulute_path
    }
}
