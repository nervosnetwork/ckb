#[cfg(not(target_family = "wasm"))]
use crate::ChunkCommand;
use crate::scheduler::Scheduler;
use crate::{
    error::{ScriptError, TransactionScriptError},
    syscalls::generator::generate_ckb_syscalls,
    type_id::TypeIdSystemScript,
    types::{
        DebugPrinter, Machine, RunMode, ScriptGroup, ScriptGroupType, ScriptVersion, SgData,
        SyscallGenerator, TerminatedResult, TxData,
    },
    verify_env::TxVerifyEnv,
};
use ckb_chain_spec::consensus::{Consensus, TYPE_ID_CODE_HASH};
use ckb_error::Error;
#[cfg(feature = "logging")]
use ckb_logger::{debug, info};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_types::{
    bytes::Bytes,
    core::{Cycle, ScriptHashType, cell::ResolvedTransaction},
    packed::{Byte32, Script},
};
#[cfg(not(target_family = "wasm"))]
use ckb_vm::machine::Pause as VMPause;
use ckb_vm::{DefaultMachineRunner, Error as VMInternalError};
use std::sync::Arc;
#[cfg(not(target_family = "wasm"))]
use tokio::sync::{
    oneshot,
    watch::{self, Receiver},
};

#[cfg(test)]
mod tests;

/// This struct leverages CKB VM to verify transaction inputs.
pub struct TransactionScriptsVerifier<
    DL: CellDataProvider,
    V = DebugPrinter,
    M: DefaultMachineRunner = Machine,
> {
    tx_data: Arc<TxData<DL>>,
    syscall_generator: SyscallGenerator<DL, V, <M as DefaultMachineRunner>::Inner>,
    syscall_context: V,
}

impl<DL> TransactionScriptsVerifier<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    /// Create a script verifier using default CKB syscalls and a default debug printer
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
    ) -> Self {
        let debug_printer: DebugPrinter = Arc::new(
            #[allow(unused_variables)]
            |hash: &Byte32, message: &str| {
                #[cfg(feature = "logging")]
                debug!("script group: {} DEBUG OUTPUT: {}", hash, message);
            },
        );

        Self::new_with_debug_printer(rtx, data_loader, consensus, tx_env, debug_printer)
    }

    /// Create a script verifier using default CKB syscalls and a custom debug printer
    pub fn new_with_debug_printer(
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
        debug_printer: DebugPrinter,
    ) -> Self {
        Self::new_with_generator(
            rtx,
            data_loader,
            consensus,
            tx_env,
            generate_ckb_syscalls,
            debug_printer,
        )
    }
}

