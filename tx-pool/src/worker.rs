//! Generic worker runner for tx-pool pipeline stages.
//!
//! `WorkerRunner` encapsulates the scheduling skeleton shared by the verify
//! workers and the ordered resolver: wait on `ChunkCommand` changes / queue
//! notifications, pop one job at a time, and process it to completion.
//! Stage-specific logic is provided by implementing [`JobHandler`].

use ckb_logger::{debug, error};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use futures_util::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::util::panic_payload_to_string;

/// Run a single job's future, catching any panic it raises.
///
/// Returns `Err(payload)` when the job panicked. Pipeline workers wrap every
/// popped job in this guard so that one bad job can neither kill the whole
/// worker (losing every cleanup the job still needed) nor strand its
/// bookkeeping markers (`active` sets, RBF registrations) forever. The
/// worker-level respawn monitors remain as the backstop for panics
/// *outside* the per-job guard (queue pop, scheduling).
pub(crate) async fn catch_job_panic<F: Future<Output = ()>>(fut: F) -> Result<(), String> {
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(()) => Ok(()),
        Err(payload) => Err(panic_payload_to_string(payload.as_ref())),
    }
}

/// Run the future produced by `make`, retrying exactly once after a panic.
///
/// `make` is called per attempt so each attempt gets fresh inputs. The
/// first panic is logged before the retry; the returned `Err` carries the
/// final (second) panic payload when both attempts fail. Only use this for
/// work that is *idempotent* — re-running it after a partial first attempt
/// must be safe.
pub(crate) async fn run_with_one_retry<F, Make>(mut make: Make) -> Result<(), String>
where
    F: Future<Output = ()>,
    Make: FnMut() -> F,
{
    match catch_job_panic(make()).await {
        Ok(()) => Ok(()),
        Err(first) => {
            error!("job panicked: {}; retrying once (idempotent work)", first);
            catch_job_panic(make()).await
        }
    }
}

/// Outcome of a worker run loop.
pub(crate) enum WorkerOutcome {
    /// The worker exited cleanly because the cancellation token fired or the
    /// command channel was dropped.
    Stopped,
    /// The worker panicked while processing a job.
    Panicked(String),
}

/// Stage-specific callbacks used by [`WorkerRunner`].
///
/// All methods return `impl Future + Send` explicitly so the runner can be
/// spawned on a multi-threaded Tokio runtime without requiring the
/// `async-trait` crate.
pub(crate) trait JobHandler: Clone + Send + Sync + 'static {
    /// A single item popped from the queue.
    type Job: Send;

    /// Payload sent back to the monitor loop when the worker exits.
    type Exit: Send + std::fmt::Debug;

    /// Human-readable name used only for debug logging.
    fn worker_name(&self) -> &'static str;

    /// Cheap check to avoid locking the queue when it is empty.
    fn is_queue_empty(&self) -> impl Future<Output = bool> + Send;

    /// Subscribe to the queue's readiness notification.
    fn queue_ready(&self) -> impl Future<Output = Arc<Notify>> + Send;

    /// Pop one job from the queue.
    ///
    /// Returning `None` means the worker should go back to waiting.
    fn pop_one(&mut self) -> impl Future<Output = Option<Self::Job>> + Send;

    /// Process a single popped job to completion.
    fn process_one(&mut self, job: Self::Job) -> impl Future<Output = ()> + Send;

    /// The next time this stage has time-based work (e.g. a delayed job
    /// becoming due). The default returns `None`, meaning the stage only
    /// reacts to queue notifications and command changes.
    fn next_deadline(&self) -> impl Future<Output = Option<tokio::time::Instant>> + Send {
        std::future::ready(None)
    }

    /// Build the exit payload reported to the monitor loop.
    fn make_exit(&self, outcome: WorkerOutcome) -> Self::Exit;
}

/// Shared worker scheduling skeleton.
pub(crate) struct WorkerRunner<H: JobHandler> {
    handler: H,
    command_rx: watch::Receiver<ChunkCommand>,
    exit_signal: CancellationToken,
    status: ChunkCommand,
}

impl<H: JobHandler> Clone for WorkerRunner<H> {
    fn clone(&self) -> Self {
        Self {
            handler: self.handler.clone(),
            command_rx: self.command_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            status: self.status.clone(),
        }
    }
}

impl<H: JobHandler> WorkerRunner<H> {
    pub(crate) fn new(
        handler: H,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        Self {
            handler,
            command_rx,
            exit_signal,
            status: ChunkCommand::Resume,
        }
    }

    /// Spawn the worker and report its exit via `exit_tx`.
    pub(crate) fn start(
        self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, H::Exit)>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut runner = self;
            let outcome = match AssertUnwindSafe(runner.run()).catch_unwind().await {
                Ok(()) => WorkerOutcome::Stopped,
                Err(payload) => WorkerOutcome::Panicked(panic_payload_to_string(payload.as_ref())),
            };
            let exit = runner.handler.make_exit(outcome);
            if let Err(err) = exit_tx.send((worker_id, exit)) {
                error!("failed to notify tx-pool worker exit: {:?}", err.0);
            }
        })
    }

    async fn run(&mut self) {
        let queue_ready = self.handler.queue_ready().await;
        self.refresh_status();
        loop {
            let deadline = self.handler.next_deadline().await;
            // Recomputed every loop iteration: the deadline future is dropped
            // (cancel-safe) whenever another branch wins first.
            let deadline_fired = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = self.exit_signal.cancelled() => break,
                changed = self.command_rx.changed() => {
                    if changed.is_err() {
                        // The command sender was dropped: no further
                        // commands can arrive, and `changed()` would
                        // resolve immediately forever, spinning the loop
                        // at 100% CPU. Channel drop means a clean stop
                        // (see `WorkerOutcome::Stopped`).
                        break;
                    }
                    self.status = self.command_rx.borrow_and_update().to_owned();
                    self.process_loop().await;
                }
                _ = queue_ready.notified() => self.process_loop().await,
                _ = deadline_fired => self.process_loop().await,
            }
        }
    }

    fn refresh_status(&mut self) {
        self.status = self.command_rx.borrow().to_owned();
    }

    async fn process_loop(&mut self) {
        loop {
            if self.exit_signal.is_cancelled() {
                debug!("{} process_loop cancelled", self.handler.worker_name());
                return;
            }
            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }
            if self.handler.is_queue_empty().await {
                return;
            }
            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }
            let Some(job) = self.handler.pop_one().await else {
                return;
            };
            self.handler.process_one(job).await;
        }
    }
}

#[cfg(test)]
mod tests {
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

    #[tokio::test]
    async fn run_with_one_retry_retries_once_then_gives_up() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // First attempt panics, second succeeds.
        let attempts = AtomicUsize::new(0);
        let result = run_with_one_retry(|| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!(attempt > 0, "first attempt panics deterministically");
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);

        // Both attempts panic: bounded to exactly two, error returned.
        let attempts = AtomicUsize::new(0);
        let result = run_with_one_retry(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { panic!("always panics") }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
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
}
