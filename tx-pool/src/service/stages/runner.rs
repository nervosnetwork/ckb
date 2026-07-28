//! Generic worker runner for tx-pool pipeline stages.
//!
//! `WorkerRunner` encapsulates the scheduling skeleton shared by the verify
//! workers and the ordered resolver: wait on `ChunkCommand` changes / queue
//! notifications, pop one job at a time, and process it to completion.
//! Stage-specific logic is provided by implementing [`JobHandler`].

use ckb_logger::{debug, error};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{Notify, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ContinuationMode {
    Permit,
    Final,
}

/// Stage-specific callbacks used by [`WorkerRunner`].
///
/// All methods return `impl Future + Send` explicitly so the runner can be
/// spawned on a multi-threaded Tokio runtime without requiring the
/// `async-trait` crate.
pub(crate) trait JobHandler: Clone + Send + Sync + 'static {
    /// A single item popped from the queue.
    type Job: Send;

    /// Human-readable name used only for debug logging.
    fn worker_name(&self) -> &'static str;

    /// Subscribe to the queue's readiness notification.
    fn queue_ready(&self) -> impl Future<Output = Arc<Notify>> + Send;

    /// Pop one job from the queue.
    ///
    /// Returning `None` means the worker should go back to waiting.
    fn pop_one(&mut self) -> impl Future<Output = Option<Self::Job>> + Send;

    /// Process a single popped job and optionally return a same-lane lease
    /// already checked out by the completion transition.
    fn process_one(&mut self, job: Self::Job) -> impl Future<Output = Option<Self::Job>> + Send;

    /// Finish one lease without checking out another. The separate return type
    /// makes it impossible for pause/cancel drain logic to drop a continuation.
    fn process_final(&mut self, job: Self::Job) -> impl Future<Output = ()> + Send;

    /// The next time this stage has time-based work (e.g. a delayed job
    /// becoming due). The default returns `None`, meaning the stage only
    /// reacts to queue notifications and command changes.
    fn next_deadline(&self) -> impl Future<Output = Option<tokio::time::Instant>> + Send {
        std::future::ready(None)
    }
}

/// Shared worker scheduling skeleton.
pub(crate) struct WorkerRunner<H: JobHandler> {
    handler: H,
    command_rx: watch::Receiver<ChunkCommand>,
    exit_signal: CancellationToken,
    status: ChunkCommand,
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

    /// Run one worker until cancellation or command-authority loss.  Panics
    /// from kernel bookkeeping are invariant failures and are intentionally
    /// not converted into an in-process restart protocol.
    pub(crate) async fn run(mut self, worker_id: usize) {
        self.run_loop().await;
        if !self.exit_signal.is_cancelled() {
            error!(
                "tx-pool {} {worker_id} stopped because its command channel closed",
                self.handler.worker_name()
            );
        }
    }

    async fn run_loop(&mut self) {
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
                        // rather than turning the select loop into a hot spin.
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
            let Some(mut job) = self.handler.pop_one().await else {
                return;
            };
            loop {
                let Some(next) = self.handler.process_one(job).await else {
                    break;
                };
                let command_closed = self.command_rx.has_changed().is_err();
                self.refresh_status();
                if command_closed
                    || self.exit_signal.is_cancelled()
                    || self.status != ChunkCommand::Resume
                {
                    self.handler.process_final(next).await;
                    return;
                }
                job = next;
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/stage_runner.rs"]
mod tests;
