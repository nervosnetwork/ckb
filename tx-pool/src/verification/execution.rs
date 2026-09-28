//! Owns interruptible VM work and charges execution-thread CPU time.

use super::cpu_clock::CpuTime;
use crate::util::block_offload;
use ckb_script::{
    ChunkCommand, RunMode, Scheduler, SchedulerRunner, VM_INTERRUPTED_MESSAGE,
    types::{DebugPrinter, Machine, TerminatedResult},
};
use ckb_snapshot::Snapshot;
use ckb_store::data_loader_wrapper::DataLoaderWrapper;
use ckb_types::core::Cycle;
use ckb_vm::{Error, machine::Pause};
use futures_util::FutureExt;
use std::{future::Future, io, time::Duration};
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
    /// acknowledges suspension and accounts for its CPU time, before any resume.
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
                    return Err(Error::External(VM_INTERRUPTED_MESSAGE.into()));
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
            let elapsed = match elapsed {
                Ok(elapsed) => elapsed,
                Err(error) => {
                    // This runner cannot safely grant another group or resume
                    // after losing its CPU receipt, even if failure is final.
                    self.remaining = Duration::ZERO;
                    return unmeasured_result(result, error).map(Some);
                }
            };
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
    /// comes from the child thread, and its monitor cannot affect a resume.
    async fn run_slice<F>(
        &mut self,
        mut execute: F,
        desired: &mut ChunkCommand,
    ) -> Result<(F, Result<TerminatedResult, Error>, io::Result<Duration>), Error>
    where
        F: FnMut(Pause) -> Result<TerminatedResult, Error> + Send + 'static,
    {
        let limit = self.remaining;
        let mode = self.mode;
        let pause = Pause::new();
        let child_pause = pause.clone();
        let (started, start) = oneshot::channel();
        let child = VmTask {
            pause,
            handle: tokio::spawn(async move {
                let measured = mode.run(|| {
                    let (clock, monitor) = CpuTime::start_monitored().map_err(clock_failure)?;
                    let _ = started.send(monitor);
                    let result = execute(child_pause);
                    Ok((result, clock.elapsed()))
                });
                measured.map(|(result, elapsed)| (execute, result, elapsed))
            }),
        };
        // Off-CPU waiting may postpone checks, but cannot itself request Pause.
        // This future survives command changes and completes at most once.
        let monitor = async move {
            match start.await {
                Ok(monitor) => wait_for_cpu_budget(limit, || monitor.observed()).await,
                Err(_) => std::future::pending().await,
            }
        };
        child.join(self.command, desired, monitor).await?
    }
}

/// Keep determined failures available for canonical attribution. An unmeasured
/// success or Pause is a local interruption, never a new script error.
fn unmeasured_result(
    result: Result<TerminatedResult, Error>,
    error: io::Error,
) -> Result<TerminatedResult, Error> {
    let interrupted = clock_failure(error);
    match result {
        Ok(result) if result.exit_code != 0 => Ok(result),
        Err(error) if error != Error::Pause => Err(error),
        _ => Err(interrupted),
    }
}

fn clock_failure(error: io::Error) -> Error {
    ckb_logger::warn!("VM CPU measurement failed: {error}");
    Error::External(VM_INTERRUPTED_MESSAGE.into())
}

/// Wait for observed CPU exhaustion, without repeatedly pausing off-CPU work.
/// The 1 ms floor follows Tokio's timer granularity and bounds polling when a
/// thread stops making progress just short of its limit.
async fn wait_for_cpu_budget(
    limit: Duration,
    mut elapsed: impl FnMut() -> std::io::Result<Duration>,
) -> std::io::Result<()> {
    loop {
        let remaining = limit.saturating_sub(elapsed()?);
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.max(Duration::from_millis(1))).await;
    }
}

/// Dropping the caller interrupts and cancels the slice it owns.
struct VmTask<T> {
    pause: Pause,
    handle: JoinHandle<T>,
}

impl<T> VmTask<T> {
    /// Observe controls and the CPU monitor until this owned task has joined.
    /// Desired commands survive the slice; late clock observations do not.
    async fn join(
        mut self,
        command: &mut watch::Receiver<ChunkCommand>,
        desired: &mut ChunkCommand,
        monitor: impl Future<Output = io::Result<()>>,
    ) -> Result<T, Error> {
        let monitor = monitor.fuse();
        tokio::pin!(monitor);
        let completed = loop {
            tokio::select! {
                biased;
                result = &mut self.handle => break result,
                observed = &mut monitor => {
                    if let Err(error) = observed {
                        // Completion can occur after the first join poll. A
                        // failed read of the old thread must not stop a resume.
                        if let Some(result) = (&mut self.handle).now_or_never() {
                            break result;
                        }
                        ckb_logger::warn!("VM CPU monitoring failed: {error}");
                        *desired = ChunkCommand::Stop;
                    }
                    self.pause.interrupt();
                }
                changed = command.changed(), if *desired != ChunkCommand::Stop => {
                    *desired = changed.map_or(ChunkCommand::Stop, |_| {
                        command.borrow_and_update().clone()
                    });
                    if *desired != ChunkCommand::Resume {
                        self.pause.interrupt();
                    }
                }
            }
        };
        match completed {
            Ok(completed) => Ok(completed),
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(_) => Err(Error::External(VM_INTERRUPTED_MESSAGE.into())),
        }
    }
}

impl<T> Drop for VmTask<T> {
    fn drop(&mut self) {
        self.pause.interrupt();
        self.handle.abort();
    }
}
