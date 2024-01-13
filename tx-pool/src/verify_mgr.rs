use crate::component::entry::TxEntry;
use crate::try_or_return_with_snapshot;

use crate::{error::Reject, service::TxPoolService};
use ckb_script::{ChunkCommand, VerifyResult};
use ckb_stop_handler::CancellationToken;
use ckb_types::packed::Byte32;
use ckb_verification::cache::TxVerificationCache;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

use crate::verify_queue::{Entry, VerifyQueue};

use ckb_chain_spec::consensus::Consensus;
use ckb_snapshot::Snapshot;
use ckb_store::data_loader_wrapper::AsDataLoader;
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::{cell::ResolvedTransaction, Cycle};
//use ckb_verification::cache::TxVerificationCache;
use ckb_verification::{
    cache::{CacheEntry, Completed},
    ContextualWithoutScriptTransactionVerifier, DaoScriptSizeVerifier, ScriptVerifier,
    TimeRelativeTransactionVerifier, TxVerifyEnv,
};

type Stop = bool;

#[derive(Clone, Debug)]
pub enum VerifyNotify {
    Done {
        short_id: String,
        result: (Result<Stop, Reject>, Arc<Snapshot>),
    },
}

#[derive(Clone, Debug)]
enum State {
    //Stopped,
    //Suspended(Arc<TransactionSnapshot>),
    Completed(Cycle),
}

struct Worker {
    tasks: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    queue_rx: watch::Receiver<usize>,
    outbox: UnboundedSender<VerifyNotify>,
    service: TxPoolService,
    exit_signal: CancellationToken,
}

impl Clone for Worker {
    fn clone(&self) -> Self {
        Self {
            tasks: Arc::clone(&self.tasks),
            command_rx: self.command_rx.clone(),
            queue_rx: self.queue_rx.clone(),
            exit_signal: self.exit_signal.clone(),
            outbox: self.outbox.clone(),
            service: self.service.clone(),
        }
    }
}

impl Worker {
    pub fn new(
        service: TxPoolService,
        tasks: Arc<RwLock<VerifyQueue>>,
        command_rx: watch::Receiver<ChunkCommand>,
        queue_rx: watch::Receiver<usize>,
        outbox: UnboundedSender<VerifyNotify>,
        exit_signal: CancellationToken,
    ) -> Self {
        Worker {
            service,
            tasks,
            command_rx,
            queue_rx,
            outbox,
            exit_signal,
        }
    }

    /// start handle tasks
    pub fn start(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = self.exit_signal.cancelled() => {
                        break;
                    }
                    _ = interval.tick() => {
                        self.process_inner().await;
                    }
                    _ = self.queue_rx.changed() => {
                        self.process_inner().await;
                    }
                }
            }
        })
    }

    async fn process_inner(&mut self) {
        if self.tasks.read().await.get_first().is_none() {
            return;
        }
        // pick a entry to run verify
        let entry = match self.tasks.write().await.pop_first() {
            Some(entry) => entry,
            None => return,
        };
        eprintln!("begin to process: {:?}", entry);
        let short_id = entry.tx.proposal_short_id().to_string();
        let (res, snapshot) = self
            .run_verify_tx(entry.clone())
            .await
            .expect("run_verify_tx failed");
        eprintln!("process done: {:?}", res);
        self.outbox
            .send(VerifyNotify::Done {
                short_id,
                result: (res.clone(), snapshot.clone()),
            })
            .unwrap();

        match res {
            Err(e) => {
                self.service
                    .after_process(entry.tx, entry.remote, &snapshot, &Err(e))
                    .await;
            }
            _ => {}
        }
    }

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

        eprintln!("run_verify_tx cached: {:?}", cached);
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
                    self.service
                        .after_process(tx, remote, &submit_snapshot, &Ok(completed))
                        .await;
                    return Some((Ok(false), submit_snapshot));
                }
                CacheEntry::Suspended(_suspended) => {
                    eprintln!("not expected suspended: {:?}", cached);
                    //panic!("not expected");
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

        eprintln!("begin to loop: {:?}", rtx);
        let ret = self
            .loop_resume(
                Arc::clone(&rtx),
                data_loader,
                max_cycles,
                Arc::clone(&consensus),
                Arc::clone(&tx_env),
            )
            .await;
        eprintln!("loop done: {:?}", ret);
        let state = try_or_return_with_snapshot!(ret, snapshot);

        let completed: Completed = match state {
            // verify failed
            // State::Stopped => return Some((Ok(true), snapshot)),
            State::Completed(cycles) => Completed { cycles, fee },
        };
        eprintln!("completed: {:?}", completed);
        eprintln!("remote: {:?}", remote);
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

        let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
        let (ret, submit_snapshot) = self.service.submit_entry(tip_hash, entry, status).await;
        try_or_return_with_snapshot!(ret, snapshot);

        self.service.notify_block_assembler(status).await;

        self.service
            .after_process(tx, remote, &submit_snapshot, &Ok(completed))
            .await;

        update_cache(
            Arc::clone(&self.service.txs_verify_cache),
            tx_hash,
            CacheEntry::Completed(completed),
        )
        .await;

        return Some((Ok(false), snapshot));
    }

    async fn loop_resume<
        DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    >(
        &mut self,
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        max_cycles: Cycle,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
    ) -> Result<State, Reject> {
        let script_verifier = ScriptVerifier::new(rtx, data_loader, consensus, tx_env);
        let res = script_verifier
            .resumable_verify_with_signal(max_cycles, &mut self.command_rx)
            .await
            .map_err(Reject::Verification)?;
        match res {
            VerifyResult::Completed(cycles) => {
                return Ok(State::Completed(cycles));
            }
            VerifyResult::Suspended(_) => {
                panic!("not expected");
            }
        }
    }
}

async fn update_cache(cache: Arc<RwLock<TxVerificationCache>>, tx_hash: Byte32, entry: CacheEntry) {
    let mut guard = cache.write().await;
    guard.put(tx_hash, entry);
}

pub(crate) struct VerifyMgr {
    workers: Vec<Worker>,
    worker_notify: UnboundedReceiver<VerifyNotify>,
    join_handles: Option<Vec<JoinHandle<()>>>,
    signal_exit: CancellationToken,
}

impl VerifyMgr {
    pub fn new(
        service: TxPoolService,
        chunk_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        queue_rx: watch::Receiver<usize>,
    ) -> Self {
        let (notify_tx, notify_rx) = unbounded_channel::<VerifyNotify>();
        let workers: Vec<_> = (0..4)
            .map({
                let tasks = Arc::clone(&verify_queue);
                let command_rx = chunk_rx.clone();
                let signal_exit = signal_exit.clone();
                move |_| {
                    let worker = Worker::new(
                        service.clone(),
                        Arc::clone(&tasks),
                        command_rx.clone(),
                        queue_rx.clone(),
                        notify_tx.clone(),
                        signal_exit.clone(),
                    );
                    worker
                }
            })
            .collect();
        Self {
            workers,
            worker_notify: notify_rx,
            join_handles: None,
            signal_exit,
        }
    }

    fn start_workers(&mut self) {
        let mut join_handles = Vec::new();
        for w in self.workers.iter_mut() {
            let h = w.clone().start();
            join_handles.push(h);
        }
        self.join_handles.replace(join_handles);
    }

    async fn start_loop(&mut self) {
        let mut interval = tokio::time::interval(Duration::from_micros(1000));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = self.signal_exit.cancelled() => {
                    break;
                },
                res = self.worker_notify.recv() => {
                    eprintln!("res: {:?}", res);
                }
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
