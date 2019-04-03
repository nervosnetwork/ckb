use ckb_util::{parking_lot::deadlock, Condvar, Mutex};
use ctrlc;
use log::warn;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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

pub fn deadlock_detection() {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(10));
        let deadlocks = deadlock::check_deadlock();
        if deadlocks.is_empty() {
            continue;
        }

        warn!("{} deadlocks detected", deadlocks.len());
        for (i, threads) in deadlocks.iter().enumerate() {
            warn!("Deadlock #{}", i);
            for t in threads {
                warn!("Thread Id {:#?}", t.thread_id());
                warn!("{:#?}", t.backtrace());
            }
        }
    });
}
