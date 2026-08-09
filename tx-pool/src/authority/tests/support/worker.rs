use super::*;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

const TEST_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityTestWorkerCommandError {
    NotOwned,
    Closed,
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityTestWorkerShutdownError {
    Timeout(AuthorityWorkerRole),
    Join {
        role: AuthorityWorkerRole,
        error: tokio::task::JoinError,
    },
    Worker {
        role: AuthorityWorkerRole,
        error: AuthorityWorkerFault,
    },
}

impl std::fmt::Display for AuthorityTestWorkerShutdownError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout(role) => write!(formatter, "{role:?} worker shutdown timed out"),
            Self::Join { role, error } => {
                write!(formatter, "{role:?} worker join failed: {error:?}")
            }
            Self::Worker { role, error } => {
                write!(formatter, "{role:?} worker returned a fault: {error:?}")
            }
        }
    }
}

/// Test-only structured owner for one authority worker generation.
///
/// Normal completion consumes every join handle. Unwind or early return runs
/// `Drop`, which closes the command/cancellation roots and aborts every still-
/// owned task instead of detaching it from a discarded `JoinHandle`.
pub(in crate::authority) struct AuthorityTestWorkerOwner {
    command: Option<watch::Sender<ChunkCommand>>,
    cancel: CancellationToken,
    tasks: Vec<AuthorityWorkerTask>,
}

impl AuthorityTestWorkerOwner {
    pub(in crate::authority) fn spawn_set(
        runtime: AuthorityRuntime,
        handle: &Handle,
        cache: Arc<RwLock<TxVerificationCache>>,
        cache_updates: mpsc::Sender<VerificationCacheUpdate>,
        initial_command: ChunkCommand,
    ) -> Result<Self, AuthorityWorkerSpawnError> {
        let (command, command_rx) = watch::channel(initial_command);
        let cancel = CancellationToken::new();
        let handles =
            runtime.spawn_workers(handle, cache, cache_updates, command_rx, cancel.clone())?;
        Ok(Self {
            command: Some(command),
            cancel,
            tasks: handles.tasks,
        })
    }

    pub(in crate::authority) fn spawn_maintenance(
        runtime: AuthorityRuntime,
        handle: &Handle,
    ) -> Result<Self, AuthorityWorkerSpawnError> {
        let mut tasks = Vec::new();
        tasks
            .try_reserve(1)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.child_token();
        let task = AuthorityWorkerTask {
            role: AuthorityWorkerRole::Maintenance,
            handle: handle.spawn(async move { run_maintenance_driver(runtime, task_cancel).await }),
        };
        tasks.push(task);
        Ok(Self {
            command: None,
            cancel,
            tasks,
        })
    }

    pub(in crate::authority) fn spawn_observed_maintenance(
        runtime: AuthorityRuntime,
        handle: &Handle,
        rounds: Arc<AtomicUsize>,
    ) -> Result<Self, AuthorityWorkerSpawnError> {
        let mut tasks = Vec::new();
        tasks
            .try_reserve(1)
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.child_token();
        let task = AuthorityWorkerTask {
            role: AuthorityWorkerRole::Maintenance,
            handle: handle.spawn(async move {
                run_maintenance_driver_for_foundation(runtime, task_cancel, rounds).await
            }),
        };
        tasks.push(task);
        Ok(Self {
            command: None,
            cancel,
            tasks,
        })
    }

    pub(in crate::authority) fn send(
        &self,
        command: ChunkCommand,
    ) -> Result<(), AuthorityTestWorkerCommandError> {
        self.command
            .as_ref()
            .ok_or(AuthorityTestWorkerCommandError::NotOwned)?
            .send(command)
            .map_err(|_| AuthorityTestWorkerCommandError::Closed)
    }

    pub(in crate::authority) fn role_count(&self, role: AuthorityWorkerRole) -> usize {
        self.tasks.iter().filter(|task| task.role == role).count()
    }

    pub(in crate::authority) fn verifier_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| matches!(task.role, AuthorityWorkerRole::Verifier(_)))
            .count()
    }

    pub(in crate::authority) fn abort_handles(
        &self,
    ) -> Result<Vec<tokio::task::AbortHandle>, AuthorityWorkerSpawnError> {
        let mut handles = Vec::new();
        handles
            .try_reserve(self.tasks.len())
            .map_err(|_| AuthorityWorkerSpawnError::Allocation)?;
        handles.extend(self.tasks.iter().map(|task| task.handle.abort_handle()));
        Ok(handles)
    }

    pub(in crate::authority) async fn shutdown(
        mut self,
    ) -> Result<(), AuthorityTestWorkerShutdownError> {
        self.request_stop();
        let mut first_error = None;
        for task in self.tasks.iter_mut().rev() {
            let role = task.role;
            match tokio::time::timeout(TEST_WORKER_SHUTDOWN_TIMEOUT, &mut task.handle).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => {
                    first_error
                        .get_or_insert(AuthorityTestWorkerShutdownError::Worker { role, error });
                }
                Ok(Err(error)) => {
                    first_error
                        .get_or_insert(AuthorityTestWorkerShutdownError::Join { role, error });
                }
                Err(_) => {
                    task.handle.abort();
                    drop((&mut task.handle).await);
                    first_error.get_or_insert(AuthorityTestWorkerShutdownError::Timeout(role));
                }
            }
        }
        self.tasks.clear();
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn request_stop(&self) {
        if let Some(command) = &self.command {
            let _ = command.send(ChunkCommand::Stop);
        }
        self.cancel.cancel();
    }
}

impl Drop for AuthorityTestWorkerOwner {
    fn drop(&mut self) {
        self.request_stop();
        for task in &self.tasks {
            task.handle.abort();
        }
    }
}

pub(in crate::authority) async fn run_maintenance_driver_for_foundation(
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
    rounds: Arc<AtomicUsize>,
) -> Result<(), AuthorityWorkerFault> {
    run_maintenance_driver_loop(runtime, cancel, move || {
        rounds.fetch_add(1, AtomicOrdering::Relaxed);
    })
    .await
}
