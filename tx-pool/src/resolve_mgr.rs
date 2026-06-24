//! Resolve stage of the tx-pool pipeline.
//!
//! The pipeline now has two resolver stages:
//!
//! 1. **Concurrent pre-resolver** (`PreResolveMgr`): multiple workers pop raw
//!    transactions from the first-stage [`ResolveQueue`] and run the cheap but
//!    not-strictly-ordered `pre_check`.  Transactions whose inputs are already
//!    available are pushed straight to the [`VerifyQueue`].
//!
//! 2. **Ordered resolver** (`OrderedResolver`): a single worker pops jobs from
//!    the [`OrderedResolveQueue`] that the pre-resolver could not finish
//!    because of missing inputs.  Keeping this stage ordered reduces orphan-pool
//!    churn for dependent transactions.
//!
//! Running the pre-resolve stage concurrently means independent transactions
//! can be resolved in parallel, while the ordered resolver still guarantees
//! that dependent transactions are retried in arrival order.

use crate::component::ordered_resolve_queue::OrderedResolveQueue;
use crate::component::resolve_queue::ResolveQueue;
use crate::component::verify_queue::VerifyQueue;
use crate::error::Reject;
use crate::resolved_tx::{ResolveJob, ResolvedTx};
use crate::service::{TxPoolService, TxVerificationResult};
use ckb_logger::{debug, error, info};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use ckb_types::core::error::OutPointError;
use ckb_types::packed::{Byte32, ProposalShortId};
use futures_util::FutureExt;
use std::any::Any;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};
use tokio::task::JoinHandle;

/// Result of attempting to resolve one transaction.
#[derive(Debug)]
pub(crate) enum ResolveStageResult {
    /// Transaction resolved successfully and is ready for verification.
    Ready(ResolvedTx),
    /// Transaction has unknown parent transactions and should be sent to the
    /// ordered resolver (or, if already there, to the orphan pool).
    Orphan(ProposalShortId, HashSet<Byte32>),
    /// Transaction is invalid and should be rejected.
    Reject(ckb_types::core::TransactionView, Reject),
}

/// Worker that concurrently pre-resolves transactions from the first-stage
/// queue.
struct PreResolveWorker {
    resolve_queue: Arc<RwLock<ResolveQueue>>,
    ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
    verify_queue: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    service: TxPoolService,
    exit_signal: CancellationToken,
    status: ChunkCommand,
}

impl Clone for PreResolveWorker {
    fn clone(&self) -> Self {
        Self {
            resolve_queue: Arc::clone(&self.resolve_queue),
            ordered_queue: Arc::clone(&self.ordered_queue),
            verify_queue: Arc::clone(&self.verify_queue),
            command_rx: self.command_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            service: self.service.clone(),
            status: self.status.clone(),
        }
    }
}

impl PreResolveWorker {
    fn new(
        service: TxPoolService,
        resolve_queue: Arc<RwLock<ResolveQueue>>,
        ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        PreResolveWorker {
            service,
            resolve_queue,
            ordered_queue,
            verify_queue,
            command_rx,
            exit_signal,
            status: ChunkCommand::Resume,
        }
    }

