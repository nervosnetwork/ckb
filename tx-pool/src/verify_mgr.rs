use ckb_stop_handler::CancellationToken;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot::Sender;
use tokio::sync::{mpsc, oneshot, watch, RwLock};
use tokio::task;

use crate::verify_queue::{Entry, VerifyQueue};

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum ChunkCommand {
    Suspend,
    Resume,
}
pub struct VerifyMgr {
    pub workers: Vec<(
        tokio::task::JoinHandle<()>,
        UnboundedSender<Entry>,
        Sender<()>,
        UnboundedReceiver<()>,
    )>,
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
        Self {
            workers: vec![],
            chunk_rx,
            current_state: ChunkCommand::Resume,
            signal_exit,
            verify_queue,
        }
    }

    pub async fn try_process(&mut self) -> bool {
        if let Some(entry) = self.verify_queue.write().await.get_first() {
            println!("entry: {:?}", entry);
            if self.workers.len() <= 10 {
                let (job_sender, job_receiver) = mpsc::unbounded_channel::<Entry>();
                let (exit_sender, exit_receiver) = oneshot::channel();
                let (res_sender, res_receiver) = mpsc::unbounded_channel::<()>();
                let mut worker = Worker::new(job_receiver, exit_receiver, res_sender);
                let handle = task::spawn(async move { worker.run().await });
                job_sender.send(entry).unwrap();
                self.workers
                    .push((handle, job_sender, exit_sender, res_receiver));
            } else {
                return false;
            }
            return true;
        }
        false
    }

    pub async fn run(&mut self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_micros(1500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = self.chunk_rx.changed() => {
                    self.current_state = self.chunk_rx.borrow().to_owned();
                    if matches!(self.current_state, ChunkCommand::Resume) {
                        let stop = self.try_process().await;
                        if stop {
                            break;
                        }
                    }
                },
                _ = interval.tick() => {
                    if matches!(self.current_state, ChunkCommand::Resume) {
                        let stop = self.try_process().await;
                        if stop {
                            break;
                        }
                    }
                },
                _ = self.signal_exit.cancelled() => {
                    break;
                },
                else => break,
            }
        }
    }
}

struct Worker {
    job_receiver: mpsc::UnboundedReceiver<Entry>,
    exit_receiver: oneshot::Receiver<()>,
    res_sender: mpsc::UnboundedSender<()>,
}

impl Worker {
    pub fn new(
        job_receiver: mpsc::UnboundedReceiver<Entry>,
        exit_receiver: oneshot::Receiver<()>,
        res_sender: mpsc::UnboundedSender<()>,
    ) -> Self {
        Self {
            job_receiver,
            exit_receiver,
            res_sender,
        }
    }

    pub async fn run(&mut self) {
        loop {
            tokio::select! {
                entry = self.job_receiver.recv() => {
                    if let Some(entry) = entry {
                        eprintln!("entry: {:?}", entry);
                        let _ = self.res_sender.send(());
                    } else {
                        break;
                    }
                }
                _ = &mut self.exit_receiver => {
                    break;
                }
            }
        }
    }
}
