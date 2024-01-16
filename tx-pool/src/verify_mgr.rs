extern crate num_cpus;
use crate::component::entry::TxEntry;
use crate::try_or_return_with_snapshot;
use crate::verify_queue::{Entry, VerifyQueue};
use crate::{error::Reject, service::TxPoolService};
use ckb_chain_spec::consensus::Consensus;
use ckb_logger::info;
use ckb_script::{ChunkCommand, VerifyResult};
use ckb_snapshot::Snapshot;
use ckb_stop_handler::CancellationToken;
use ckb_store::data_loader_wrapper::AsDataLoader;
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::core::{cell::ResolvedTransaction, Cycle};
use ckb_verification::{
    cache::Completed, ContextualWithoutScriptTransactionVerifier, DaoScriptSizeVerifier,
    ScriptVerifier, TimeRelativeTransactionVerifier, TxVerifyEnv,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;

struct Worker {
    tasks: Arc<RwLock<VerifyQueue>>,
    command_rx: watch::Receiver<ChunkCommand>,
    queue_rx: watch::Receiver<usize>,
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
        exit_signal: CancellationToken,
    ) -> Self {
        Worker {
            service,
            tasks,
            command_rx,
            queue_rx,
            exit_signal,
        }
    }

    pub fn start(mut self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let try_pick = tokio::select! {
                    _ = self.exit_signal.cancelled() => {
                        break;
                    }
                    _ = self.queue_rx.changed() => {
                        true
                    }
                    _ = interval.tick() => {
                        true
                    }
                    _ = self.command_rx.changed() => {
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
        if self.command_rx.borrow().to_owned() == ChunkCommand::Suspend {
            return;
        }

        if self.tasks.read().await.get_first().is_none() {
            return;
        }
        // pick a entry to run verify
        let entry = match self.tasks.write().await.pop_first() {
            Some(entry) => entry,
            None => return,
        };

        let (res, snapshot) = self
            .run_verify_tx(entry.clone())
            .await
            .expect("run_verify_tx failed");

        match res {
            Ok(completed) => {
                self.service
                    .after_process(entry.tx, entry.remote, &snapshot, &Ok(completed))
                    .await;
            }
            Err(e) => {
                self.service
                    .after_process(entry.tx, entry.remote, &snapshot, &Err(e))
                    .await;
            }
        }
    }

    async fn run_verify_tx(
        &mut self,
        entry: Entry,
    ) -> Option<(Result<Completed, Reject>, Arc<Snapshot>)> {
        let Entry { tx, remote } = entry;
        let tx_hash = tx.hash();

        let (ret, snapshot) = self.service.pre_check(&tx).await;
        let (tip_hash, rtx, status, fee, tx_size) = try_or_return_with_snapshot!(ret, snapshot);

        let cached = self.service.fetch_tx_verify_cache(&tx_hash).await;

        let tip_header = snapshot.tip_header();
        let consensus = snapshot.cloned_consensus();

        let tx_env = Arc::new(TxVerifyEnv::new_submit(tip_header));

        if let Some(ref completed) = cached {
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
            let (ret, snapshot) = self.service.submit_entry(tip_hash, entry, status).await;
            try_or_return_with_snapshot!(ret, snapshot);
            return Some((Ok(completed), snapshot));
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

        let ret = self
            .loop_resume(
                Arc::clone(&rtx),
                data_loader,
                max_cycles,
                Arc::clone(&consensus),
                Arc::clone(&tx_env),
            )
            .await;
        let state = try_or_return_with_snapshot!(ret, snapshot);

        let completed: Completed = match state {
            VerifyResult::Completed(cycles) => Completed { cycles, fee },
            _ => {
                panic!("not expected");
            }
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
        let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
        let (ret, snapshot) = self.service.submit_entry(tip_hash, entry, status).await;
        try_or_return_with_snapshot!(ret, snapshot);

        self.service.notify_block_assembler(status).await;

        let mut guard = self.service.txs_verify_cache.write().await;
        guard.put(tx_hash, completed);

        Some((Ok(completed), snapshot))
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
    ) -> Result<VerifyResult, Reject> {
        let script_verifier = ScriptVerifier::new(rtx, data_loader, consensus, tx_env);
        let res = script_verifier
            .resumable_verify_with_signal(max_cycles, &mut self.command_rx)
            .await
            .map_err(Reject::Verification)?;
        Ok(res)
    }
}

pub(crate) struct VerifyMgr {
    workers: Vec<Worker>,
    join_handles: Option<Vec<JoinHandle<()>>>,
    signal_exit: CancellationToken,
    command_rx: watch::Receiver<ChunkCommand>,
}

impl VerifyMgr {
    pub fn new(
        service: TxPoolService,
        chunk_rx: watch::Receiver<ChunkCommand>,
        signal_exit: CancellationToken,
        verify_queue: Arc<RwLock<VerifyQueue>>,
        queue_rx: watch::Receiver<usize>,
    ) -> Self {
        let workers: Vec<_> = (0..num_cpus::get())
            .map({
                let tasks = Arc::clone(&verify_queue);
                let command_rx = chunk_rx.clone();
                let signal_exit = signal_exit.clone();
                move |_| {
                    Worker::new(
                        service.clone(),
                        Arc::clone(&tasks),
                        command_rx.clone(),
                        queue_rx.clone(),
                        signal_exit.clone(),
                    )
                }
            })
            .collect();
        Self {
            workers,
            join_handles: None,
            signal_exit,
            command_rx: chunk_rx,
        }
    }

    async fn start_loop(&mut self) {
        let mut join_handles = Vec::new();
        for w in self.workers.iter_mut() {
            let h = w.clone().start();
            join_handles.push(h);
        }
        self.join_handles.replace(join_handles);
        loop {
            tokio::select! {
                _ = self.signal_exit.cancelled() => {
                    info!("TxPool chunk_command service received exit signal, exit now");
                    break;
                },
                _ = self.command_rx.changed() => {
                    //eprintln!("command: {:?}", self.command_rx.borrow());
                }
            }
        }
    }

    pub async fn run(&mut self) {
        self.start_loop().await;
    }
}