    fn start(
        self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, PreResolveWorkerExit)>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let exit = match AssertUnwindSafe(self.run()).catch_unwind().await {
                Ok(()) => PreResolveWorkerExit::Stopped,
                Err(payload) => PreResolveWorkerExit::Panicked {
                    message: panic_payload_to_string(payload.as_ref()),
                },
            };
            if let Err(err) = exit_tx.send((worker_id, exit)) {
                error!(
                    "failed to notify tx-pool pre-resolve worker exit: {:?}",
                    err.0
                );
            }
        })
    }

    async fn run(mut self) {
        let queue_ready = self.resolve_queue.read().await.subscribe();
        self.refresh_status();
        loop {
            tokio::select! {
                _ = self.exit_signal.cancelled() => {
                    break;
                }
                _ = self.command_rx.changed() => {
                    self.status = self.command_rx.borrow_and_update().to_owned();
                    self.process_inner().await;
                }
                _ = queue_ready.notified() => {
                    self.process_inner().await;
                }
            };
        }
    }

    fn refresh_status(&mut self) {
        self.status = self.command_rx.borrow().to_owned();
    }

    async fn process_inner(&mut self) {
        loop {
            if self.exit_signal.is_cancelled() {
                info!("Pre-resolve worker::process_inner exit_signal is cancelled");
                return;
            }
            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }
            if self.resolve_queue.read().await.is_empty() {
                return;
            }

            let job = {
                let mut queue = self.resolve_queue.write().await;
                queue.pop_front()
            };
            let Some(job) = job else {
                return;
            };

            match self.resolve(job.clone()).await {
                ResolveStageResult::Ready(resolved) => {
                    self.push_to_verify_queue(resolved).await;
                }
                ResolveStageResult::Orphan(_, _) => {
                    // Missing inputs: let the ordered resolver retry in sequence.
                    self.ordered_queue.write().await.add_tx(job);
                }
                ResolveStageResult::Reject(tx, reject) => {
                    let (_ret, snapshot) = self
                        .service
                        .with_tx_pool_read_lock(|_tx_pool, snapshot| snapshot)
                        .await;
                    self.service
                        .after_process(tx, job.remote, &snapshot, &Err(reject))
                        .await;
                }
            }
        }
    }

    async fn resolve(&self, job: ResolveJob) -> ResolveStageResult {
        let id = job.tx.proposal_short_id();
        let (pre_check_ret, _snapshot) = self.service.pre_check(&job.tx).await;

        match pre_check_ret {
            Ok((pre_resolve_tip, rtx, status, fee, tx_size)) => {
                debug!("pre-resolve stage resolved tx {}", id);
                ResolveStageResult::Ready(ResolvedTx {
                    tx: job.tx,
                    rtx,
                    status,
                    fee,
                    tx_size,
                    pre_resolve_tip,
                    snapshot: _snapshot,
                    remote: job.remote,
                    is_proposal_tx: job.is_proposal_tx,
                })
            }
            Err(reject) => {
                if crate::util::is_missing_input(&reject) {
                    let parents = job.tx.unique_parents();
                    ResolveStageResult::Orphan(id, parents)
                } else {
                    ResolveStageResult::Reject(job.tx, reject)
                }
            }
        }
    }

    async fn push_to_verify_queue(&self, resolved: ResolvedTx) {
        let mut queue = self.verify_queue.write().await;
        match queue.add_tx(resolved.clone()) {
            Ok(true) => {}
            Ok(false) => {
                debug!("resolved tx {} already in verify queue", resolved.tx.hash());
            }
            Err(reject) => {
                self.service
                    .after_process(
                        resolved.tx,
                        resolved.remote,
                        &resolved.snapshot,
                        &Err(reject),
                    )
                    .await;
            }
        }
    }
}

#[derive(Debug)]
enum PreResolveWorkerExit {
    Stopped,
    Panicked { message: String },
}

/// Manager of the concurrent pre-resolve stage.
pub(crate) struct PreResolveMgr {
    workers: Vec<(watch::Sender<ChunkCommand>, PreResolveWorker)>,
    join_handles: Option<Vec<Option<JoinHandle<()>>>>,
    signal_exit: CancellationToken,
    command_rx: watch::Receiver<ChunkCommand>,
}

impl PreResolveMgr {
    pub fn new(
        service: TxPoolService,
        resolve_queue: Arc<RwLock<ResolveQueue>>,
        ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
    ) -> Self {
        let worker_num = service.tx_pool_config.max_tx_verify_workers;
        let workers: Vec<_> = (0..worker_num)
            .map({
                let resolve_queue = Arc::clone(&resolve_queue);
                let ordered_queue = Arc::clone(&ordered_queue);
                let verify_queue = Arc::clone(&verify_queue);
                let signal_exit = signal_exit.clone();
                move |_| {
                    let (child_tx, child_rx) = watch::channel(ChunkCommand::Resume);
                    (
                        child_tx,
                        PreResolveWorker::new(
                            service.clone(),
                            Arc::clone(&resolve_queue),
                            Arc::clone(&ordered_queue),
                            Arc::clone(&verify_queue),
                            child_rx,
                            signal_exit.clone(),
                        ),
                    )
                }
            })
            .collect();
        Self {
            workers,
            join_handles: None,
            signal_exit,
            command_rx,
        }
    }

    fn send_child_command(&self, command: ChunkCommand) {
        for w in &self.workers {
            if let Err(err) = w.0.send(command.clone()) {
                info!("send pre-resolve worker command failed, error: {}", err);
            }
        }
    }

