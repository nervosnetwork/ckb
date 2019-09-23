use futures::Future;
use std::thread;
use tokio::runtime::{Builder, TaskExecutor};
use tokio_executor::enter;

pub type Executor = TaskExecutor;

pub fn new_executor<F, R>(block: F) -> (Executor, thread::JoinHandle<()>)
where
    F: FnOnce(Executor) -> R + Send + 'static,
    R: Future<Item = (), Error = ()> + Send + 'static,
{
    let (tx, rx) = crossbeam_channel::bounded(1);
    let handler = thread::Builder::new()
        .spawn(move || {
            let mut entered = enter().expect("nested tokio::run");
            let mut runtime = Builder::new()
                .core_threads(num_cpus::get())
                .name_prefix("GlobalRuntime-")
                .build()
                .expect("Global tokio runtime init");

            let executor = runtime.executor();
            let future = block(executor.clone());
            tx.send(executor).expect("Send global tokio runtime");

            runtime.spawn(future);
            entered
                .block_on(runtime.shutdown_on_idle())
                .expect("shutdown cannot error")
        })
        .expect("Start Global tokio runtime");
    let executor = rx.recv().expect("Recv global tokio runtime");
    (executor, handler)
}
