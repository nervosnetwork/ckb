use ckb_stop_handler::{SignalSender, StopHandler};
use once_cell::sync::OnceCell;
use std::{future::Future, sync, thread};
use tokio::sync::oneshot;

pub use tokio::runtime::{Builder, Handle};

pub static GLOBAL_RUNTIME_HANDLE: OnceCell<Handle> = OnceCell::new();

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

pub fn new_global_runtime() -> StopHandler<()> {
    let mut runtime = Builder::new()
        .threaded_scheduler()
        .thread_name("ckb-global-runtime")
        .build()
        .expect("ckb runtime initialized");

    let handle = runtime.handle().clone();

    GLOBAL_RUNTIME_HANDLE
        .set(handle)
        .expect("GLOBAL_RUNTIME_HANDLE init once");

    let (tx, rx) = oneshot::channel();
    let thread = thread::Builder::new()
        .name("ckb-global-runtime-tb".to_string())
        .spawn(move || {
            runtime.block_on(rx).expect("ckb-global-runtime block on");
        })
        .expect("tokio runtime started");

    StopHandler::new(SignalSender::Tokio(tx), Some(thread))
}

pub fn global_handle() -> &'static Handle {
    GLOBAL_RUNTIME_HANDLE
        .get()
        .expect("GLOBAL_RUNTIME_HANDLE initialized")
}
