//! Utilities for tokio runtime.
//!
//! Handle is a newtype wrap and unwrap tokio::Handle, it is workaround with Rust Orphan Rules.
//! We need `Handle` impl ckb spawn trait decouple tokio dependence

use ckb_spawn::Spawn;
use ckb_stop_handler::{SignalSender, StopHandler};
use core::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;
use tokio::runtime::Builder;
use tokio::runtime::Handle as TokioHandle;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use tokio;
pub use tokio::runtime::Runtime;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// Handle to the runtime.
#[derive(Debug, Clone)]
pub struct Handle {
    pub(crate) inner: TokioHandle,
}

impl Handle {
    /// Enter the runtime context. This allows you to construct types that must
    /// have an executor available on creation such as [`tokio::time::Sleep`] or [`tokio::net::TcpStream`].
    /// It will also allow you to call methods such as [`tokio::spawn`].
    pub fn enter<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _enter = self.inner.enter();
        f()
    }

    /// Spawns a future onto the runtime.
    ///
    /// This spawns the given future onto the runtime's executor
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.inner.spawn(future)
    }

    /// Run a future to completion on the Tokio runtime from a synchronous context.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.inner.block_on(future)
    }

    /// Spawns a future onto the runtime blocking pool.
    ///
    /// This spawns the given future onto the runtime's blocking executor
    pub fn spawn_blocking<F, R>(&self, f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.spawn_blocking(f)
    }

    /// Transform to inner tokio handler
    pub fn into_inner(self) -> TokioHandle {
        self.inner
    }
}

/// Create new threaded_scheduler tokio Runtime, return `Runtime`
pub fn new_global_runtime() -> (Handle, Runtime) {
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_name("GlobalRt")
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicU32 = AtomicU32::new(0);
            let id = ATOMIC_ID
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    // A long thread name will cut to 15 characters in debug tools.
                    // Such as "top", "htop", "gdb" and so on.
                    // It's a kernel limit.
                    //
                    // So if we want to see the whole name in debug tools,
                    // this number should have 6 digits at most,
                    // since the prefix uses 9 characters in below code.
                    //
                    // There still has a issue:
                    // When id wraps around, we couldn't know whether the old id
                    // is released or not.
                    // But we can ignore this, because it's almost impossible.
                    if n >= 999_999 {
                        Some(0)
                    } else {
                        Some(n + 1)
                    }
                })
                .expect("impossible since the above closure must return Some(number)");
            format!("GlobalRt-{id}")
        })
        .build()
        .expect("ckb runtime initialized");

    let handle = runtime.handle().clone();

    (Handle { inner: handle }, runtime)
}

/// Create new threaded_scheduler tokio Runtime, return `Handle` and background thread join handle
pub fn new_background_runtime() -> (Handle, StopHandler<()>) {
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .thread_name("GlobalRt")
        .thread_name_fn(|| {
            static ATOMIC_ID: AtomicU32 = AtomicU32::new(0);
            let id = ATOMIC_ID
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                    // A long thread name will cut to 15 characters in debug tools.
                    // Such as "top", "htop", "gdb" and so on.
                    // It's a kernel limit.
                    //
                    // So if we want to see the whole name in debug tools,
                    // this number should have 6 digits at most,
                    // since the prefix uses 9 characters in below code.
                    //
                    // There still has a issue:
                    // When id wraps around, we couldn't know whether the old id
                    // is released or not.
                    // But we can ignore this, because it's almost impossible.
                    if n >= 999_999 {
                        Some(0)
                    } else {
                        Some(n + 1)
                    }
                })
                .expect("impossible since the above closure must return Some(number)");
            format!("GlobalRt-{id}")
        })
        .build()
        .expect("ckb runtime initialized");

    let handle = runtime.handle().clone();

    let (tx, rx) = oneshot::channel();
    let thread = thread::Builder::new()
        .name("GlobalRtBuilder".to_string())
        .spawn(move || {
            let ret = runtime.block_on(rx);
            runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
            ckb_logger::debug!("global runtime finish {:?}", ret);
        })
        .expect("tokio runtime started");

    (
        Handle { inner: handle },
        StopHandler::new(SignalSender::Tokio(tx), Some(thread), "GT".to_string()),
    )
}

impl Spawn for Handle {
    fn spawn_task<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn(future);
    }
}
