use super::*;

#[derive(Clone)]
struct NoopHandler;

impl JobHandler for NoopHandler {
    type Job = ();

    fn worker_name(&self) -> &'static str {
        "noop"
    }

    fn is_queue_empty(&self) -> impl Future<Output = bool> + Send {
        std::future::ready(true)
    }

    fn queue_ready(&self) -> impl Future<Output = Arc<Notify>> + Send {
        std::future::ready(Arc::new(Notify::new()))
    }

    fn pop_one(&mut self) -> impl Future<Output = Option<()>> + Send {
        std::future::ready(None)
    }

    fn process_one(&mut self, _job: Self::Job) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// A dropped command channel must stop the worker: `changed()` resolves
/// with `Err` immediately and forever, so treating it like a normal
/// wakeup would spin the select loop at 100% CPU.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_exits_when_command_channel_dropped() {
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let runner = WorkerRunner::new(NoopHandler, command_rx, CancellationToken::new());
    let handle = tokio::spawn(async move { runner.run(0).await });

    drop(command_tx);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("worker must stop when the command channel is dropped")
        .expect("worker task joins");
}
