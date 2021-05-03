//! Utilities for tokio runtime.

use ckb_logger::debug;
use ckb_spawn::Spawn;
use ckb_stop_handler::{SignalSender, StopHandler};
use core::future::Future;
use std::thread;
use tokio::runtime::Builder;
use tokio::runtime::Handle as TokioHandle;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use tokio;

// Handle is a newtype wrap and unwrap tokio::Handle, it is workaround with Rust Orphan Rules.
// We need `Handle` impl ckb spawn trait decouple tokio dependence

/// Handle to the runtime.
#[derive(Debug, Clone)]
pub struct Handle {
    pub(crate) inner: TokioHandle,
}

impl Handle {
    /// Enter the runtime context. This allows you to construct types that must
    /// have an executor available on creation such as [`Delay`] or [`TcpStream`].
    /// It will also allow you to call methods such as [`tokio::spawn`].
    pub fn enter<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.inner.enter(f)
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

    /// Spawns a future onto the runtime blocking pool.
    ///
    /// This spawns the given future onto the runtime's executor
    pub fn spawn_blocking<F, R>(&self, f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.spawn_blocking(f)
    }

    /// Run a future to completion on the Tokio runtime from a synchronous context.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.inner.block_on(future)
    }
}

/// Create new threaded_scheduler tokio Runtime, return `Handle` and background thread join handle
pub fn new_global_runtime() -> (Handle, StopHandler<()>) {
    let mut runtime = Builder::new()
        .enable_all()
        .threaded_scheduler()
        .thread_name("ckb-global-runtime")
        .build()
        .expect("ckb runtime initialized");

    let handle = runtime.handle().clone();

    let (tx, rx) = oneshot::channel();
    let thread = thread::Builder::new()
        .name("ckb-global-runtime-tb".to_string())
        .spawn(move || {
            let ret = runtime.block_on(rx);
            debug!("global runtime finish {:?}", ret);
        })
        .expect("tokio runtime started");

    (
        Handle { inner: handle },
        StopHandler::new(SignalSender::Tokio(tx), Some(thread)),
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
