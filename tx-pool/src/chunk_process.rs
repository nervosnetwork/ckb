use crate::component::chunk::Entry;
use crate::component::entry::TxEntry;
use crate::try_or_return_with_snapshot;
use crate::{error::Reject, service::TxPoolService};
use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_logger::debug;
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
    ContextualWithoutScriptTransactionVerifier, ScriptError, ScriptVerifier, ScriptVerifyResult,
    ScriptVerifyState, TimeRelativeTransactionVerifier, TransactionSnapshot, TxVerifyEnv,
};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::sync::RwLock;
use tokio::task::block_in_place;
use tokio_util::sync::CancellationToken;

const MIN_STEP_CYCLE: Cycle = 10_000_000;

type Stop = bool;

#[derive(Eq, PartialEq, Clone, Debug)]
pub(crate) enum ChunkCommand {
    Suspend,
    Resume,
}

enum State {
    Stopped,
    Suspended(Arc<TransactionSnapshot>),
    Completed(Cycle),
}

pub(crate) struct ChunkProcess {
    service: TxPoolService,
    recv: watch::Receiver<ChunkCommand>,
    current_state: ChunkCommand,
    signal: CancellationToken,
}

impl ChunkProcess {
    pub fn new(
        service: TxPoolService,
        recv: watch::Receiver<ChunkCommand>,
        signal: CancellationToken,
    ) -> Self {
        ChunkProcess {
            service,
            recv,
            signal,
            current_state: ChunkCommand::Resume,
        }
    }

    pub async fn run(mut self) {
        let mut interval = tokio::time::interval(std::time::Duration::from_micros(1500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = self.recv.changed() => {
                    self.current_state = self.recv.borrow().to_owned();
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
                _ = self.signal.cancelled() => {
                    debug!("TxPool received exit signal, exit now");
                    break
                },
                else => break,
            }
        }
    }

    async fn try_process(&mut self) -> Stop {
        match self.get_front().await {
            Some(entry) => self.process(entry).await,
            None => false,
        }
    }

    async fn get_front(&self) -> Option<Entry> {
        self.service.chunk.write().await.pop_front()
    }

    async fn remove_front(&self) {
        let mut guard = self.service.chunk.write().await;
        guard.clean_front();
    }

    async fn process(&mut self, entry: Entry) -> Stop {
        let (ret, snapshot) = self
            .process_inner(entry.clone())
            .await
            .expect("process_inner can not return None");

        match ret {
            Ok(stop) => stop,
            Err(e) => {
                self.service
                    .after_process(entry.tx, entry.remote, &snapshot, &Err(e))
                    .await;
                self.remove_front().await;
                false
            }
        }
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
        let mut tmp_state: Option<ScriptVerifyState> = None;

        let completed: Cycle = loop {
            if self.signal.is_cancelled() {
                return Ok(State::Stopped);
            }
            if self.recv.has_changed().unwrap_or(false) {
                self.current_state = self.recv.borrow_and_update().to_owned();
            }

            if matches!(self.current_state, ChunkCommand::Suspend) {
                let state = tmp_state.take();

                if let Some(state) = state {
                    let snap = state.try_into().map_err(Reject::Verification)?;
                    return Ok(State::Suspended(Arc::new(snap)));
                }
            }

            let mut last_step = false;
            let ret = if let Some(ref snap) = init_snap {
                if snap.current_cycles > max_cycles {
                    let error =
                        exceeded_maximum_cycles_error(&script_verifier, max_cycles, snap.current);
                    return Err(Reject::Verification(error));
                }

                let (limit_cycles, last) = snap.next_limit_cycles(MIN_STEP_CYCLE, max_cycles);
                last_step = last;
                let ret = script_verifier.resume_from_snap(snap, limit_cycles);
                init_snap = None;
                ret
            } else if let Some(state) = tmp_state {
                // once we start loop from state, clean tmp snap.
                init_snap = None;
                if state.current_cycles > max_cycles {
                    let error =
                        exceeded_maximum_cycles_error(&script_verifier, max_cycles, state.current);
                    return Err(Reject::Verification(error));
                }

                // next_limit_cycles
                // let remain = max_cycles - self.current_cycles;
                // let next_limit = self.limit_cycles + step_cycles;

                // if next_limit < remain {
                //     (next_limit, false)
                // } else {
                //     (remain, true)
                // }
                let (limit_cycles, last) = state.next_limit_cycles(MIN_STEP_CYCLE, max_cycles);
                last_step = last;

                block_in_place(|| script_verifier.resume_from_state(state, limit_cycles))
            } else {
                block_in_place(|| script_verifier.resumable_verify(MIN_STEP_CYCLE))
            }
            .map_err(Reject::Verification)?;

            match ret {
                ScriptVerifyResult::Completed(cycles) => {
                    break cycles;
                }
                ScriptVerifyResult::Suspended(state) => {
                    if last_step {
                        let error = exceeded_maximum_cycles_error(
                            &script_verifier,
                            max_cycles,
                            state.current,
                        );
                        return Err(Reject::Verification(error));
                    }
                    tmp_state = Some(state);
                }
            }
        };

        Ok(State::Completed(completed))
    }

    async fn process_inner(
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
                    self.service
                        .after_process(tx, remote, &submit_snapshot, &Ok(completed))
                        .await;
                    self.remove_front().await;
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
            State::Stopped => return Some((Ok(true), snapshot)),
            State::Suspended(snap) => {
                update_cache(
                    Arc::clone(&self.service.txs_verify_cache),
                    tx_hash,
                    CacheEntry::suspended(snap, fee),
                )
                .await;
                return Some((Ok(false), snapshot));
            }
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

        let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
        let (ret, submit_snapshot) = self.service.submit_entry(tip_hash, entry, status).await;
        try_or_return_with_snapshot!(ret, snapshot);

        self.service.notify_block_assembler(status).await;

        self.service
            .after_process(tx, remote, &submit_snapshot, &Ok(completed))
            .await;

        self.remove_front().await;

        update_cache(
            Arc::clone(&self.service.txs_verify_cache),
            tx_hash,
            CacheEntry::Completed(completed),
        )
        .await;

        Some((Ok(false), submit_snapshot))
    }
}

fn exceeded_maximum_cycles_error<
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
>(
    verifier: &ScriptVerifier<DL>,
    max_cycles: Cycle,
    current: usize,
) -> Error {
    verifier
        .inner()
        .groups()
        .nth(current)
        .map(|(_hash, group)| ScriptError::ExceededMaximumCycles(max_cycles).source(group))
        .unwrap_or_else(|| {
            ScriptError::VMInternalError(format!("suspended state group missing {current:?}"))
                .unknown_source()
        })
        .into()
}

async fn update_cache(cache: Arc<RwLock<TxVerificationCache>>, tx_hash: Byte32, entry: CacheEntry) {
    let mut guard = cache.write().await;
    guard.put(tx_hash, entry);
}