    fn spawn_worker(
        &mut self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, PreResolveWorkerExit)>,
    ) {
        let Some(worker) = self
            .workers
            .get(worker_id)
            .map(|(_, worker)| worker.clone())
        else {
            error!(
                "cannot respawn missing tx-pool pre-resolve worker {}",
                worker_id
            );
            return;
        };
        let handle = worker.start(worker_id, exit_tx);
        if let Some(handles) = self.join_handles.as_mut()
            && let Some(handle_slot) = handles.get_mut(worker_id)
        {
            handle_slot.replace(handle);
        } else {
            error!(
                "cannot store handle for tx-pool pre-resolve worker {}",
                worker_id
            );
        }
    }

    async fn join_worker(&mut self, worker_id: usize) {
        let handle = self
            .join_handles
            .as_mut()
            .and_then(|handles| handles.get_mut(worker_id))
            .and_then(Option::take);

        if let Some(handle) = handle
            && let Err(err) = handle.await
        {
            error!(
                "tx-pool pre-resolve worker {} join failed after exit notification: {}",
                worker_id, err
            );
        }
    }

    async fn start_loop(&mut self) {
        let (worker_exit_tx, mut worker_exit_rx) = mpsc::unbounded_channel();
        let mut join_handles = Vec::new();
        for (worker_id, (_, worker)) in self.workers.iter().enumerate() {
            let handle = worker.clone().start(worker_id, worker_exit_tx.clone());
            join_handles.push(Some(handle));
        }
        self.join_handles.replace(join_handles);

        loop {
            tokio::select! {
                _ = self.signal_exit.cancelled() => {
                    info!("TxPool pre-resolve service received exit signal, exit now");
                    self.send_child_command(ChunkCommand::Stop);
                    break;
                },
                _ = self.command_rx.changed() => {
                    let command = self.command_rx.borrow().to_owned();
                    self.send_child_command(command);
                },
                Some((worker_id, exit)) = worker_exit_rx.recv() => {
                    self.join_worker(worker_id).await;
                    if self.signal_exit.is_cancelled() {
                        continue;
                    }
                    match exit {
                        PreResolveWorkerExit::Stopped => {
                            error!(
                                "tx-pool pre-resolve worker {} stopped unexpectedly, respawning",
                                worker_id
                            );
                        }
                        PreResolveWorkerExit::Panicked { message } => {
                            error!(
                                "tx-pool pre-resolve worker {} panicked: {}; respawning",
                                worker_id, message
                            );
                        }
                    }
                    self.spawn_worker(worker_id, worker_exit_tx.clone());
                }
            }
        }
        if let Some(jh) = self.join_handles.take() {
            for h in jh.into_iter().flatten() {
                if let Err(err) = h.await {
                    error!("tx-pool pre-resolve worker join failed: {}", err);
                }
            }
        }
        info!("TxPool pre-resolve service exited");
    }

    pub async fn run(&mut self) {
        self.start_loop().await;
    }
}

/// Single ordered resolver worker.
///
/// Processes transactions that the concurrent pre-resolver could not resolve
/// because of missing inputs.  Keeping this worker single-threaded preserves
/// arrival ordering for dependent transactions.
pub(crate) struct OrderedResolver {
    ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
    verify_queue: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    service: TxPoolService,
    exit_signal: CancellationToken,
    status: ChunkCommand,
}

impl Clone for OrderedResolver {
    fn clone(&self) -> Self {
        Self {
            ordered_queue: Arc::clone(&self.ordered_queue),
            verify_queue: Arc::clone(&self.verify_queue),
            command_rx: self.command_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            service: self.service.clone(),
            status: self.status.clone(),
        }
    }
}

impl OrderedResolver {
    pub fn new(
        service: TxPoolService,
        ordered_queue: Arc<RwLock<OrderedResolveQueue>>,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        OrderedResolver {
            ordered_queue,
            verify_queue,
            command_rx,
            exit_signal,
            service,
            status: ChunkCommand::Resume,
        }
    }

