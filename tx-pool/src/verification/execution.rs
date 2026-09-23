//! Owns interruptible VM work and charges only time spent inside the scheduler.

use crate::util::block_offload;
use ckb_script::{
    ChunkCommand, RunMode, Scheduler, SchedulerRunner,
    types::{DebugPrinter, Machine, TerminatedResult},
};
use ckb_snapshot::Snapshot;
use ckb_store::data_loader_wrapper::DataLoaderWrapper;
use ckb_types::core::Cycle;
use ckb_vm::{Error, machine::Pause};
use futures_util::FutureExt;
use std::time::{Duration, Instant};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

#[cfg(test)]
mod tests;

/// Pool computation leaves a runtime worker available for controls and timers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ComputeMode {
    Inline,
    YieldRuntimeWorker,
}

impl ComputeMode {
    pub(crate) fn run<T>(self, operation: impl FnOnce() -> T) -> T {
        match self {
            Self::Inline => operation(),
            Self::YieldRuntimeWorker => block_offload(operation),
        }
    }
}

pub(super) struct VmRunner<'a> {
    pub command: &'a mut watch::Receiver<ChunkCommand>,
    pub remaining: Duration,
    pub mode: ComputeMode,
}

impl SchedulerRunner<Scheduler<DataLoaderWrapper<Snapshot>, DebugPrinter, Machine>>
    for VmRunner<'_>
{
    async fn run(
        &mut self,
        mut scheduler: Scheduler<DataLoaderWrapper<Snapshot>, DebugPrinter, Machine>,
        max_cycles: Cycle,
    ) -> Result<Option<TerminatedResult>, Error> {
        self.run_vm(move |pause| {
            let remaining = max_cycles
                .checked_sub(scheduler.consumed_cycles())
                .ok_or(Error::CyclesExceeded)?;
            scheduler.run(RunMode::Pause(pause, remaining))
        })
        .await
    }
}

impl VmRunner<'_> {
    /// A slice returns the scheduler only after it stops. Joining it both
    /// acknowledges suspension and accounts for its active time, before any resume.
    async fn run_vm(
        &mut self,
        mut execute: impl FnMut(Pause) -> Result<TerminatedResult, Error> + Send + 'static,
    ) -> Result<Option<TerminatedResult>, Error> {
        let mut desired = self.command.borrow_and_update().clone();
        loop {
            if self.remaining.is_zero() {
                return Ok(None);
            }
            while desired != ChunkCommand::Resume {
                if desired == ChunkCommand::Stop {
                    return Err(Error::External("stopped".into()));
                }
                desired = self
                    .command
                    .changed()
                    .await
                    .map_or(ChunkCommand::Stop, |_| {
                        self.command.borrow_and_update().clone()
                    });
            }

            let (returned, result, elapsed) = self.run_slice(execute, &mut desired).await?;
            execute = returned;
            self.remaining = self.remaining.saturating_sub(elapsed);
            if self.remaining.is_zero() {
                return Ok(None);
            }
            if !matches!(result, Err(Error::Pause)) {
                return result.map(Some);
            }
        }
    }

    /// Own one execution slice through interruption and join. Its returned time
    /// covers only the child execution, and its deadline cannot affect a resume.
    async fn run_slice<F>(
        &mut self,
        mut execute: F,
        desired: &mut ChunkCommand,
    ) -> Result<(F, Result<TerminatedResult, Error>, Duration), Error>
    where
        F: FnMut(Pause) -> Result<TerminatedResult, Error> + Send + 'static,
    {
        let limit = self.remaining;
        let mode = self.mode;
        let pause = Pause::new();
        let child_pause = pause.clone();
        let (started, start) = oneshot::channel();
        let mut child = VmTask {
            pause,
            handle: tokio::spawn(async move {
                let now = Instant::now();
                let _ = started.send(now);
                let result = mode.run(|| execute(child_pause));
                (execute, result, now.elapsed())
            }),
        };
        // The deadline starts inside the task, excluding time in its queue.
        let deadline = async move {
            match start
                .await
                .ok()
                .and_then(|started| started.checked_add(limit))
            {
                Some(deadline) => tokio::time::sleep_until(deadline.into()).await,
                None => std::future::pending().await,
            }
        }
        .fuse();
        tokio::pin!(deadline);
        let completed = loop {
            tokio::select! {
                biased;
                result = &mut child.handle => break result,
                _ = &mut deadline => {
                    child.pause.interrupt();
                }
                changed = self.command.changed(), if *desired != ChunkCommand::Stop => {
                    *desired = changed.map_or(ChunkCommand::Stop, |_| {
                        self.command.borrow_and_update().clone()
                    });
                    if *desired != ChunkCommand::Resume {
                        child.pause.interrupt();
                    }
                }
            }
        };
        match completed {
            Ok(completed) => Ok(completed),
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(_) => Err(Error::External("stopped".into())),
        }
    }
}

/// Dropping the caller interrupts and cancels the slice it owns.
struct VmTask<T> {
    pause: Pause,
    handle: JoinHandle<T>,
}

impl<T> Drop for VmTask<T> {
    fn drop(&mut self) {
        self.pause.interrupt();
        self.handle.abort();
    }
}
