use crate::component::entry::TxEntry;
use crate::try_or_return_with_snapshot;

use crate::{error::Reject, service::TxPoolService};
use ckb_channel::Receiver;
use ckb_stop_handler::CancellationToken;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

use crate::verify_queue::{self, Entry, VerifyQueue};

use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_logger::info;
use ckb_snapshot::Snapshot;
use ckb_store::data_loader_wrapper::AsDataLoader;
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::{
    core::{cell::ResolvedTransaction, Cycle},
    packed::Byte32,
};
use ckb_verification::cache::TxVerificationCache;
use ckb_verification::{
    cache::{CacheEntry, Completed},
    ContextualWithoutScriptTransactionVerifier, DaoScriptSizeVerifier, ScriptError, ScriptVerifier,
    ScriptVerifyResult, ScriptVerifyState, TimeRelativeTransactionVerifier, TransactionSnapshot,
    TxVerifyEnv,
};
use tokio::task::block_in_place;

type Stop = bool;

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum ChunkCommand {
    Suspend,
    Resume,
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum VerifyNotify {
    Start {
        short_id: String,
    },
    Done {
        short_id: String,
        result: Option<(Result<Stop, Reject>, Arc<Snapshot>)>,
    },
}

enum State {
    Stopped,
    //Suspended(Arc<TransactionSnapshot>),
    Completed(Cycle),
}

struct Worker {
    tasks: Arc<RwLock<VerifyQueue>>,
    inbox: Receiver<ChunkCommand>,
    outbox: UnboundedSender<VerifyNotify>,
    service: TxPoolService,
    exit_signal: CancellationToken,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            inbox: self.inbox.clone(),
            outbox: self.outbox.clone(),
            service: self.service.clone(),
            exit_signal: self.exit_signal.clone(),
        }
    }
}

