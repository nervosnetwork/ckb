use ckb_logger::info;
use std::io::{stdin, stdout, Write};

#[cfg(feature = "with_pyroscope")]
use ckb_app_config::ExitCode;
#[cfg(feature = "with_pyroscope")]
use pyroscope::{
    pyroscope::{PyroscopeAgentReady, PyroscopeAgentRunning},
    PyroscopeAgent,
};
#[cfg(feature = "with_pyroscope")]
use pyroscope_pprofrs::{pprof_backend, PprofConfig};

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

#[cfg(feature = "with_pyroscope")]
pub struct PyroscopeGuard {
    ready: Option<PyroscopeAgent<PyroscopeAgentReady>>,
    running: Option<PyroscopeAgent<PyroscopeAgentRunning>>,
}

#[cfg(feature = "with_pyroscope")]
impl PyroscopeGuard {
    pub fn new() -> PyroscopeGuard {
        if let Some((host, name, auth)) = Self::params() {
            if let Ok(ready) = PyroscopeAgent::builder(&host, &name)
                .backend(pprof_backend(PprofConfig::new().sample_rate(100)))
                .auth_token(&auth)
                .build()
            {
                println!("Pyroscope ready {} {} {}", host, name, auth);
                return PyroscopeGuard {
                    ready: Some(ready),
                    running: None,
                };
            }
        }
        PyroscopeGuard {
            ready: None,
            running: None,
        }
    }

    fn params() -> Option<(String, String, String)> {
        use std::env;

        let host = env::var("PYROSCOPE_HOST").ok()?;
        let name = env::var("PYROSCOPE_NAME").ok()?;
        let auth = env::var("PYROSCOPE_AUTH").ok()?;
        Some((host, name, auth))
    }

    pub fn start(&mut self) -> Result<(), ExitCode> {
        if let Some(ready) = self.ready.take() {
            self.running = Some(ready.start().map_err(|e| {
                eprintln!("Pyroscope error: {}", e);
                ExitCode::Failure
            })?);
        }
        Ok(())
    }

    pub fn shutdown(mut self) -> Result<(), ExitCode> {
        if let Some(running) = self.running.take() {
            let ready = running.stop().map_err(|e| {
                eprintln!("Pyroscope error: {}", e);
                ExitCode::Failure
            })?;

            ready.shutdown()
        }
        Ok(())
    }
}
