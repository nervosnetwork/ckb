extern crate num_cpus;
use crate::component::verify_queue::VerifyQueue;
use crate::service::TxPoolService;
use ckb_logger::info;
use ckb_script::ChunkCommand;
use ckb_stop_handler::CancellationToken;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

struct Worker {
    tasks: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    service: TxPoolService,
    exit_signal: CancellationToken,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            command_rx: self.command_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            service: self.service.clone(),
        }
    }
}

impl Worker {
    pub fn new(
        service: TxPoolService,
        tasks: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        exit_signal: CancellationToken,
    ) -> Self {
        Worker {
            service,
            tasks,
            command_rx,
            exit_signal,
        }
    }

    pub async fn start(mut self) -> JoinHandle<()> {
        // use a channel to receive the queue change event makes sure the worker
        // know immediately when the queue is changed, otherwise we may have a delay of `interval`
        let mut queue_rx = self.tasks.read().await.subscribe();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let try_pick = tokio::select! {
                    _ = self.exit_signal.cancelled() => {
                        break;
                    }
                    _ = self.command_rx.changed() => {
                        *self.command_rx.borrow() == ChunkCommand::Resume
                    }
                    _ = queue_rx.changed() => {
                        true
                    }
                    _ = interval.tick() => {
                        true
                    }
                };
                if try_pick {
                    self.process_inner().await;
                }
            }
        })
    }

    async fn process_inner(&mut self) {
        if self.tasks.read().await.peek().is_none() {
            return;
        }
        // pick a entry to run verify
        let entry = match self.tasks.write().await.pop_first() {
            Some(entry) => entry,
            None => return,
        };

        let (res, snapshot) = self
            .service
            .run_verify_tx(entry.clone(), &mut self.command_rx)
            .await
            .expect("run_verify_tx failed");

        self.service
            .after_process(entry.tx, entry.remote, &snapshot, &res)
            .await;
    }
}

pub(crate) struct VerifyMgr {
    workers: Vec<(watch::Sender<ChunkCommand>, Worker)>,
    join_handles: Option<Vec<JoinHandle<()>>>,
    signal_exit: CancellationToken,
    command_rx: watch::Receiver<ChunkCommand>,
}

impl VerifyMgr {
    pub fn new(
        service: TxPoolService,
        command_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
    ) -> Self {
        let workers: Vec<_> = (0..num_cpus::get())
            .map({
                let tasks = Arc::clone(&service.verify_queue);
                let signal_exit = signal_exit.clone();
                move |_| {
                    let (child_tx, child_rx) = watch::channel(ChunkCommand::Resume);
                    (
                        child_tx,
                        Worker::new(
                            service.clone(),
                            Arc::clone(&tasks),
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
                info!("send worker command failed, error: {}", err);
            }
        }
    }

    async fn start_loop(&mut self) {
        let mut join_handles = Vec::new();
        for w in self.workers.iter_mut() {
            let h = w.1.clone().start().await;
            join_handles.push(h);
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
                }
            }
        }
        if let Some(jh) = self.join_handles.take() {
            for h in jh {
                h.await.expect("Worker thread panic");
            }
        }
        info!("TxPool verify_mgr service exited");
    }

    pub async fn run(&mut self) {
        self.start_loop().await;
    }
}
