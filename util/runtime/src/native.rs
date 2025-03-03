use ckb_spawn::Spawn;
use core::future::Future;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::available_parallelism;
use tokio::runtime::{Builder, Handle as TokioHandle, Runtime};

use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;

// Handle is a newtype wrap and unwrap tokio::Handle, it is workaround with Rust Orphan Rules.
// We need `Handle` impl ckb spawn trait decouple tokio dependence

/// Handle to the runtime.
#[derive(Debug, Clone)]
pub struct Handle {
    pub(crate) inner: TokioHandle,
    guard: Option<Sender<()>>,
}

impl Handle {
    /// Create a new Handle
    pub fn new(inner: TokioHandle, guard: Option<Sender<()>>) -> Self {
        Self { inner, guard }
    }

    /// Drop the guard
    pub fn drop_guard(&mut self) {
        let _ = self.guard.take();
    }
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
        let tokio_task_guard = self.guard.clone();

        self.inner.spawn(async move {
            // move tokio_task_guard into the spawned future
            // so that it will be dropped when the future is finished
            let _guard = tokio_task_guard;
            future.await
        })
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

/// Create a new runtime with unique name.
fn new_runtime(worker_num: Option<usize>) -> Runtime {
    Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_num.unwrap_or_else(|| available_parallelism().unwrap().into()))
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
                    if n >= 999_999 { Some(0) } else { Some(n + 1) }
                })
                .expect("impossible since the above closure must return Some(number)");
            format!("GlobalRt-{id}")
        })
        .build()
        .expect("ckb runtime initialized")
}

/// Create new threaded_scheduler tokio Runtime, return `Runtime`
pub fn new_global_runtime(worker_num: Option<usize>) -> (Handle, Receiver<()>, Runtime) {
    let runtime = new_runtime(worker_num);
    let handle = runtime.handle().clone();
    let (guard, handle_stop_rx): (Sender<()>, Receiver<()>) = tokio::sync::mpsc::channel::<()>(1);

    (Handle::new(handle, Some(guard)), handle_stop_rx, runtime)
}

/// Create new threaded_scheduler tokio Runtime, return `Handle` and background thread join handle,
/// NOTICE: This is only used in testing
pub fn new_background_runtime() -> Handle {
    let runtime = new_runtime(None);
    let handle = runtime.handle().clone();

    let (guard, mut handle_stop_rx): (Sender<()>, Receiver<()>) =
        tokio::sync::mpsc::channel::<()>(1);
    let _thread = std::thread::Builder::new()
        .name("GlobalRtBuilder".to_string())
        .spawn(move || {
            let ret = runtime.block_on(async move { handle_stop_rx.recv().await });
            ckb_logger::debug!("Global runtime finished {:?}", ret);
        })
        .expect("tokio runtime started");

    Handle::new(handle, Some(guard))
}

impl Spawn for Handle {
    fn spawn_task<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn(future);
    }
}
