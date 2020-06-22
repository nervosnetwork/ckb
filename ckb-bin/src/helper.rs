use std::io::{stdin, stdout, Write};

#[cfg(not(feature = "deadlock_detection"))]
pub fn deadlock_detection() {}

#[cfg(feature = "deadlock_detection")]
pub fn deadlock_detection() {
    use ckb_logger::{info, warn};
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
