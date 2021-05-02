use ckb_logger::info;
use std::io::{stdin, stdout, Write};

#[cfg(not(feature = "deadlock_detection"))]
pub fn deadlock_detection() {}

#[cfg(feature = "deadlock_detection")]
pub fn deadlock_detection() {
    use ckb_logger::warn;
    use ckb_util::parking_lot::deadlock;
    use std::{thread, time::Duration};

    info!("deadlock_detection enable");
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

pub fn prompt(msg: &str) -> String {
    let stdout = stdout();
    let mut stdout = stdout.lock();
    let stdin = stdin();

    write!(stdout, "{}", msg).unwrap();
    stdout.flush().unwrap();

    let mut input = String::new();
    let _ = stdin.read_line(&mut input);

    input
}

/// Raise the soft open file descriptor resource limit to the hard resource
/// limit.
///
/// # Panics
///
/// Panics if [`libc::getrlimit`], [`libc::setrlimit`], [`libc::sysctl`], [`libc::getrlimit`] or [`libc::setrlimit`]
/// fail.
///
/// darwin_fd_limit exists to work around an issue where launchctl on Mac OS X
/// defaults the rlimit maxfiles to 256/unlimited. The default soft limit of 256
/// ends up being far too low for our multithreaded scheduler testing, depending
/// on the number of cores available.
pub fn raise_fd_limit() {
    if let Some(limit) = fdlimit::raise_fd_limit() {
        info!("raise_fd_limit newly-increased limit: {}", limit);
    }
}
