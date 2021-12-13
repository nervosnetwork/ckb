use crate::component::chunk::Entry;
use crate::component::entry::TxEntry;
use crate::try_or_return_with_snapshot;
use crate::{error::Reject, service::TxPoolService};
use ckb_async_runtime::Handle;
use ckb_channel::{select, Receiver};
use ckb_error::Error;
use ckb_snapshot::Snapshot;
use ckb_store::ChainStore;
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::core::Cycle;
use ckb_verification::{
    cache::{CacheEntry, Completed},
    ContextualWithoutScriptTransactionVerifier, ScriptError, ScriptVerifier, ScriptVerifyResult,
    ScriptVerifyState, TimeRelativeTransactionVerifier, TxVerifyEnv,
};
use std::convert::TryInto;
use std::sync::Arc;

const MIN_STEP_CYCLE: Cycle = 10_000_000;

type Stop = bool;

#[derive(Eq, PartialEq, Clone, Debug)]
pub(crate) enum ProcessState {
    Interrupt,
    Normal,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub(crate) enum Command {
    Suspend,
    Continue,
    Stop,
}

pub(crate) struct TxChunkProcess {
    service: TxPoolService,
    handle: Handle,
    recv: Receiver<Command>,
    p_state: ProcessState,
}

impl TxChunkProcess {
    pub fn new(service: TxPoolService, handle: Handle, recv: Receiver<Command>) -> Self {
        TxChunkProcess {
            service,
            handle,
            recv,
            p_state: ProcessState::Normal,
        }
    }

    pub fn run(&mut self) {
        loop {
            select! {
                recv(self.recv) -> res => {
                    match res {
                        Ok(cmd) => match cmd {
                            Command::Continue => {
                                self.p_state = ProcessState::Normal;
                                let stop = self.try_process();
                                if stop {
                                    break;
                                }
                            }
                            Command::Suspend => self.p_state = ProcessState::Interrupt,
                            Command::Stop => break,
                        },
                        Err(_) => {
                            break
                        },
                    }
                }
                default(std::time::Duration::from_micros(1500)) => {
                    if matches!(self.p_state, ProcessState::Normal) {
                        let stop = self.try_process();
                        if stop {
                            break;
                        }
                    }
                }
            }
        }
    }

