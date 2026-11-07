//! Thread CPU clocks for calibration, execution receipts and cooperative limits.
//!
//! The synchronous owner takes the authoritative receipt. A transferable monitor
//! observes CPU progress while execution is active. Its readings can lag or include
//! later work after execution returns; it must never bill or override the receipt.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use nix::time::ClockId;
use std::{io, marker::PhantomData, rc::Rc, time::Duration};

/// A precise timestamp that cannot leave the thread it measures.
pub(super) struct CpuTime {
    total: Duration,
    _thread: PhantomData<Rc<()>>,
}

impl CpuTime {
    pub(super) fn now() -> io::Result<Self> {
        Ok(Self {
            total: platform::current_time()?,
            _thread: PhantomData,
        })
    }

    pub(super) fn elapsed(&self) -> io::Result<Duration> {
        platform::current_time()?
            .checked_sub(self.total)
            .ok_or_else(|| io::Error::other("thread CPU clock moved backwards"))
    }

    pub(super) fn start_monitored() -> io::Result<(Self, CpuMonitor)> {
        let target = platform::ThreadClock::current()?;
        let start = Self::now()?;
        let monitor = CpuMonitor {
            target,
            start: start.total,
        };
        Ok((start, monitor))
    }
}

/// Owns the target clock independently of the execution's final receipt.
pub(super) struct CpuMonitor {
    target: platform::ThreadClock,
    start: Duration,
}

impl CpuMonitor {
    pub(super) fn observed(&self) -> io::Result<Duration> {
        // Darwin's remote reading may lag the precise starting timestamp. Zero
        // means no proven progress; only the precise receipt checks monotonicity.
        Ok(self.target.read()?.saturating_sub(self.start))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_posix_clock(clock: ClockId) -> io::Result<Duration> {
    let time = clock.now()?;
    Ok(Duration::new(
        time.tv_sec().try_into().map_err(io::Error::other)?,
        time.tv_nsec().try_into().map_err(io::Error::other)?,
    ))
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub(super) fn current_time() -> io::Result<Duration> {
        read_posix_clock(ClockId::CLOCK_THREAD_CPUTIME_ID)
    }

    pub(super) struct ThreadClock(ClockId);

    impl ThreadClock {
        pub(super) fn current() -> io::Result<Self> {
            let mut clock = 0;
            // SAFETY: pthread_self is live in this call; clock is writable.
            let error = unsafe { libc::pthread_getcpuclockid(libc::pthread_self(), &mut clock) };
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error));
            }
            Ok(Self(ClockId::from_raw(clock)))
        }

