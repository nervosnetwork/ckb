use crate::component::chunk::Entry;
use crate::component::entry::TxEntry;
use crate::{error::Reject, service::TxPoolService};
use ckb_async_runtime::Handle;
use ckb_channel::{select, Receiver};
use ckb_error::Error;
use ckb_store::ChainStore;
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::{core::Cycle, packed::Byte32};
use ckb_verification::{
    cache::{CacheEntry, Completed},
    ContextualWithoutScriptTransactionVerifier, ScriptError, ScriptGroupType, ScriptVerifier,
    ScriptVerifyResult, ScriptVerifyState, TimeRelativeTransactionVerifier, TxVerifyEnv,
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
        self.process_inner(entry.clone()).unwrap_or_else(|e| {
            self.handle
                .block_on(self.service.after_process(entry.tx, entry.remote, &Err(e)));
            self.handle.block_on(self.remove_front());
            false
        })
    }

    fn process_inner(&mut self, entry: Entry) -> Result<Stop, Reject> {
        let Entry { tx, remote } = entry;
        let tx_hash = tx.hash();

        let (tip_hash, snapshot, rtx, status, fee, tx_size) =
            self.handle.block_on(self.service.pre_check(tx.clone()))?;
        let cached = self
            .handle
            .block_on(self.service.fetch_tx_verify_cache(&tx_hash));

        let tip_header = snapshot.tip_header();
        let consensus = snapshot.consensus();

        let tx_env = TxVerifyEnv::new_submit(tip_header);
        let mut init_snap = None;

        if let Some(ref cached) = cached {
            match cached {
                CacheEntry::Completed(completed) => {
                    let completed = TimeRelativeTransactionVerifier::new(
                        &rtx,
                        consensus,
                        snapshot.as_ref(),
                        &tx_env,
                    )
                    .verify()
                    .map(|_| *completed)
                    .map_err(Reject::Verification)?;

                    let entry = TxEntry::new(rtx, completed.cycles, fee, tx_size);
                    self.handle.block_on(
                        self.service
                            .submit_entry(completed, tip_hash, entry, status),
                    )?;
                    self.handle
                        .block_on(self.service.after_process(tx, remote, &Ok(completed)));
                    self.handle.block_on(self.remove_front());
                    return Ok(false);
                }
                CacheEntry::Suspended(suspended) => {
                    init_snap = Some(Arc::clone(&suspended.snap));
                }
            }
        }

        let data_loader = snapshot.as_data_provider();
        let fee =
            ContextualWithoutScriptTransactionVerifier::new(&rtx, consensus, &data_loader, &tx_env)
                .verify()
                .map_err(Reject::Verification)?;
        let script_verifier = ScriptVerifier::new(&rtx, consensus, &data_loader, &tx_env);
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
                            let snap = state.try_into().map_err(Reject::Verification)?;
                            let txs_verify_cache = Arc::clone(&self.service.txs_verify_cache);
                            self.handle.block_on(async move {
                                let mut guard = txs_verify_cache.write().await;
                                guard.put(tx_hash, CacheEntry::suspended(Arc::new(snap), fee));
                            })
                        }
                        self.p_state = ProcessState::Interrupt;
                        return Ok(false);
                    }
                    Command::Stop => {
                        return Ok(true);
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
                        exceeded_maximum_cycles_error(&script_verifier, max_cycles, &snap.current);
                    return Err(Reject::Verification(error));
                }
                let remain = max_cycles - snap.current_cycles;
                let step_cycles = snap.limit_cycles + MIN_STEP_CYCLE;

                let limit_cycles = if step_cycles < remain {
                    step_cycles
                } else {
                    last_step = true;
                    remain
                };
                let ret = script_verifier.resume_from_snap(snap, limit_cycles);
                init_snap = None;
                ret
            } else if let Some(state) = tmp_state {
                // once we start loop from state, clean tmp snap.
                init_snap = None;
                if state.current_cycles > max_cycles {
                    let error =
                        exceeded_maximum_cycles_error(&script_verifier, max_cycles, &state.current);
                    return Err(Reject::Verification(error));
                }

                let remain = max_cycles - state.current_cycles;
                let step_cycles = state.limit_cycles + MIN_STEP_CYCLE;

                let limit_cycles = if step_cycles < remain {
                    step_cycles
                } else {
                    last_step = true;
                    remain
                };
                script_verifier.resume_from_state(state, limit_cycles)
            } else {
                script_verifier.resumable_verify(MIN_STEP_CYCLE)
            }
            .map_err(Reject::Verification)?;

            match ret {
                ScriptVerifyResult::Completed(cycles) => {
                    break Completed { cycles, fee };
                }
                ScriptVerifyResult::Suspended(state) => {
                    if last_step {
                        let error = exceeded_maximum_cycles_error(
                            &script_verifier,
                            max_cycles,
                            &state.current,
                        );
                        return Err(Reject::Verification(error));
                    }
                    tmp_state = Some(state);
                }
            }
        };

        let entry = TxEntry::new(rtx.clone(), completed.cycles, fee, tx_size);
        self.handle.block_on(
            self.service
                .submit_entry(completed, tip_hash, entry, status),
        )?;
        self.handle
            .block_on(self.service.after_process(tx, remote, &Ok(completed)));

        self.handle.block_on(self.remove_front());

        let txs_verify_cache = Arc::clone(&self.service.txs_verify_cache);
        self.handle.block_on(async move {
            let mut guard = txs_verify_cache.write().await;
            guard.put(tx_hash, CacheEntry::Completed(completed));
        });

        Ok(false)
    }
}

fn exceeded_maximum_cycles_error<DL: CellDataProvider + HeaderProvider>(
    verifier: &ScriptVerifier<'_, DL>,
    max_cycles: Cycle,
    current: &(ScriptGroupType, Byte32),
) -> Error {
    verifier
        .inner()
        .find_script_group(current.0, &current.1)
        .map(|group| ScriptError::ExceededMaximumCycles(max_cycles).source(&group))
        .unwrap_or_else(|| {
            ScriptError::VMInternalError(format!("suspended state group missing {:?}", current))
                .unknown_source()
        })
        .into()
}