impl Worker {
    pub fn new(
        service: TxPoolService,
        tasks: Arc<RwLock<VerifyQueue>>,
        inbox: Receiver<ChunkCommand>,
        outbox: UnboundedSender<VerifyNotify>,
        exit_signal: CancellationToken,
    ) -> Self {
        Worker {
            service,
            tasks,
            inbox,
            outbox,
            exit_signal,
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
                    let res = self.run_verify_tx(entry).await;
                    self.outbox
                        .send(VerifyNotify::Done {
                            short_id: entry.tx.proposal_short_id().to_string(),
                            result: res,
                        })
                        .unwrap();
                } else {
                    // sleep for 100 ms
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        })
    }

    // fn run_verify_tx(&self, entry: &Entry) {
    //     let short_id = entry.tx.proposal_short_id();
    //     self.outbox
    //         .send(VerifyNotify::Start {
    //             short_id: short_id.to_string(),
    //         })
    //         .unwrap();
    // }

    async fn run_verify_tx(
        &mut self,
        entry: Entry,
    ) -> Option<(Result<Stop, Reject>, Arc<Snapshot>)> {
        let Entry { tx, remote } = entry;
        let tx_hash = tx.hash();

        let (ret, snapshot) = self.service.pre_check(&tx).await;
        let (tip_hash, rtx, status, fee, tx_size) = try_or_return_with_snapshot!(ret, snapshot);

        let cached = self.service.fetch_tx_verify_cache(&tx_hash).await;

        let tip_header = snapshot.tip_header();
        let consensus = snapshot.cloned_consensus();

        let tx_env = Arc::new(TxVerifyEnv::new_submit(tip_header));
        let mut init_snap = None;

        if let Some(ref cached) = cached {
            match cached {
                CacheEntry::Completed(completed) => {
                    let ret = TimeRelativeTransactionVerifier::new(
                        Arc::clone(&rtx),
                        Arc::clone(&consensus),
                        snapshot.as_data_loader(),
                        Arc::clone(&tx_env),
                    )
                    .verify()
                    .map(|_| *completed)
                    .map_err(Reject::Verification);
                    let completed = try_or_return_with_snapshot!(ret, snapshot);

                    let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
                    let (ret, submit_snapshot) =
                        self.service.submit_entry(tip_hash, entry, status).await;
                    try_or_return_with_snapshot!(ret, submit_snapshot);
                    // self.service
                    //     .after_process(tx, remote, &submit_snapshot, &Ok(completed))
                    //     .await;
                    // self.remove_front().await;
                    return Some((Ok(false), submit_snapshot));
                }
                CacheEntry::Suspended(suspended) => {
                    init_snap = Some(Arc::clone(&suspended.snap));
                }
            }
        }

        let cloned_snapshot = Arc::clone(&snapshot);
        let data_loader = cloned_snapshot.as_data_loader();
        let ret = ContextualWithoutScriptTransactionVerifier::new(
            Arc::clone(&rtx),
            Arc::clone(&consensus),
            data_loader.clone(),
            Arc::clone(&tx_env),
        )
        .verify()
        .and_then(|result| {
            DaoScriptSizeVerifier::new(
                Arc::clone(&rtx),
                Arc::clone(&consensus),
                data_loader.clone(),
            )
            .verify()?;
            Ok(result)
        })
        .map_err(Reject::Verification);
        let fee = try_or_return_with_snapshot!(ret, snapshot);

        let max_cycles = if let Some((declared_cycle, _peer)) = remote {
            declared_cycle
        } else {
            consensus.max_block_cycles()
        };

        let ret = self.loop_resume(
            Arc::clone(&rtx),
            data_loader,
            init_snap,
            max_cycles,
            Arc::clone(&consensus),
            Arc::clone(&tx_env),
        );
        let state = try_or_return_with_snapshot!(ret, snapshot);

        let completed: Completed = match state {
            // verify failed
            State::Stopped => return Some((Ok(true), snapshot)),
            State::Completed(cycles) => Completed { cycles, fee },
        };
        if let Some((declared_cycle, _peer)) = remote {
            if declared_cycle != completed.cycles {
                return Some((
                    Err(Reject::DeclaredWrongCycles(
                        declared_cycle,
                        completed.cycles,
                    )),
                    snapshot,
                ));
            }
        }
        // verify passed
        return Some((Ok(false), snapshot));

        //let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
        //let (ret, submit_snapshot) = self.service.submit_entry(tip_hash, entry, status).await;
        //try_or_return_with_snapshot!(ret, snapshot);

        // self.service.notify_block_assembler(status).await;

        // self.service
        //     .after_process(tx, remote, &submit_snapshot, &Ok(completed))
        //     .await;

        // self.remove_front().await;

        // update_cache(
        //     Arc::clone(&self.service.txs_verify_cache),
        //     tx_hash,
        //     CacheEntry::Completed(completed),
        // )
        // .await;

        // Some((Ok(false), submit_snapshot))
    }

    fn loop_resume<
        DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    >(
        &mut self,
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        mut init_snap: Option<Arc<TransactionSnapshot>>,
        max_cycles: Cycle,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
    ) -> Result<State, Reject> {
        let script_verifier = ScriptVerifier::new(rtx, data_loader, consensus, tx_env);
        let CYCLE_LIMIT = 1000000000;
        script_verifier.resumable_verify_with_signal(CYCLE_LIMIT)?;
    }
}

pub(crate) struct VerifyMgr {
    workers: Vec<(ckb_channel::Sender<ChunkCommand>, Worker)>,
    worker_notify: UnboundedReceiver<VerifyNotify>,
    join_handles: Option<Vec<JoinHandle<()>>>,
    pub chunk_rx: watch::Receiver<ChunkCommand>,
    pub current_state: ChunkCommand,
    pub signal_exit: CancellationToken,
    pub verify_queue: Arc<RwLock<VerifyQueue>>,
    pub service: TxPoolService,
}

impl VerifyMgr {
    pub fn new(
        service: TxPoolService,
        chunk_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
        //verify_queue: Arc<RwLock<VerifyQueue>>,
    ) -> Self {
        let (notify_tx, notify_rx) = unbounded_channel::<VerifyNotify>();
        let verify_queue = Arc::new(RwLock::new(VerifyQueue::new()));
        let workers: Vec<_> = (0..4)
            .map({
                let tasks = Arc::clone(&verify_queue);
                move |_| {
                    let (command_tx, command_rx) = ckb_channel::unbounded();
                    let worker = Worker::new(
                        service.clone(),
                        Arc::clone(&tasks),
                        command_rx,
                        notify_tx.clone(),
                        signal_exit.clone(),
                    );
                    (command_tx, worker)
                }
            })
            .collect();
        Self {
            service,
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
        let mut interval = tokio::time::interval(Duration::from_micros(1000));
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