impl<DL, V, M> TransactionScriptsVerifier<DL, V, M>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Clone,
    V: Clone,
    M: DefaultMachineRunner,
{
    /// Creates a script verifier for the transaction.
    ///
    /// ## Params
    ///
    /// * `rtx` - transaction which cell out points have been resolved.
    /// * `data_loader` - used to load cell data.
    /// * `consensus` - consensus parameters.
    /// * `tx_env` - environment for verifying transaction, such as committed block, etc.
    /// * `syscall_generator` - a syscall generator for current verifier
    /// * `syscall_context` - context for syscall generator
    pub fn new_with_generator(
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
        syscall_generator: SyscallGenerator<DL, V, <M as DefaultMachineRunner>::Inner>,
        syscall_context: V,
    ) -> TransactionScriptsVerifier<DL, V, M> {
        let tx_data = Arc::new(TxData::new(rtx, data_loader, consensus, tx_env));

        TransactionScriptsVerifier {
            tx_data,
            syscall_generator,
            syscall_context,
        }
    }

    //////////////////////////////////////////////////////////////////
    // Functions below have been moved from verifier struct to TxData,
    // however we still preserve all the public APIs by delegating
    // them to TxData.
    //////////////////////////////////////////////////////////////////

    #[inline]
    #[allow(dead_code)]
    fn hash(&self) -> Byte32 {
        self.tx_data.tx_hash()
    }

    /// Extracts actual script binary either in dep cells.
    pub fn extract_script(&self, script: &Script) -> Result<Bytes, ScriptError> {
        self.tx_data.extract_script(script)
    }

    /// Returns the version of the machine based on the script and the consensus rules.
    pub fn select_version(&self, script: &Script) -> Result<ScriptVersion, ScriptError> {
        self.tx_data.select_version(script)
    }

    /// Returns all script groups.
    pub fn groups(&self) -> impl Iterator<Item = (&'_ Byte32, &'_ ScriptGroup)> {
        self.tx_data.groups()
    }

    /// Returns all script groups with type.
    pub fn groups_with_type(
        &self,
    ) -> impl Iterator<Item = (ScriptGroupType, &'_ Byte32, &'_ ScriptGroup)> {
        self.tx_data.groups_with_type()
    }

    /// Finds the script group from cell deps.
    pub fn find_script_group(
        &self,
        script_group_type: ScriptGroupType,
        script_hash: &Byte32,
    ) -> Option<&ScriptGroup> {
        self.tx_data
            .find_script_group(script_group_type, script_hash)
    }

    //////////////////////////////////////////////////////////////////
    // This marks the end of delegated functions.
    //////////////////////////////////////////////////////////////////

    /// Verifies the transaction by running scripts.
    ///
    /// ## Params
    ///
    /// * `max_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    ///   when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles on success, Otherwise it returns the verification error.
    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles: Cycle = 0;

        // Now run each script group
        for (_hash, group) in self.groups() {
            // max_cycles must reduce by each group exec
            let used_cycles = self
                .verify_script_group(group, max_cycles - cycles)
                .map_err(|e| {
                    #[cfg(feature = "logging")]
                    logging::on_script_error(_hash, &self.hash(), &e);
                    e.source(group)
                })?;

            cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
        }
        Ok(cycles)
    }

    /// Runs a single script in current transaction, while this is not useful for
    /// CKB itself, it can be very helpful when building a CKB debugger.
    pub fn verify_single(
        &self,
        script_group_type: ScriptGroupType,
        script_hash: &Byte32,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        match self.find_script_group(script_group_type, script_hash) {
            Some(group) => self.verify_script_group(group, max_cycles),
            None => Err(ScriptError::ScriptNotFound(script_hash.clone())),
        }
    }

    fn verify_script_group(
        &self,
        group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Cycle, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.into()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: &self.tx_data.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify()
        } else {
            self.run(group, max_cycles)
        }
    }

    /// Create a scheduler to manage virtual machine instances.
    pub fn create_scheduler(
        &self,
        script_group: &ScriptGroup,
    ) -> Result<Scheduler<DL, V, M>, ScriptError> {
        let sg_data = SgData::new(&self.tx_data, script_group)?;
        Ok(Scheduler::new(
            sg_data,
            self.syscall_generator,
            self.syscall_context.clone(),
        ))
    }

    /// Runs a single program, then returns the exit code together with the entire
    /// machine to the caller for more inspections.
    pub fn detailed_run(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<TerminatedResult, ScriptError> {
        let mut scheduler = self.create_scheduler(script_group)?;
        scheduler
            .run(RunMode::LimitCycles(max_cycles))
            .map_err(|err| self.map_vm_internal_error(err, max_cycles))
    }

    fn run(&self, script_group: &ScriptGroup, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let result = self.detailed_run(script_group, max_cycles)?;

        if result.exit_code == 0 {
            Ok(result.consumed_cycles)
        } else {
            Err(ScriptError::validation_failure(
                &script_group.script,
                result.exit_code,
            ))
        }
    }

    fn map_vm_internal_error(&self, error: VMInternalError, max_cycles: Cycle) -> ScriptError {
        match error {
            VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
            VMInternalError::External(reason) if reason.eq("stopped") => ScriptError::Interrupts,
            _ => ScriptError::VMInternalError(error),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl<DL, V, M> TransactionScriptsVerifier<DL, V, M>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    V: Send + Clone + 'static,
    M: DefaultMachineRunner + Send + 'static,
{
    /// Performing a resumable verification on the transaction scripts with signal channel,
    /// if `Suspend` comes from `command_rx`, the process will be hang up until `Resume` comes,
    /// otherwise, it will return until the verification is completed.
    pub async fn resumable_verify_with_signal(
        &self,
        limit_cycles: Cycle,
        command_rx: &mut Receiver<ChunkCommand>,
    ) -> Result<Cycle, Error> {
        let mut cycles = 0;

        let groups: Vec<_> = self.groups().collect();
        for (_hash, group) in groups.iter() {
            // vm should early return invalid cycles
            let remain_cycles = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::Other(format!("expect invalid cycles {limit_cycles} {cycles}"))
                    .source(group)
            })?;

            match self
                .verify_group_with_signal(group, remain_cycles, command_rx)
                .await
            {
                Ok(used_cycles) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Err(e) => {
                    #[cfg(feature = "logging")]
                    logging::on_script_error(_hash, &self.hash(), &e);
                    return Err(e.source(group).into());
                }
            }
        }

        Ok(cycles)
    }

    async fn verify_group_with_signal(
        &self,
        group: &ScriptGroup,
        max_cycles: Cycle,
        command_rx: &mut Receiver<ChunkCommand>,
    ) -> Result<Cycle, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.into()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: &self.tx_data.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify()
        } else {
            self.chunk_run_with_signal(group, max_cycles, command_rx)
                .await
        }
    }

    async fn chunk_run_with_signal(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
        signal: &mut Receiver<ChunkCommand>,
    ) -> Result<Cycle, ScriptError> {
        let mut scheduler = self.create_scheduler(script_group)?;
        let mut pause = VMPause::new();
        let child_pause = pause.clone();
        let (finish_tx, mut finish_rx) =
            oneshot::channel::<Result<TerminatedResult, ckb_vm::Error>>();

        // send initial `Resume` command to child
        // it's maybe useful to set initial command to `signal.borrow().to_owned()`
        // so that we can control the initial state of child, which is useful for testing purpose
        let (child_tx, mut child_rx) = watch::channel(ChunkCommand::Resume);
        let jh = tokio::spawn(async move {
            child_rx.mark_changed();
            loop {
                let pause_cloned = child_pause.clone();
                let _ = child_rx.changed().await;
                match *child_rx.borrow() {
                    ChunkCommand::Stop => {
                        let exit = Err(ckb_vm::Error::External("stopped".into()));
                        let _ = finish_tx.send(exit);
                        return;
                    }
                    ChunkCommand::Suspend => {
                        continue;
                    }
                    ChunkCommand::Resume => {
                        //info!("[verify-test] run_vms_child: resume");
                        let res = scheduler.run(RunMode::Pause(pause_cloned, max_cycles));
                        match res {
                            Ok(_) => {
                                let _ = finish_tx.send(res);
                                return;
                            }
                            Err(VMInternalError::Pause) => {
                                // continue to wait for
                                debug_assert!(
                                    scheduler.consumed_cycles() <= max_cycles,
                                    "Consumed cycles ({}) exceeded max_cycles ({})",
                                    scheduler.consumed_cycles(),
                                    max_cycles
                                );
                            }
                            _ => {
                                let _ = finish_tx.send(res);
                                return;
                            }
                        }
                    }
                }
            }
        });

        loop {
            tokio::select! {
                Ok(_) = signal.changed() => {
                    let command = signal.borrow().to_owned();
                    //info!("[verify-test] run_vms_with_signal: {:?}", command);
                    match command {
                        ChunkCommand::Suspend => {
                            pause.interrupt();
                        }
                        ChunkCommand::Stop => {
                            pause.interrupt();
                            let _ = child_tx.send(command);
                        }
                        ChunkCommand::Resume => {
                            pause.free();
                            let _ = child_tx.send(command);
                        }
                    }
                }
                Ok(res) = &mut finish_rx => {
                    let _ = jh.await;
                    match res {
                        Ok(TerminatedResult {
                            exit_code: 0,
                            consumed_cycles: cycles,
                        }) => {
                            return Ok(cycles);
                        }
                        Ok(TerminatedResult { exit_code, .. }) => {
                            return Err(ScriptError::validation_failure(
                                &script_group.script,
                                exit_code
                            ))},
                        Err(err) => {
                            return Err(self.map_vm_internal_error(err, max_cycles));
                        }
                    }

                }
                else => { break Err(ScriptError::validation_failure(&script_group.script, 0)) }
            }
        }
    }
}

fn wrapping_cycles_add(
    lhs: Cycle,
    rhs: Cycle,
    group: &ScriptGroup,
) -> Result<Cycle, TransactionScriptError> {
    lhs.checked_add(rhs)
        .ok_or_else(|| ScriptError::CyclesOverflow(lhs, rhs).source(group))
}

#[cfg(feature = "logging")]
mod logging {
    use super::{Byte32, ScriptError, info};

    pub fn on_script_error(group: &Byte32, tx: &Byte32, error: &ScriptError) {
        info!(
            "Error validating script group {} of transaction {}: {}",
            group, tx, error
        );
    }
}
