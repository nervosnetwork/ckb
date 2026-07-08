use crate::component::pipeline_queue::PipelineQueue;
use crate::component::verify_queue::VerifyQueue;
use crate::resolved_tx::ResolvedTx;
use crate::service::TxPoolService;
use crate::worker::{JobHandler, WorkerOutcome, WorkerRunner};
use ckb_logger::{debug, error, info};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};
use tokio::task::JoinHandle;

#[derive(Clone, Debug, PartialEq)]
enum WorkerRole {
    OnlySmallCycleTx,
    SubmitTimeFirst,
}

#[derive(Debug)]
enum WorkerExit {
    Stopped { role: WorkerRole },
    Panicked { role: WorkerRole, message: String },
}

#[derive(Clone)]
struct VerifyHandler {
    tasks: Arc<RwLock<VerifyQueue>>,
    service: TxPoolService,
    role: WorkerRole,
    /// A clone of the command receiver used by `verify_and_submit_tx` to check
    /// for pause/cancel while verifying. `WorkerRunner` holds another clone for
    /// its own select loop; sharing the same watch channel is cheap and correct.
    command_rx: watch::Receiver<ChunkCommand>,
}

impl JobHandler for VerifyHandler {
    type Job = ResolvedTx;
    type Exit = WorkerExit;

    fn worker_name(&self) -> &'static str {
        "verify worker"
    }

    async fn is_queue_empty(&self) -> bool {
        self.tasks.read().await.is_empty()
    }

    async fn queue_ready(&self) -> Arc<tokio::sync::Notify> {
        self.tasks.read().await.subscribe()
    }

    async fn pop_one(&mut self) -> Option<ResolvedTx> {
        let mut tasks = self.tasks.write().await;
        match tasks.pop_front(self.role == WorkerRole::OnlySmallCycleTx) {
            Some(resolved) => Some(resolved),
            None => {
                if !tasks.is_empty() {
                    tasks.re_notify();
                    debug!(
                        "Worker (role: {:?}) didn't get tx after pop_front, but tasks is not empty, notify other Workers now",
                        self.role
                    );
                }
                None
            }
        }
    }

    async fn process_one(&mut self, resolved: ResolvedTx) {
        let tx = resolved.tx.clone();
        let remote = resolved.remote;
        let is_proposal_tx = resolved.is_proposal_tx;
        if let Some((res, snapshot)) = self
            .service
            .verify_and_submit_tx(resolved, Some(&mut self.command_rx))
            .await
        {
            self.service
                .after_process(tx.clone(), remote, &snapshot, &res, is_proposal_tx)
                .await;
        } else {
            info!("verify_and_submit_tx for tx: {} returned none", tx.hash());
        }
    }

    fn make_exit(&self, outcome: WorkerOutcome) -> WorkerExit {
        match outcome {
            WorkerOutcome::Stopped => WorkerExit::Stopped {
                role: self.role.clone(),
            },
            WorkerOutcome::Panicked(message) => WorkerExit::Panicked {
                role: self.role.clone(),
                message,
            },
        }
    }
}

pub(crate) struct VerifyMgr {
    workers: Vec<WorkerRunner<VerifyHandler>>,
    join_handles: Option<Vec<Option<JoinHandle<()>>>>,
    signal_exit: CancellationToken,
}

impl VerifyMgr {
    pub fn new(
        service: TxPoolService,
        command_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
    ) -> Self {
        let worker_num = service.tx_pool_config.max_tx_verify_workers;
        let workers: Vec<_> = (0..worker_num)
            .map({
                let tasks = Arc::clone(&service.queues.verify_queue);
                let signal_exit = signal_exit.clone();
                move |idx| {
                    let role = if idx == 0 && worker_num > 1 {
                        WorkerRole::OnlySmallCycleTx
                    } else {
                        WorkerRole::SubmitTimeFirst
                    };
                    let handler = VerifyHandler {
                        tasks: Arc::clone(&tasks),
                        service: service.clone(),
                        role,
                        command_rx: command_rx.clone(),
                    };
                    WorkerRunner::new(handler, command_rx.clone(), signal_exit.clone())
                }
            })
            .collect();
        Self {
            workers,
            join_handles: None,
            signal_exit,
        }
    }

    fn spawn_worker(
        &mut self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, WorkerExit)>,
    ) {
        let Some(worker) = self.workers.get(worker_id).cloned() else {
            error!("cannot respawn missing tx-pool verify worker {}", worker_id);
            return;
        };
        let handle = worker.start(worker_id, exit_tx);
        if let Some(handles) = self.join_handles.as_mut()
            && let Some(handle_slot) = handles.get_mut(worker_id)
        {
            handle_slot.replace(handle);
        } else {
            error!(
                "cannot store handle for tx-pool verify worker {}",
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
                "tx-pool verify worker {} join failed after exit notification: {}",
                worker_id, err
            );
        }
    }

    async fn start_loop(&mut self) {
        let (worker_exit_tx, mut worker_exit_rx) = mpsc::unbounded_channel();
        let mut join_handles = Vec::new();
        for (worker_id, w) in self.workers.iter_mut().enumerate() {
            let h = w.clone().start(worker_id, worker_exit_tx.clone());
            join_handles.push(Some(h));
        }
        self.join_handles.replace(join_handles);
        loop {
            tokio::select! {
                _ = self.signal_exit.cancelled() => {
                    info!("TxPool chunk_command service received exit signal, exit now");
                    // Workers will exit via their own CancellationToken;
                    // no need to broadcast Stop through per-worker channels.
                    break;
                },
                Some((worker_id, exit)) = worker_exit_rx.recv() => {
                    self.join_worker(worker_id).await;
                    if self.signal_exit.is_cancelled() {
                        continue;
                    }
                    match exit {
                        WorkerExit::Stopped { role } => {
                            error!(
                                "tx-pool verify worker {} ({:?}) stopped unexpectedly, respawning",
                                worker_id, role
                            );
                        }
                        WorkerExit::Panicked { role, message } => {
                            error!(
                                "tx-pool verify worker {} ({:?}) panicked: {}; respawning",
                                worker_id, role, message
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
                    error!("tx-pool verify worker join failed: {}", err);
                }
            }
        }
        info!("TxPool verify_mgr service exited");
    }

    pub async fn run(&mut self) {
        self.start_loop().await;
    }
}