        pub(super) fn read(&self) -> io::Result<Duration> {
            // A clock ID does not keep its thread alive. After exit it may fail
            // or refer to a reused ID; this advisory reading never becomes a bill.
            read_posix_clock(self.0)
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    // Keep the owned Mach port API together; libc only exposes part of it.
    unsafe extern "C" {
        fn mach_thread_self() -> libc::mach_port_t;
        static mut mach_task_self_: libc::mach_port_t;
        fn mach_port_deallocate(
            task: libc::mach_port_t,
            name: libc::mach_port_t,
        ) -> libc::kern_return_t;
    }

    pub(super) fn current_time() -> io::Result<Duration> {
        // Includes current CPU time not yet reflected in THREAD_BASIC_INFO.
        read_posix_clock(ClockId::CLOCK_THREAD_CPUTIME_ID)
    }

    pub(super) struct ThreadClock(libc::mach_port_t);

    impl ThreadClock {
        pub(super) fn current() -> io::Result<Self> {
            // SAFETY: returns one owned send right for the calling thread.
            let port = unsafe { mach_thread_self() };
            if port == libc::MACH_PORT_NULL as libc::mach_port_t {
                return Err(io::Error::other("could not acquire the thread port"));
            }
            Ok(Self(port))
        }

        pub(super) fn read(&self) -> io::Result<Duration> {
            let mut info = std::mem::MaybeUninit::<libc::thread_basic_info>::uninit();
            let mut count = libc::THREAD_BASIC_INFO_COUNT;
            // SAFETY: this object owns the send right; buffer and count match
            // THREAD_BASIC_INFO. A dead thread returns a kernel error.
            let result = unsafe {
                libc::thread_info(
                    self.0,
                    libc::THREAD_BASIC_INFO as libc::thread_flavor_t,
                    info.as_mut_ptr().cast(),
                    &mut count,
                )
            };
            if result != libc::KERN_SUCCESS || count != libc::THREAD_BASIC_INFO_COUNT {
                return Err(io::Error::other(format!(
                    "thread_info failed: {result}, count {count}"
                )));
            }
            // SAFETY: successful THREAD_BASIC_INFO with the expected count
            // initializes the complete structure.
            let info = unsafe { info.assume_init() };
            Ok(duration(info.user_time)?.saturating_add(duration(info.system_time)?))
        }
    }

    fn duration(time: libc::time_value_t) -> io::Result<Duration> {
        let seconds = time.seconds.try_into().map_err(io::Error::other)?;
        let micros = time.microseconds.try_into().map_err(io::Error::other)?;
        Ok(Duration::from_secs(seconds).saturating_add(Duration::from_micros(micros)))
    }

    impl Drop for ThreadClock {
        fn drop(&mut self) {
            // SAFETY: releases exactly the send right acquired by mach_thread_self.
            unsafe { mach_port_deallocate(mach_task_self_, self.0) };
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use winapi::{
        shared::minwindef::{FALSE, FILETIME},
        um::{
            handleapi::DuplicateHandle,
            processthreadsapi::{GetCurrentProcess, GetCurrentThread, GetThreadTimes},
            winnt::{DUPLICATE_SAME_ACCESS, HANDLE},
        },
    };

    pub(super) fn current_time() -> io::Result<Duration> {
        // SAFETY: the pseudo handle is used synchronously on its own thread.
        read(unsafe { GetCurrentThread() })
    }

    pub(super) struct ThreadClock(OwnedHandle);

    impl ThreadClock {
        pub(super) fn current() -> io::Result<Self> {
            let mut handle = std::ptr::null_mut();
            // SAFETY: duplicate the calling thread's pseudo handle into this
            // process. The resulting real handle may be queried from any thread.
            let result = unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    GetCurrentThread(),
                    GetCurrentProcess(),
                    &mut handle,
                    0,
                    FALSE,
                    DUPLICATE_SAME_ACCESS,
                )
            };
            if result == FALSE {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful duplication transfers one valid owned handle.
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle.cast()) }))
        }

        pub(super) fn read(&self) -> io::Result<Duration> {
            read(self.0.as_raw_handle().cast())
        }
    }

    fn read(handle: HANDLE) -> io::Result<Duration> {
        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = creation;
        let mut kernel = creation;
        let mut user = creation;
        // SAFETY: handle is either the current-thread pseudo handle or an owned
        // real thread handle; each output points to separate writable storage.
        if unsafe { GetThreadTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
            == FALSE
        {
            return Err(io::Error::last_os_error());
        }
        Ok(duration(kernel).saturating_add(duration(user)))
    }

    fn duration(time: FILETIME) -> Duration {
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Duration::new(
            ticks / 10_000_000,
            ((ticks % 10_000_000) as u32).saturating_mul(100),
        )
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;

    pub(super) fn current_time() -> io::Result<Duration> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "thread CPU clock is unavailable",
        ))
    }

    pub(super) struct ThreadClock;

    impl ThreadClock {
        pub(super) fn current() -> io::Result<Self> {
            current_time().map(|_| Self)
        }

        pub(super) fn read(&self) -> io::Result<Duration> {
            current_time()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn monitor_follows_its_target_and_the_final_receipt_survives_thread_exit() {
        let (monitor, received) = mpsc::channel();
        let (measured, receipt) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let limit = Duration::from_millis(20);
        let thread = std::thread::spawn(move || {
            let (start, target) = CpuTime::start_monitored().unwrap();
            monitor.send(target).unwrap();
            while start.elapsed().unwrap() < limit + Duration::from_millis(1) {
                std::hint::spin_loop();
            }
            measured.send(start.elapsed().unwrap()).unwrap();
            released.recv().unwrap();
        });
        let monitor = received.recv().unwrap();
        let elapsed = receipt.recv().unwrap();
        assert!(elapsed >= limit);
        // Observe the remote update itself; publishing the receipt does not
        // prove that the worker has already blocked or flushed its CPU counters.
        let watchdog = std::time::Instant::now();
        while monitor.observed().unwrap() < limit {
            assert!(watchdog.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(1));
        }
        release.send(()).unwrap();
        thread.join().unwrap();
        // The monitor may now fail or retain a terminal reading. Neither can
        // change the already returned authoritative receipt.
        let _ = monitor.observed();
    }

    #[test]
    fn lagging_monitor_and_backwards_precise_clock_are_distinct() {
        let (mut start, mut monitor) = CpuTime::start_monitored().unwrap();
        monitor.start = Duration::MAX;
        assert_eq!(monitor.observed().unwrap(), Duration::ZERO);
        start.total = Duration::MAX;
        assert!(start.elapsed().is_err());
    }
}
