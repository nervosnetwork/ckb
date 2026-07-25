use super::*;

#[tokio::test]
async fn catch_job_panic_captures_panic_and_lets_work_continue() {
    let mut completed = Vec::new();

    let panicked = catch_job_panic(async {
        panic!("deterministic test panic");
    })
    .await;
    assert!(panicked.is_err());
    assert!(panicked.unwrap_err().contains("deterministic test panic"));

    // The worker can keep processing subsequent jobs after a panic.
    let ok = catch_job_panic(async {
        completed.push(1);
    })
    .await;
    assert!(ok.is_ok());
    assert_eq!(completed, vec![1]);
}

#[derive(Clone)]
struct NoopHandler;

impl JobHandler for NoopHandler {
    type Job = ();
    type Exit = ();

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

    fn make_exit(&self, _outcome: WorkerOutcome) -> Self::Exit {}
}

/// A dropped command channel must stop the worker: `changed()` resolves
/// with `Err` immediately and forever, so treating it like a normal
/// wakeup would spin the select loop at 100% CPU.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_exits_when_command_channel_dropped() {
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let runner = WorkerRunner::new(NoopHandler, command_rx, CancellationToken::new());
    let (exit_tx, mut exit_rx) = mpsc::unbounded_channel();
    let handle = runner.start(0, exit_tx);

    drop(command_tx);
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("worker must stop when the command channel is dropped")
        .expect("worker task joins");
    let (worker_id, ()) = exit_rx.recv().await.expect("exit is reported");
    assert_eq!(worker_id, 0);
}
