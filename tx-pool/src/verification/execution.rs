//! Owns interruptible VM work and charges execution-thread CPU time.

use super::cpu_clock::CpuTime;
use crate::{authority::service::ComputePool, util::block_offload};
use ckb_script::{
    ChunkCommand, RunMode, Scheduler, SchedulerRunner, VM_INTERRUPTED_MESSAGE,
    types::{DebugPrinter, Machine, TerminatedResult},
};
#[cfg(test)]
use ckb_snapshot::Snapshot;
#[cfg(test)]
use ckb_store::data_loader_wrapper::DataLoaderWrapper;
use ckb_types::core::Cycle;
use ckb_verification::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{Error, machine::Pause};
use futures_util::FutureExt;
use std::{
    future::Future,
    io,
    panic::AssertUnwindSafe,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

#[cfg(test)]
mod tests;

pub(super) struct VmRunner<'a> {
    pub command: &'a mut watch::Receiver<ChunkCommand>,
    pub remaining: Duration,
    pub compute: Option<&'a ComputePool>,
}

impl<DL> SchedulerRunner<Scheduler<DL, DebugPrinter, Machine>> for VmRunner<'_>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    async fn run(
        &mut self,
        mut scheduler: Scheduler<DL, DebugPrinter, Machine>,
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
            match result {
                // Determined consensus failures must always reach canonical
                // attribution (and rejection/peer-ban handling), even when
                // the slice simultaneously exhausted its CPU-time budget.
                Ok(result) if result.exit_code != 0 => return Ok(Some(result)),
                Err(error) if error != Error::Pause => return Err(error),
                Ok(result) => {
                    if self.remaining.is_zero() {
                        return Ok(None);
                    }
                    return Ok(Some(result));
                }
                Err(_) => {
                    if self.remaining.is_zero() {
                        return Ok(None);
                    }
                }
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
        let pause = Pause::new();
        let child_pause = pause.clone();
        let (started, start) = oneshot::channel();
        let handle = if let Some(compute) = self.compute {
            let (done_tx, done_rx) = oneshot::channel();
            let task = Box::new(move || {
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || {
                    if child_pause.has_interrupted() {
                        return Ok((execute, Err(Error::Pause), Ok(Duration::ZERO)));
                    }
                    let (clock, monitor) = CpuTime::start_monitored().map_err(clock_failure)?;
                    let _ = started.send(monitor);
                    let result = execute(child_pause);
                    let elapsed = clock.elapsed();
                    Ok((execute, result, elapsed))
                }));
                let _ = done_tx.send(outcome);
            });
            if let Err((_, task)) = compute.push_permitted(task) {
                drop(task);
                return Err(Error::External(VM_INTERRUPTED_MESSAGE.into()));
            }
            SliceHandle::Compute(done_rx)
        } else {
            SliceHandle::Tokio(tokio::spawn(async move {
                let measured = block_offload(|| {
                    let (clock, monitor) = CpuTime::start_monitored().map_err(clock_failure)?;
                    let _ = started.send(monitor);
                    let result = execute(child_pause);
                    Ok((result, clock.elapsed()))
                });
                measured.map(|(result, elapsed)| (execute, result, elapsed))
            }))
        };
        let child = VmTask { pause, handle };
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

enum SliceHandle<T> {
    Tokio(JoinHandle<T>),
    Compute(oneshot::Receiver<std::thread::Result<T>>),
}

impl<T> Future for SliceHandle<T> {
    type Output = Result<T, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Self::Tokio(handle) => Pin::new(handle).poll(cx).map(|completed| match completed {
                Ok(value) => Ok(value),
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(_) => Err(Error::External(VM_INTERRUPTED_MESSAGE.into())),
            }),
            Self::Compute(rx) => Pin::new(rx).poll(cx).map(|completed| match completed {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(payload)) => std::panic::resume_unwind(payload),
                Err(_) => Err(Error::External(VM_INTERRUPTED_MESSAGE.into())),
            }),
        }
    }
}

/// Dropping the caller interrupts and cancels the slice it owns.
struct VmTask<T> {
    pause: Pause,
    handle: SliceHandle<T>,
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
        loop {
            tokio::select! {
                biased;
                result = &mut self.handle => return result,
                observed = &mut monitor => {
                    if let Err(error) = observed {
                        // Completion can occur after the first join poll. A
                        // failed read of the old thread must not stop a resume.
                        if let Some(result) = (&mut self.handle).now_or_never() {
                            return result;
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
        }
    }
}

impl<T> Drop for VmTask<T> {
    fn drop(&mut self) {
        self.pause.interrupt();
        if let SliceHandle::Tokio(handle) = &self.handle {
            handle.abort();
        }
    }
}