    fn try_process(&mut self) -> Stop {
        match self.handle.block_on(self.get_front()) {
            Some(entry) => self.process(entry),
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

    fn process(&mut self, entry: Entry) -> Stop {
        let (ret, snapshot) = self
            .process_inner(entry.clone())
            .expect("process_inner can not return None");
        ret.unwrap_or_else(|e| {
            self.handle.block_on(self.service.after_process(
                entry.tx,
                entry.remote,
                &snapshot,
                &Err(e),
            ));
            self.handle.block_on(self.remove_front());
            false
        })
    }

    fn process_inner(&mut self, entry: Entry) -> Option<(Result<Stop, Reject>, Arc<Snapshot>)> {
        let Entry { tx, remote } = entry;
        let tx_hash = tx.hash();

        let (ret, snapshot) = self.handle.block_on(self.service.pre_check(&tx));
        let (tip_hash, rtx, status, fee, tx_size) = try_or_return_with_snapshot!(ret, snapshot);

        let cached = self
            .handle
            .block_on(self.service.fetch_tx_verify_cache(&tx_hash));

        let tip_header = snapshot.tip_header();
        let consensus = snapshot.cloned_consensus();

        let tx_env = TxVerifyEnv::new_submit(tip_header);
        let mut init_snap = None;

        if let Some(ref cached) = cached {
            match cached {
                CacheEntry::Completed(completed) => {
                    let ret = TimeRelativeTransactionVerifier::new(
                        &rtx,
                        &consensus,
                        snapshot.as_ref(),
                        &tx_env,
                    )
                    .verify()
                    .map(|_| *completed)
                    .map_err(Reject::Verification);
                    let completed = try_or_return_with_snapshot!(ret, snapshot);

                    let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
                    let (ret, submit_snapshot) = self.handle.block_on(
                        self.service
                            .submit_entry(completed, tip_hash, entry, status),
                    );
                    try_or_return_with_snapshot!(ret, submit_snapshot);
                    self.handle.block_on(self.service.after_process(
                        tx,
                        remote,
                        &submit_snapshot,
                        &Ok(completed),
                    ));
                    self.handle.block_on(self.remove_front());
                    return Some((Ok(false), submit_snapshot));
                }
                CacheEntry::Suspended(suspended) => {
                    init_snap = Some(Arc::clone(&suspended.snap));
                }
            }
        }

        let cloned_snapshot = Arc::clone(&snapshot);
        let data_loader = cloned_snapshot.as_data_provider();
        let ret = ContextualWithoutScriptTransactionVerifier::new(
            &rtx,
            &consensus,
            &data_loader,
            &tx_env,
        )
        .verify()
        .map_err(Reject::Verification);
        let fee = try_or_return_with_snapshot!(ret, snapshot);
        let script_verifier = ScriptVerifier::new(&rtx, &consensus, &data_loader, &tx_env);
        let mut tmp_state: Option<ScriptVerifyState> = None;

        let max_cycles = if let Some((declared_cycle, _peer)) = remote {
            declared_cycle
        } else {
            consensus.max_block_cycles()
        };

        let completed: Completed = loop {
            // Should get here until there is no command, otherwise there maybe have a very large delay
            while let Ok(cmd) = self.recv.try_recv() {
                match cmd {
                    Command::Suspend => {
                        let state = tmp_state.take();
                        if let Some(state) = state {
                            let ret = state.try_into().map_err(Reject::Verification);
                            let snap = try_or_return_with_snapshot!(ret, snapshot);
                            let txs_verify_cache = Arc::clone(&self.service.txs_verify_cache);
                            self.handle.block_on(async move {
                                let mut guard = txs_verify_cache.write().await;
                                guard.put(tx_hash, CacheEntry::suspended(Arc::new(snap), fee));
                            })
                        }
                        self.p_state = ProcessState::Interrupt;
                        return Some((Ok(false), snapshot));
                    }
                    Command::Stop => {
                        return Some((Ok(true), snapshot));
                    }
                    Command::Continue => {
                        self.p_state = ProcessState::Normal;
                    }
                }
            }

            let mut last_step = false;
            let ret = if let Some(ref snap) = init_snap {
                if snap.current_cycles > max_cycles {
                    let error =
                        exceeded_maximum_cycles_error(&script_verifier, max_cycles, snap.current);
                    return Some((Err(Reject::Verification(error)), snapshot));
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
                    return Some((Err(Reject::Verification(error)), snapshot));
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
                script_verifier.resume_from_state(state, limit_cycles)
            } else {
                script_verifier.resumable_verify(MIN_STEP_CYCLE)
            }
            .map_err(Reject::Verification);

            let ret = try_or_return_with_snapshot!(ret, snapshot);

            match ret {
                ScriptVerifyResult::Completed(cycles) => {
                    break Completed { cycles, fee };
                }
                ScriptVerifyResult::Suspended(state) => {
                    if last_step {
                        let error = exceeded_maximum_cycles_error(
                            &script_verifier,
                            max_cycles,
                            state.current,
                        );
                        return Some((Err(Reject::Verification(error)), snapshot));
                    }
                    tmp_state = Some(state);
                }
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

        let entry = TxEntry::new(rtx.clone(), completed.cycles, fee, tx_size);
        let (ret, submit_snapshot) = self.handle.block_on(
            self.service
                .submit_entry(completed, tip_hash, entry, status),
        );
        try_or_return_with_snapshot!(ret, snapshot);

        self.handle.block_on(self.service.after_process(
            tx,
            remote,
            &submit_snapshot,
            &Ok(completed),
        ));

        self.handle.block_on(self.remove_front());

        let txs_verify_cache = Arc::clone(&self.service.txs_verify_cache);
        self.handle.block_on(async move {
            let mut guard = txs_verify_cache.write().await;
            guard.put(tx_hash, CacheEntry::Completed(completed));
        });

        Some((Ok(false), submit_snapshot))
    }
}

fn exceeded_maximum_cycles_error<DL: CellDataProvider + HeaderProvider>(
    verifier: &ScriptVerifier<'_, DL>,
    max_cycles: Cycle,
    current: usize,
) -> Error {
    verifier
        .inner()
        .groups()
        .nth(current)
        .map(|(_hash, group)| ScriptError::ExceededMaximumCycles(max_cycles).source(group))
        .unwrap_or_else(|| {
            ScriptError::VMInternalError(format!("suspended state group missing {:?}", current))
                .unknown_source()
        })
        .into()
}
