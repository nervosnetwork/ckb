//! Utilities for tokio runtime.

use ckb_logger::debug;
use ckb_stop_handler::{SignalSender, StopHandler};
use std::{future::Future, sync, thread};
use tokio::sync::oneshot;

pub use tokio::runtime::{Builder, Handle};

/// Creates a new tokio runtime.
pub fn new_runtime<F, R>(
    name_prefix: &str,
    runtime_builder_opt: Option<Builder>,
    block: F,
) -> (Handle, thread::JoinHandle<()>)
where
    F: FnOnce(Handle) -> R + Send + 'static,
    R: Future,
{
    let barrier = sync::Arc::new(sync::Barrier::new(2));
    let barrier_clone = sync::Arc::clone(&barrier);

    let service_name = format!("{}Service", name_prefix);
    let runtime_name = format!("{}Runtime", name_prefix);

    let mut runtime = runtime_builder_opt
        .unwrap_or_else(|| {
            let mut builder = Builder::new();
            builder.threaded_scheduler();
            builder
        })
        .thread_name(&runtime_name)
        .build()
        .unwrap_or_else(|_| panic!("tokio runtime {} initialized", runtime_name));

    let handle = runtime.handle().clone();
    let executor = handle.clone();

    let handler = thread::Builder::new()
        .name(service_name)
        .spawn(move || {
            let future = block(handle);
            barrier_clone.wait();
            runtime.block_on(future);
        })
        .unwrap_or_else(|_| panic!("tokio runtime {} started", runtime_name));

    barrier.wait();
    (executor, handler)
}

/// Create new threaded_scheduler tokio Runtime, return `Handle` and background thread join handle
pub fn new_global_runtime() -> (Handle, StopHandler<()>) {
    let mut runtime = Builder::new()
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
        handle,
        StopHandler::new(SignalSender::Tokio(tx), Some(thread)),
    )
}
