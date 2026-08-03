//! Structured task ownership for one unified-authority service generation.
//!
//! This owner is not another transaction state machine. It only proves that
//! every task and linear publication claim starts together, that normal
//! shutdown drains capabilities in order, and that task exits are classified
//! by the state they own instead of by a generic `is_finished` rule.

use super::{
    plan::EffectCloseError,
    publisher::{
        AuthorityEffectEndpoints, AuthorityEffectPublisherFault,
        run_claimed_authority_effect_publisher,
    },
    resolver::VerificationCacheUpdate,
    runtime::AuthorityRuntime,
    template_driver::{
        AuthorityBlockAssembler, AuthorityTemplateDriverFault, AuthorityTemplateRole,
        AuthorityTemplateTask,
    },
    worker::{
        AuthorityWorkerFault, AuthorityWorkerFaultKind, AuthorityWorkerRole,
        AuthorityWorkerSpawnError, AuthorityWorkerTask,
    },
};
use crate::constants::VERIFY_CACHE_CHANNEL_SIZE;
use ckb_async_runtime::Handle;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_verification::cache::TxVerificationCache;
use std::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{RwLock, mpsc, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityTaskRole {
    Worker(AuthorityWorkerRole),
    EffectPublisher,
    VerificationCache,
    Template(AuthorityTemplateRole),
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityGenerationFault {
    Worker {
        role: AuthorityTaskRole,
        fault: AuthorityWorkerFaultKind,
    },
    WorkerJoin {
        role: AuthorityTaskRole,
        error: tokio::task::JoinError,
    },
    Publisher(AuthorityEffectPublisherFault),
    PublisherJoin(tokio::task::JoinError),
    PublisherClosed,
    EffectClose(EffectCloseError),
    EffectDrain,
    ShutdownTimeout,
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityDerivedTaskFailure {
    Template {
        role: AuthorityTaskRole,
        fault: AuthorityTemplateDriverFault,
    },
    TemplateJoin {
        role: AuthorityTaskRole,
        error: tokio::task::JoinError,
    },
    TemplateClosed(AuthorityTaskRole),
    TemplateTimeout(AuthorityTaskRole),
    VerificationCacheJoin(tokio::task::JoinError),
    VerificationCacheClosed,
    VerificationCacheTimeout,
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityTopologyEvent {
    ShutdownRequested(AuthorityTaskRole),
    DerivedDegraded(AuthorityDerivedTaskFailure),
    GenerationInvalid(AuthorityGenerationFault),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AuthorityTopologyStartError {
    Cancelled,
    EffectPublisherClaimed,
    Worker(AuthorityWorkerSpawnError),
}

#[derive(Debug)]
pub(in crate::authority) enum AuthorityShutdownStatus {
    PersistenceEligible,
    PersistenceForbidden(AuthorityGenerationFault),
}

#[derive(Debug)]
pub(in crate::authority) struct AuthorityShutdownReport {
    status: AuthorityShutdownStatus,
    derived_failures: Vec<AuthorityDerivedTaskFailure>,
}

impl AuthorityShutdownReport {
    pub(in crate::authority) fn persistence_eligible(&self) -> bool {
        matches!(self.status, AuthorityShutdownStatus::PersistenceEligible)
    }

    pub(in crate::authority) fn status(&self) -> &AuthorityShutdownStatus {
        &self.status
    }

    pub(in crate::authority) fn derived_failures(&self) -> &[AuthorityDerivedTaskFailure] {
        &self.derived_failures
    }
}

/// Complete task and shutdown owner for one authority generation.
///
/// The optional block assembler has already completed its fallible adapter
/// construction. Endpoint construction is likewise outside this type. The
/// only fallible work here happens before `spawn_workers` creates its first
/// task, so an error cannot leave a partial generation running.
pub(in crate::authority) struct AuthorityTaskTopology {
    runtime: AuthorityRuntime,
    cancel: CancellationToken,
    workers: Vec<AuthorityWorkerTask>,
    templates: Option<[Option<AuthorityTemplateTask>; 4]>,
    publisher: Option<tokio::task::JoinHandle<Result<(), AuthorityEffectPublisherFault>>>,
    verification_cache: Option<tokio::task::JoinHandle<()>>,
}

impl AuthorityTaskTopology {
    pub(in crate::authority) fn start(
        handle: &Handle,
        runtime: AuthorityRuntime,
        cache: Arc<RwLock<TxVerificationCache>>,
        command_rx: watch::Receiver<ChunkCommand>,
        endpoints: AuthorityEffectEndpoints,
        block_assembler: Option<AuthorityBlockAssembler>,
        parent_cancel: CancellationToken,
    ) -> Result<Self, AuthorityTopologyStartError> {
        let cancel = parent_cancel.child_token();
        if cancel.is_cancelled() {
            return Err(AuthorityTopologyStartError::Cancelled);
        }
        let claim = runtime
            .claim_effect_publisher()
            .ok_or(AuthorityTopologyStartError::EffectPublisherClaimed)?;
        let (cache_updates, cache_receiver) = mpsc::channel(VERIFY_CACHE_CHANNEL_SIZE);
        let workers = runtime
            .spawn_workers(
                handle,
                Arc::clone(&cache),
                cache_updates,
                command_rx,
                cancel.child_token(),
            )
            .map_err(AuthorityTopologyStartError::Worker)?;

        let verification_cache =
            handle.spawn(run_verification_cache_updates(cache, cache_receiver));
        let publisher_runtime = runtime.clone();
        let publisher = handle.spawn(async move {
            run_claimed_authority_effect_publisher(publisher_runtime, endpoints, claim).await
        });
        let templates = block_assembler.map(|assembler| {
            assembler
                .spawn_drivers(handle, cancel.child_token())
                .tasks
                .map(Some)
        });

        Ok(Self {
            runtime,
            cancel,
            workers: workers.tasks,
            templates,
            publisher: Some(publisher),
            verification_cache: Some(verification_cache),
        })
    }

    /// Wait for the first task boundary without polling or manufacturing a
    /// second health state. Derived failures may be reported and observed
    /// again; a generation-invalid event must be consumed by
    /// `invalidate_generation` so its linear failure capability is retained.
    pub(in crate::authority) async fn next_event(&mut self) -> AuthorityTopologyEvent {
        poll_fn(|context| self.poll_next_event(context)).await
    }

    #[cfg(test)]
    pub(super) fn install_template_task_for_foundation(&mut self, task: AuthorityTemplateTask) {
        self.templates = Some([Some(task), None, None, None]);
    }

    /// Ordered graceful shutdown. A timeout or any authority-owning task
    /// failure makes persistence ineligible; template/cache failures remain
    /// derived diagnostics and cannot change transaction ownership.
    pub(in crate::authority) async fn shutdown(
        mut self,
        timeout: Duration,
    ) -> AuthorityShutdownReport {
        self.cancel.cancel();
        match tokio::time::timeout(timeout, self.shutdown_authority()).await {
            Ok(Ok(())) => {
                let mut derived_failures = Vec::new();
                self.join_templates(timeout, &mut derived_failures).await;
                self.join_verification_cache(timeout, &mut derived_failures)
                    .await;
                AuthorityShutdownReport {
                    status: AuthorityShutdownStatus::PersistenceEligible,
                    derived_failures,
                }
            }
            Ok(Err(fault)) => {
                self.abort_all();
                AuthorityShutdownReport {
                    status: AuthorityShutdownStatus::PersistenceForbidden(fault),
                    derived_failures: Vec::new(),
                }
            }
            Err(_) => {
                self.abort_all();
                AuthorityShutdownReport {
                    status: AuthorityShutdownStatus::PersistenceForbidden(
                        AuthorityGenerationFault::ShutdownTimeout,
                    ),
                    derived_failures: Vec::new(),
                }
            }
        }
    }

    /// End a generation whose exact event already proved safe continuation or
    /// persistence impossible. No repair or replacement authority is created
    /// here; G5.3c owns the service-level operational response.
    pub(in crate::authority) fn invalidate_generation(
        mut self,
        fault: AuthorityGenerationFault,
    ) -> AuthorityShutdownReport {
        self.cancel.cancel();
        self.abort_all();
        AuthorityShutdownReport {
            status: AuthorityShutdownStatus::PersistenceForbidden(fault),
            derived_failures: Vec::new(),
        }
    }

    async fn shutdown_authority(&mut self) -> Result<(), AuthorityGenerationFault> {
        if let Some(fault) = self.join_authority_workers().await {
            return Err(fault);
        }

        if let Err(error) = self.runtime.close_effects() {
            return Err(AuthorityGenerationFault::EffectClose(error));
        }
        if let Some(fault) = self.join_publisher().await {
            return Err(fault);
        }
        if !self.runtime.effects_closed_and_drained() {
            return Err(AuthorityGenerationFault::EffectDrain);
        }
        Ok(())
    }

    async fn join_authority_workers(&mut self) -> Option<AuthorityGenerationFault> {
        while let Some(task) = self.workers.last_mut() {
            let role = task.role;
            let result = (&mut task.handle).await;
            self.workers.pop();
            if let Some(fault) = worker_generation_fault(AuthorityTaskRole::Worker(role), result) {
                return Some(fault);
            }
        }
        None
    }

    async fn join_templates(
        &mut self,
        timeout: Duration,
        failures: &mut Vec<AuthorityDerivedTaskFailure>,
    ) {
        let Some(templates) = self.templates.as_mut() else {
            return;
        };
        for slot in templates {
            let Some(mut task) = slot.take() else {
                continue;
            };
            let role = AuthorityTaskRole::Template(task.role);
            match tokio::time::timeout(timeout, &mut task.handle).await {
                Ok(result) => {
                    if let Some(failure) = template_failure(role, result, true) {
                        failures.push(failure);
                    }
                }
                Err(_) => {
                    task.handle.abort();
                    let _ = task.handle.await;
                    failures.push(AuthorityDerivedTaskFailure::TemplateTimeout(role));
                }
            }
        }
    }

    async fn join_publisher(&mut self) -> Option<AuthorityGenerationFault> {
        let result = join_slot(&mut self.publisher).await?;
        match result {
            Ok(Ok(())) => None,
            Ok(Err(fault)) => Some(AuthorityGenerationFault::Publisher(fault)),
            Err(error) => Some(AuthorityGenerationFault::PublisherJoin(error)),
        }
    }

    async fn join_verification_cache(
        &mut self,
        timeout: Duration,
        failures: &mut Vec<AuthorityDerivedTaskFailure>,
    ) {
        let Some(mut task) = self.verification_cache.take() else {
            return;
        };
        match tokio::time::timeout(timeout, &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                failures.push(AuthorityDerivedTaskFailure::VerificationCacheJoin(error));
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                failures.push(AuthorityDerivedTaskFailure::VerificationCacheTimeout);
            }
        }
    }

    fn poll_next_event(&mut self, context: &mut Context<'_>) -> Poll<AuthorityTopologyEvent> {
        let cancelled = self.cancel.is_cancelled();
        let mut completed_worker = None;
        for (index, task) in self.workers.iter_mut().enumerate() {
            if let Poll::Ready(result) = Pin::new(&mut task.handle).poll(context) {
                completed_worker = Some((index, result));
                break;
            }
        }
        if let Some((index, result)) = completed_worker {
            let task = self.workers.remove(index);
            let role = AuthorityTaskRole::Worker(task.role);
            if let Some(event) = worker_event(role, result) {
                return Poll::Ready(event);
            }
        }
        if let Some(event) = poll_publisher_slot(&mut self.publisher, cancelled, context) {
            return Poll::Ready(event);
        }
        if let Some(templates) = self.templates.as_mut() {
            for slot in templates {
                if let Some(event) = poll_template_slot(slot, cancelled, context) {
                    return Poll::Ready(event);
                }
            }
        }
        if let Some(event) = poll_cache_slot(&mut self.verification_cache, cancelled, context) {
            return Poll::Ready(event);
        }
        Poll::Pending
    }

    fn abort_all(&mut self) {
        for task in self.workers.drain(..) {
            task.handle.abort();
        }
        if let Some(templates) = self.templates.as_mut() {
            for slot in templates {
                if let Some(task) = slot.take() {
                    task.handle.abort();
                }
            }
        }
        abort_slot(&mut self.publisher);
        abort_slot(&mut self.verification_cache);
    }
}

impl Drop for AuthorityTaskTopology {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.abort_all();
    }
}

async fn run_verification_cache_updates(
    cache: Arc<RwLock<TxVerificationCache>>,
    mut receiver: mpsc::Receiver<VerificationCacheUpdate>,
) {
    while let Some(update) = receiver.recv().await {
        let (key, completed) = update.into_parts();
        cache.write().await.put(key, completed);
    }
}

async fn join_slot<T>(
    slot: &mut Option<tokio::task::JoinHandle<T>>,
) -> Option<Result<T, tokio::task::JoinError>> {
    let result = match slot.as_mut() {
        Some(handle) => handle.await,
        None => return None,
    };
    *slot = None;
    Some(result)
}

fn worker_generation_fault(
    role: AuthorityTaskRole,
    result: Result<Result<(), AuthorityWorkerFault>, tokio::task::JoinError>,
) -> Option<AuthorityGenerationFault> {
    match result {
        Ok(Ok(())) => None,
        Ok(Err(fault)) => Some(AuthorityGenerationFault::Worker {
            role,
            fault: fault.into_kind(),
        }),
        Err(error) => Some(AuthorityGenerationFault::WorkerJoin { role, error }),
    }
}

fn template_failure(
    role: AuthorityTaskRole,
    result: Result<Result<(), AuthorityTemplateDriverFault>, tokio::task::JoinError>,
    shutdown: bool,
) -> Option<AuthorityDerivedTaskFailure> {
    match result {
        Ok(Ok(())) if shutdown => None,
        Ok(Ok(())) => Some(AuthorityDerivedTaskFailure::TemplateClosed(role)),
        Ok(Err(fault)) => Some(AuthorityDerivedTaskFailure::Template { role, fault }),
        Err(error) => Some(AuthorityDerivedTaskFailure::TemplateJoin { role, error }),
    }
}

fn worker_event(
    role: AuthorityTaskRole,
    result: Result<Result<(), AuthorityWorkerFault>, tokio::task::JoinError>,
) -> Option<AuthorityTopologyEvent> {
    Some(match result {
        Ok(Ok(())) => AuthorityTopologyEvent::ShutdownRequested(role),
        Ok(Err(fault)) => {
            AuthorityTopologyEvent::GenerationInvalid(AuthorityGenerationFault::Worker {
                role,
                fault: fault.into_kind(),
            })
        }
        Err(error) => {
            AuthorityTopologyEvent::GenerationInvalid(AuthorityGenerationFault::WorkerJoin {
                role,
                error,
            })
        }
    })
}

fn poll_template_slot(
    slot: &mut Option<AuthorityTemplateTask>,
    cancelled: bool,
    context: &mut Context<'_>,
) -> Option<AuthorityTopologyEvent> {
    let (role, result) = match slot.as_mut() {
        Some(task) => {
            let role = AuthorityTaskRole::Template(task.role);
            let result = match Pin::new(&mut task.handle).poll(context) {
                Poll::Ready(result) => result,
                Poll::Pending => return None,
            };
            (role, result)
        }
        None => return None,
    };
    *slot = None;
    if cancelled && matches!(result, Ok(Ok(()))) {
        return Some(AuthorityTopologyEvent::ShutdownRequested(role));
    }
    template_failure(role, result, false).map(AuthorityTopologyEvent::DerivedDegraded)
}

fn poll_publisher_slot(
    slot: &mut Option<tokio::task::JoinHandle<Result<(), AuthorityEffectPublisherFault>>>,
    cancelled: bool,
    context: &mut Context<'_>,
) -> Option<AuthorityTopologyEvent> {
    let result = poll_slot(slot, context)?;
    Some(match result {
        Ok(Ok(())) if cancelled => {
            AuthorityTopologyEvent::ShutdownRequested(AuthorityTaskRole::EffectPublisher)
        }
        Ok(Ok(())) => {
            AuthorityTopologyEvent::GenerationInvalid(AuthorityGenerationFault::PublisherClosed)
        }
        Ok(Err(fault)) => {
            AuthorityTopologyEvent::GenerationInvalid(AuthorityGenerationFault::Publisher(fault))
        }
        Err(error) => AuthorityTopologyEvent::GenerationInvalid(
            AuthorityGenerationFault::PublisherJoin(error),
        ),
    })
}

fn poll_cache_slot(
    slot: &mut Option<tokio::task::JoinHandle<()>>,
    cancelled: bool,
    context: &mut Context<'_>,
) -> Option<AuthorityTopologyEvent> {
    let result = poll_slot(slot, context)?;
    Some(match result {
        Ok(()) if cancelled => {
            AuthorityTopologyEvent::ShutdownRequested(AuthorityTaskRole::VerificationCache)
        }
        Ok(()) => AuthorityTopologyEvent::DerivedDegraded(
            AuthorityDerivedTaskFailure::VerificationCacheClosed,
        ),
        Err(error) => AuthorityTopologyEvent::DerivedDegraded(
            AuthorityDerivedTaskFailure::VerificationCacheJoin(error),
        ),
    })
}

fn poll_slot<T>(
    slot: &mut Option<tokio::task::JoinHandle<T>>,
    context: &mut Context<'_>,
) -> Option<Result<T, tokio::task::JoinError>> {
    let result = match slot.as_mut() {
        Some(handle) => match Pin::new(handle).poll(context) {
            Poll::Ready(result) => result,
            Poll::Pending => return None,
        },
        None => return None,
    };
    *slot = None;
    Some(result)
}

fn abort_slot<T>(slot: &mut Option<tokio::task::JoinHandle<T>>) {
    if let Some(handle) = slot.take() {
        handle.abort();
    }
}
