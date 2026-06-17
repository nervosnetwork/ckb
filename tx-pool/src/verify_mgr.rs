use crate::component::verify_queue::VerifyQueue;
use crate::service::TxPoolService;
use ckb_logger::{debug, error, info};
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use futures_util::FutureExt;
use std::any::Any;
use std::panic::AssertUnwindSafe;
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

struct Worker {
    tasks: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    service: TxPoolService,
    exit_signal: CancellationToken,
    status: ChunkCommand,
    role: WorkerRole,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            command_rx: self.command_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            service: self.service.clone(),
            status: self.status.clone(),
            role: self.role.clone(),
        }
    }
}

impl Worker {
    pub fn new(
        service: TxPoolService,
        tasks: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
        role: WorkerRole,
    ) -> Self {
        Worker {
            service,
            tasks,
            command_rx,
            exit_signal,
            status: ChunkCommand::Resume,
            role,
        }
    }

    pub fn start(
        self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, WorkerExit)>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let role = self.role.clone();
            let exit = match AssertUnwindSafe(self.run()).catch_unwind().await {
                Ok(()) => WorkerExit::Stopped { role },
                Err(payload) => WorkerExit::Panicked {
                    role,
                    message: panic_payload_to_string(payload.as_ref()),
                },
            };

            if let Err(err) = exit_tx.send((worker_id, exit)) {
                error!("failed to notify tx-pool verify worker exit: {:?}", err.0);
            }
        })
    }

    async fn run(mut self) {
        let queue_ready = self.tasks.read().await.subscribe();
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
                info!("Verify worker::process_inner exit_signal is cancelled");
                return;
            }
            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }
            // cheap query to check queue is not empty
            if self.tasks.read().await.is_empty() {
                return;
            }

            self.refresh_status();
            if self.status != ChunkCommand::Resume {
                return;
            }

            // pick a entry to run verify
            let entry = {
                let mut tasks = self.tasks.write().await;
                match tasks.pop_front(self.role == WorkerRole::OnlySmallCycleTx) {
                    Some(entry) => entry,
                    None => {
                        if !tasks.is_empty() {
                            tasks.re_notify();
                            debug!(
                                "Worker (role: {:?}) didn't got tx after pop_front, but tasks is not empty, notify other Workers now",
                                self.role
                            );
                        }
                        return;
                    }
                }
            };

            if let Some((res, snapshot)) = self
                .service
                ._process_tx(
                    entry.tx.clone(),
                    entry.remote.map(|e| e.0),
                    Some(&mut self.command_rx),
                )
                .await
            {
                self.service
                    .after_process(entry.tx, entry.remote, &snapshot, &res)
                    .await;
            } else {
                info!("_process_tx for tx: {} returned none", entry.tx.hash());
            }
        }
    }
}

pub(crate) struct VerifyMgr {
    workers: Vec<(watch::Sender<ChunkCommand>, Worker)>,
    join_handles: Option<Vec<Option<JoinHandle<()>>>>,
    signal_exit: CancellationToken,
    command_rx: watch::Receiver<ChunkCommand>,
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
                let tasks = Arc::clone(&service.verify_queue);
                let signal_exit = signal_exit.clone();
                move |idx| {
                    let role = if idx == 0 && worker_num > 1 {
                        WorkerRole::OnlySmallCycleTx
                    } else {
                        WorkerRole::SubmitTimeFirst
                    };
                    let (child_tx, child_rx) = watch::channel(ChunkCommand::Resume);
                    (
                        child_tx,
                        Worker::new(
                            service.clone(),
                            Arc::clone(&tasks),
                            child_rx,
                            signal_exit.clone(),
                            role,
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
                info!("send worker command failed, error: {}", err);
            }
        }
    }

    fn spawn_worker(
        &mut self,
        worker_id: usize,
        exit_tx: mpsc::UnboundedSender<(usize, WorkerExit)>,
    ) {
        let Some(worker) = self
            .workers
            .get(worker_id)
            .map(|(_, worker)| worker.clone())
        else {
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
            let h = w.1.clone().start(worker_id, worker_exit_tx.clone());
            join_handles.push(Some(h));
        }
        self.join_handles.replace(join_handles);
        loop {
            tokio::select! {
                _ = self.signal_exit.cancelled() => {
                    info!("TxPool chunk_command service received exit signal, exit now");
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

fn panic_payload_to_string(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}
