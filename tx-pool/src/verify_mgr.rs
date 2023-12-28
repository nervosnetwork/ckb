use ckb_channel::Receiver;
use ckb_stop_handler::CancellationToken;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

use crate::verify_queue::{Entry, VerifyQueue};

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum ChunkCommand {
    Suspend,
    Resume,
    Shutdown,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum VerifyNotify {
    Start { short_id: String },
    Done { short_id: String },
}

struct Worker {
    tasks: Arc<RwLock<VerifyQueue>>,
    inbox: Receiver<ChunkCommand>,
    outbox: UnboundedSender<VerifyNotify>,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            inbox: self.inbox.clone(),
            outbox: self.outbox.clone(),
        }
    }
}

impl Worker {
    pub fn new(
        tasks: Arc<RwLock<VerifyQueue>>,
        inbox: Receiver<ChunkCommand>,
        outbox: UnboundedSender<VerifyNotify>,
    ) -> Self {
        Worker {
            tasks,
            inbox,
            outbox,
        }
    }

    /// start handle tasks
    pub fn start(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut pause = false;
            loop {
                match self.inbox.try_recv() {
                    Ok(msg) => match msg {
                        ChunkCommand::Shutdown => {
                            break;
                        }
                        ChunkCommand::Suspend => {
                            pause = true;
                            continue;
                        }
                        ChunkCommand::Resume => {
                            pause = false;
                        }
                    },
                    Err(err) => {
                        if !err.is_empty() {
                            eprintln!("error: {:?}", err);
                            break;
                        }
                    }
                };

                if !pause {
                    if self.tasks.read().await.get_first().is_none() {
                        // sleep for 100 ms
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    // pick a entry to run verify
                    let entry = match self.tasks.write().await.pop_first() {
                        Some(entry) => entry,
                        None => continue,
                    };
                    self.run_verify_tx(&entry)
                } else {
                    // sleep for 100 ms
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        })
    }

    fn run_verify_tx(&self, entry: &Entry) {
        let short_id = entry.tx.proposal_short_id();
        self.outbox
            .send(VerifyNotify::Start {
                short_id: short_id.to_string(),
            })
            .unwrap();
    }
}

pub struct VerifyMgr {
    workers: Vec<(ckb_channel::Sender<ChunkCommand>, Worker)>,
    worker_notify: UnboundedReceiver<VerifyNotify>,
    join_handles: Option<Vec<JoinHandle<()>>>,
    pub chunk_rx: watch::Receiver<ChunkCommand>,
    pub current_state: ChunkCommand,
    pub signal_exit: CancellationToken,
    pub verify_queue: Arc<RwLock<VerifyQueue>>,
}

impl VerifyMgr {
    pub fn new(
        chunk_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
        verify_queue: Arc<RwLock<VerifyQueue>>,
    ) -> Self {
        let (notify_tx, notify_rx) = unbounded_channel::<VerifyNotify>();
        let workers: Vec<_> = (0..4)
            .map({
                let tasks = Arc::clone(&verify_queue);
                move |_| {
                    let (command_tx, command_rx) = ckb_channel::unbounded();
                    let worker = Worker::new(Arc::clone(&tasks), command_rx, notify_tx.clone());
                    (command_tx, worker)
                }
            })
            .collect();
        Self {
            workers,
            worker_notify: notify_rx,
            join_handles: None,
            chunk_rx,
            current_state: ChunkCommand::Resume,
            signal_exit,
            verify_queue,
        }
    }

    async fn send_command(&mut self, command: ChunkCommand) {
        eprintln!(
            "send workers {:?} command: {:?}",
            std::time::SystemTime::now(),
            command
        );
        for worker in self.workers.iter_mut() {
            worker.0.send(command.clone()).unwrap();
        }
    }

    fn start_workers(&mut self) {
        let mut join_handles = Vec::new();
        for w in self.workers.iter_mut() {
            let h = w.1.clone().start();
            join_handles.push(h);
        }
        self.join_handles.replace(join_handles);
    }

    async fn start_loop(&mut self) {
        let mut interval = tokio::time::interval(Duration::from_micros(1500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = self.chunk_rx.changed() => {
                    self.current_state = self.chunk_rx.borrow().to_owned();
                    self.send_command(self.current_state.clone()).await;
                },
                res = self.worker_notify.recv() => {
                    eprintln!("res: {:?}", res);
                }
                _ = self.signal_exit.cancelled() => {
                    self.send_command(ChunkCommand::Shutdown).await;
                    break;
                },
                _ = interval.tick() => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    pub async fn run(&mut self) {
        self.start_workers();
        self.start_loop().await;
    }
}
