//! Utilities for tokio runtime.

use ckb_logger::debug;
use ckb_stop_handler::{SignalSender, StopHandler};
use std::thread;
use tokio::sync::oneshot;

use tokio::runtime::Builder;
pub use tokio::runtime::Handle;

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
        handle,
        StopHandler::new(SignalSender::Tokio(tx), Some(thread)),
    )
}
