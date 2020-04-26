use std::future::Future;
use std::thread;
use tokio::runtime;
pub use tokio::runtime::Handle;

pub fn new_runtime<F, R>(block: F) -> (Handle, thread::JoinHandle<()>)
where
    F: FnOnce(Handle) -> R + Send + 'static,
    R: Future,
{
    let (tx, rx) = crossbeam_channel::bounded(1);
    let handler = thread::Builder::new()
        .spawn(move || {
            let mut runtime = runtime::Builder::new()
                .threaded_scheduler()
                .thread_name("GlobalRuntime-")
                .build()
                .expect("Global tokio runtime init");

            let handle = runtime.handle();
            let future = block(handle.clone());
            tx.send(handle.clone()).expect("Send global tokio runtime");

            runtime.block_on(future);
        })
        .expect("Start Global tokio runtime");
    let executor = rx.recv().expect("Recv global tokio runtime");
    (executor, handler)
}