    pub fn start(self, exit_tx: tokio::sync::mpsc::UnboundedSender<ResolveExit>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let exit = match AssertUnwindSafe(self.run()).catch_unwind().await {
                Ok(()) => ResolveExit::Stopped,
                Err(payload) => ResolveExit::Panicked {
                    message: panic_payload_to_string(payload.as_ref()),
                },
            };
            if let Err(err) = exit_tx.send(exit) {
                error!(
                    "failed to notify tx-pool ordered resolver exit: {:?}",
                    err.0
                );
            }
        })
    }

    async fn run(mut self) {
        let queue_ready = self.ordered_queue.read().await.subscribe();
        self.refresh_status();
        loop {
            tokio::select! {
                _ = self.exit_signal.cancelled() => {
                    break;
                }
                _ = self.command_rx.changed() => {
                    self.status = self.command_rx.borrow_and_update().to_owned();
                    self.process_inner().await;
                }
                _ = queue_ready.notified() => {
                    self.process_inner().await;
                }
            };
        }
    }

    fn refresh_status(&mut self) {
        self.status = self.command_rx.borrow().to_owned();
    }

    async fn process_inner(&mut self) {
        loop {
            if self.exit_signal.is_cancelled() {
                info!("Ordered resolver::process_inner exit_signal is cancelled");
                return;
            }
            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }
            if self.ordered_queue.read().await.is_empty() {
                return;
            }

            let job = {
                let mut queue = self.ordered_queue.write().await;
                queue.pop_front()
            };
            let Some(job) = job else {
                return;
            };

            match self.resolve(job.clone()).await {
                ResolveStageResult::Ready(resolved) => {
                    self.push_to_verify_queue(resolved).await;
                }
                ResolveStageResult::Orphan(id, parents) => {
                    if let Some((declared_cycle, peer)) = job.remote {
                        debug!(
                            "ordered resolve stage orphan tx {} from peer {}, parents {:?}",
                            id, peer, parents
                        );
                        self.service
                            .send_result_to_relayer(TxVerificationResult::UnknownParents {
                                peer,
                                parents,
                            });
                        self.service
                            .add_orphan(job.tx.clone(), peer, declared_cycle)
                            .await;
                    } else {
                        // Local transactions with missing inputs are rejected.
                        let reject = first_unknown_input_reject(&job.tx);
                        let (_ret, snapshot) = self
                            .service
                            .with_tx_pool_read_lock(|_tx_pool, snapshot| snapshot)
                            .await;
                        self.service
                            .after_process(job.tx, None, &snapshot, &Err(reject))
                            .await;
                    }
                }
                ResolveStageResult::Reject(tx, reject) => {
                    let (_ret, snapshot) = self
                        .service
                        .with_tx_pool_read_lock(|_tx_pool, snapshot| snapshot)
                        .await;
                    self.service
                        .after_process(tx, job.remote, &snapshot, &Err(reject))
                        .await;
                }
            }
        }
    }

    async fn resolve(&self, job: ResolveJob) -> ResolveStageResult {
        let id = job.tx.proposal_short_id();
        let (pre_check_ret, _snapshot) = self.service.pre_check(&job.tx).await;

        match pre_check_ret {
            Ok((pre_resolve_tip, rtx, status, fee, tx_size)) => {
                debug!("ordered resolve stage resolved tx {}", id);
                ResolveStageResult::Ready(ResolvedTx {
                    tx: job.tx,
                    rtx,
                    status,
                    fee,
                    tx_size,
                    pre_resolve_tip,
                    snapshot: _snapshot,
                    remote: job.remote,
                    is_proposal_tx: job.is_proposal_tx,
                })
            }
            Err(reject) => {
                if crate::util::is_missing_input(&reject) {
                    let parents = job.tx.unique_parents();
                    ResolveStageResult::Orphan(id, parents)
                } else {
                    ResolveStageResult::Reject(job.tx, reject)
                }
            }
        }
    }

    async fn push_to_verify_queue(&self, resolved: ResolvedTx) {
        let mut queue = self.verify_queue.write().await;
        match queue.add_tx(resolved.clone()) {
            Ok(true) => {}
            Ok(false) => {
                debug!("resolved tx {} already in verify queue", resolved.tx.hash());
            }
            Err(reject) => {
                self.service
                    .after_process(
                        resolved.tx,
                        resolved.remote,
                        &resolved.snapshot,
                        &Err(reject),
                    )
                    .await;
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum ResolveExit {
    Stopped,
    Panicked { message: String },
}

fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn first_unknown_input_reject(tx: &ckb_types::core::TransactionView) -> Reject {
    let outpoint = tx.input_pts_iter().next().unwrap_or_default();
    Reject::Resolve(OutPointError::Unknown(outpoint))
}
