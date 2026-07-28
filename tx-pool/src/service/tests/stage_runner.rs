use super::*;
use std::sync::Mutex;

#[derive(Clone)]
struct NoopHandler;

impl JobHandler for NoopHandler {
    type Job = ();

    fn worker_name(&self) -> &'static str {
        "noop"
    }

    fn queue_ready(&self) -> impl Future<Output = Arc<Notify>> + Send {
        std::future::ready(Arc::new(Notify::new()))
    }

    fn pop_one(&mut self) -> impl Future<Output = Option<()>> + Send {
        std::future::ready(None)
    }

    fn process_one(&mut self, _job: Self::Job) -> impl Future<Output = Option<()>> + Send {
        std::future::ready(None)
    }

    fn process_final(&mut self, _job: Self::Job) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

#[derive(Clone)]
struct InterruptAfterContinuationHandler {
    ready: Arc<Notify>,
    command_tx: Option<watch::Sender<ChunkCommand>>,
    cancel: CancellationToken,
    action: InterruptAction,
    calls: Arc<Mutex<Vec<(u8, ContinuationMode)>>>,
    final_processed: Arc<Notify>,
    queued: bool,
}

#[derive(Clone, Copy)]
enum InterruptAction {
    Suspend,
    Close,
    Cancel,
}

impl JobHandler for InterruptAfterContinuationHandler {
    type Job = u8;

    fn worker_name(&self) -> &'static str {
        "continuation test"
    }

    fn queue_ready(&self) -> impl Future<Output = Arc<Notify>> + Send {
        let ready = Arc::clone(&self.ready);
        ready.notify_one();
        std::future::ready(ready)
    }

    fn pop_one(&mut self) -> impl Future<Output = Option<u8>> + Send {
        let job = self.queued.then_some(1);
        self.queued = false;
        std::future::ready(job)
    }

    async fn process_one(&mut self, job: u8) -> Option<u8> {
        self.calls
            .lock()
            .expect("test call log lock")
            .push((job, ContinuationMode::Permit));
        match self.action {
            InterruptAction::Suspend => {
                if let Some(command_tx) = &self.command_tx {
                    command_tx.send_replace(ChunkCommand::Suspend);
                }
            }
            InterruptAction::Close => {
                self.command_tx.take();
            }
            InterruptAction::Cancel => self.cancel.cancel(),
        }
        Some(2)
    }

    async fn process_final(&mut self, job: u8) {
        self.calls
            .lock()
            .expect("test call log lock")
            .push((job, ContinuationMode::Final));
        self.final_processed.notify_one();
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

async fn run_interrupted_continuation(action: InterruptAction) -> Vec<(u8, ContinuationMode)> {
    let (command_tx, command_rx) = watch::channel(ChunkCommand::Resume);
    let cancel = CancellationToken::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let final_processed = Arc::new(Notify::new());
    let runner = WorkerRunner::new(
        InterruptAfterContinuationHandler {
            ready: Arc::new(Notify::new()),
            command_tx: Some(command_tx),
            cancel: cancel.clone(),
            action,
            calls: Arc::clone(&calls),
            final_processed: Arc::clone(&final_processed),
            queued: true,
        },
        command_rx,
        cancel.clone(),
    );
    let handle = tokio::spawn(async move { runner.run(0).await });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        final_processed.notified(),
    )
    .await
    .expect("the already checked-out continuation must be completed");

    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("worker must stop after cancellation")
        .expect("worker task joins");
    calls.lock().expect("test call log lock").clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_finishes_one_checked_out_continuation_without_dropping_it() {
    assert_eq!(
        run_interrupted_continuation(InterruptAction::Suspend).await,
        vec![(1, ContinuationMode::Permit), (2, ContinuationMode::Final)]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_close_finishes_one_checked_out_continuation_then_exits() {
    assert_eq!(
        run_interrupted_continuation(InterruptAction::Close).await,
        vec![(1, ContinuationMode::Permit), (2, ContinuationMode::Final)]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_finishes_one_checked_out_continuation_without_dropping_it() {
    assert_eq!(
        run_interrupted_continuation(InterruptAction::Cancel).await,
        vec![(1, ContinuationMode::Permit), (2, ContinuationMode::Final)]
    );
}
