#[cfg(not(target_family = "wasm"))]
use crate::ChunkCommand;
use crate::scheduler::Scheduler;
use crate::{
    error::{ScriptError, TransactionScriptError},
    initial_load::InitialProgramLoadLimit,
    syscalls::generator::generate_ckb_syscalls,
    type_id::TypeIdSystemScript,
    types::{
        DebugPrinter, Machine, RunMode, ScriptGroup, ScriptGroupType, ScriptVersion, SgData,
        SyscallGenerator, TerminatedResult, TxData,
    },
    verify_env::TxVerifyEnv,
};
use ckb_chain_spec::consensus::{Consensus, TYPE_ID_CODE_HASH};
use ckb_error::{Error, ErrorKind, InternalErrorKind};
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
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::time::{Duration, Instant};
#[cfg(not(target_family = "wasm"))]
use tokio::runtime::{Handle, RuntimeFlavor};
#[cfg(not(target_family = "wasm"))]
use tokio::sync::watch::{self, Receiver};
#[cfg(not(target_family = "wasm"))]
use tokio::task::{JoinError, JoinHandle};

#[cfg(test)]
mod tests;

#[cfg(test)]
tokio::task_local! {
    static VM_CHILD_TEST_PROBE: Arc<VmChildTestProbe>;
}

#[cfg(test)]
#[derive(Default)]
struct VmChildTestProbe {
    active: AtomicBool,
    paused: AtomicBool,
    slice_started: AtomicBool,
    slice_running: AtomicBool,
    root_load_started: AtomicBool,
    dynamic_load_started: AtomicBool,
    load_outside_slice: AtomicBool,
    pause: std::sync::Mutex<Option<VMPause>>,
}

#[cfg(test)]
struct ActiveVmChildGuard(Arc<VmChildTestProbe>);

#[cfg(test)]
impl ActiveVmChildGuard {
    fn new(probe: Arc<VmChildTestProbe>) -> Self {
        probe.active.store(true, Ordering::SeqCst);
        Self(probe)
    }
}

#[cfg(test)]
impl Drop for ActiveVmChildGuard {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::SeqCst);
    }
}

#[cfg(not(target_family = "wasm"))]
/// Owns a spawned VM task so cancelling its parent cannot detach VM work.
struct VmChildTask<T> {
    command: watch::Sender<ChunkCommand>,
    handle: Option<JoinHandle<T>>,
    pause: VMPause,
}

#[cfg(not(target_family = "wasm"))]
impl<T> VmChildTask<T> {
    fn new(pause: VMPause, command: watch::Sender<ChunkCommand>, handle: JoinHandle<T>) -> Self {
        Self {
            command,
            handle: Some(handle),
            pause,
        }
    }

    fn send(&self, command: ChunkCommand) {
        let _ = self.command.send(command);
    }

    async fn join(&mut self) -> Result<T, JoinError> {
        match self.handle.as_mut() {
            Some(handle) => {
                let result = handle.await;
                self.handle = None;
                result
            }
            None => std::future::pending().await,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl<T> Drop for VmChildTask<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.pause.interrupt();
            let _ = self.command.send(ChunkCommand::Stop);
            handle.abort();
        }
    }
}

#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VmSlicePhase {
    Idle,
    Running { started_at: Instant },
    Finished,
}

/// Persistent child-owned accounting receipt. A watch receiver may coalesce
/// transitions, so every state repeats the cumulative charge and slice
/// sequence instead of relying on a lossy start/end event pair.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VmSliceState {
    seq: u64,
    charged: Duration,
    phase: VmSlicePhase,
}

#[cfg(not(target_family = "wasm"))]
impl Default for VmSliceState {
    fn default() -> Self {
        Self {
            seq: 0,
            charged: Duration::ZERO,
            phase: VmSlicePhase::Idle,
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl VmSliceState {
    fn begin(self, started_at: Instant) -> Result<Self, VMInternalError> {
        if matches!(self.phase, VmSlicePhase::Running { .. }) {
            return Err(VMInternalError::Unexpected(
                "tx-pool VM slice began while already running".to_owned(),
            ));
        }
        let seq = self.seq.checked_add(1).ok_or_else(|| {
            VMInternalError::Unexpected("tx-pool VM slice sequence exhausted".to_owned())
        })?;
        Ok(Self {
            seq,
            charged: self.charged,
            phase: VmSlicePhase::Running { started_at },
        })
    }

    fn end(self, ended_at: Instant, finished: bool) -> Result<Self, VMInternalError> {
        let VmSlicePhase::Running { started_at } = self.phase else {
            return Err(VMInternalError::Unexpected(
                "tx-pool VM slice ended without a running receipt".to_owned(),
            ));
        };
        Ok(Self {
            seq: self.seq,
            charged: self
                .charged
                .saturating_add(ended_at.saturating_duration_since(started_at)),
            phase: if finished {
                VmSlicePhase::Finished
            } else {
                VmSlicePhase::Idle
            },
        })
    }

    fn finish(self) -> Self {
        Self {
            phase: VmSlicePhase::Finished,
            ..self
        }
    }

    fn next_group_idle(self) -> Self {
        Self {
            phase: VmSlicePhase::Idle,
            ..self
        }
    }
}

#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug)]
struct VmActiveTimeBudget {
    limit: Duration,
    state: VmSliceState,
}

#[cfg(not(target_family = "wasm"))]
enum VmChildOutcome {
    Terminated(TerminatedResult),
    InitialLoadExceeded,
}

#[cfg(not(target_family = "wasm"))]
impl VmActiveTimeBudget {
    fn new(limit: Duration) -> Self {
        Self {
            limit,
            state: VmSliceState::default(),
        }
    }

    fn observe(&mut self, state: VmSliceState) {
        debug_assert!(state.seq >= self.state.seq);
        debug_assert!(state.charged >= self.state.charged);
        self.state = state;
    }

    fn remaining(self) -> Duration {
        self.limit.saturating_sub(self.state.charged)
    }

    fn exceeded(self) -> bool {
        self.state.charged >= self.limit
    }

    fn timer_deadline(self) -> Option<(u64, Instant)> {
        let VmSlicePhase::Running { started_at } = self.state.phase else {
            return None;
        };
        started_at
            .checked_add(self.remaining())
            .map(|deadline| (self.state.seq, deadline))
    }

    fn timer_still_applies(self, seq: u64) -> bool {
        self.state.seq == seq && matches!(self.state.phase, VmSlicePhase::Running { .. })
    }
}

/// Result of tx-pool-controlled resumable script verification with one shared
/// active VM time budget. Exhaustion is node-local execution policy, not a
/// script error and not a consensus result.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumableVerificationOutcome {
    /// Every script group completed under the ordinary consensus cycle limit.
    Completed(Cycle),
    /// The local active VM time budget was exhausted by scheduler slices.
    DeadlineExceeded,
    /// The parsed root program exceeds this node's fixed tx-pool initial-load
    /// work limit. This is local resource policy, not script invalidity.
    InitialLoadExceeded,
}

/// Executor placement for the tx-pool-only resumable verification path.
///
/// This does not change VM instructions, cycles, script results, or hardfork
/// behavior. `YieldRuntimeWorker` is reserved for a multi-thread Tokio runtime
/// with one worker, where an inline synchronous VM slice would otherwise
/// prevent the parent active-slice timer and control future from being polled.
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxPoolVmExecutionMode {
    /// Execute the synchronous VM slice directly in its owned Tokio child.
    Inline,
    /// Let Tokio replace the sole runtime worker while the same VM slice runs.
    YieldRuntimeWorker,
}

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
        match self
            .resumable_verify_with_signal_control(
                limit_cycles,
                command_rx,
                None,
                None,
                TxPoolVmExecutionMode::Inline,
            )
            .await?
        {
            ResumableVerificationOutcome::Completed(cycles) => Ok(cycles),
            ResumableVerificationOutcome::DeadlineExceeded
            | ResumableVerificationOutcome::InitialLoadExceeded => Err(ErrorKind::Internal
                .because(InternalErrorKind::Interrupts.other(ScriptError::Interrupts.to_string()))),
        }
    }

    /// Perform tx-pool-controlled verification with one active scheduler time
    /// budget shared by all ordinary VM script groups. The existing child
    /// pause/stop/join lifecycle is reused; no watchdog task or detached work
    /// is created.
    pub async fn resumable_verify_with_signal_and_budget(
        &self,
        limit_cycles: Cycle,
        command_rx: &mut Receiver<ChunkCommand>,
        active_time_limit: Duration,
        initial_load_limit: InitialProgramLoadLimit,
        execution_mode: TxPoolVmExecutionMode,
    ) -> Result<ResumableVerificationOutcome, Error> {
        self.resumable_verify_with_signal_control(
            limit_cycles,
            command_rx,
            Some(active_time_limit),
            Some(initial_load_limit),
            execution_mode,
        )
        .await
    }

    async fn resumable_verify_with_signal_control(
        &self,
        limit_cycles: Cycle,
        command_rx: &mut Receiver<ChunkCommand>,
        active_time_limit: Option<Duration>,
        initial_load_limit: Option<InitialProgramLoadLimit>,
        execution_mode: TxPoolVmExecutionMode,
    ) -> Result<ResumableVerificationOutcome, Error> {
        let mut cycles = 0;
        let mut active_budget = active_time_limit.map(VmActiveTimeBudget::new);

        for (_hash, group) in self.groups() {
            // vm should early return invalid cycles
            let remain_cycles = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::Other(format!("expect invalid cycles {limit_cycles} {cycles}"))
                    .source(group)
            })?;

            match self
                .verify_group_with_signal(
                    group,
                    remain_cycles,
                    command_rx,
                    active_budget.as_mut(),
                    initial_load_limit,
                    execution_mode,
                )
                .await
            {
                Ok(ResumableVerificationOutcome::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ResumableVerificationOutcome::DeadlineExceeded) => {
                    return Ok(ResumableVerificationOutcome::DeadlineExceeded);
                }
                Ok(ResumableVerificationOutcome::InitialLoadExceeded) => {
                    return Ok(ResumableVerificationOutcome::InitialLoadExceeded);
                }
                Err(error) => {
                    #[cfg(feature = "logging")]
                    logging::on_script_error(_hash, &self.hash(), &error);
                    return Err(error.source(group).into());
                }
            }
        }

        Ok(ResumableVerificationOutcome::Completed(cycles))
    }

    async fn verify_group_with_signal(
        &self,
        group: &ScriptGroup,
        max_cycles: Cycle,
        command_rx: &mut Receiver<ChunkCommand>,
        active_budget: Option<&mut VmActiveTimeBudget>,
        initial_load_limit: Option<InitialProgramLoadLimit>,
        execution_mode: TxPoolVmExecutionMode,
    ) -> Result<ResumableVerificationOutcome, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.into()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: &self.tx_data.rtx,
                script_group: group,
                max_cycles,
            };
            let cycles = verifier.verify()?;
            Ok(ResumableVerificationOutcome::Completed(cycles))
        } else {
            self.chunk_run_with_signal(
                group,
                max_cycles,
                command_rx,
                active_budget,
                initial_load_limit,
                execution_mode,
            )
            .await
        }
    }

    async fn chunk_run_with_signal(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
        signal: &mut Receiver<ChunkCommand>,
        mut active_budget: Option<&mut VmActiveTimeBudget>,
        initial_load_limit: Option<InitialProgramLoadLimit>,
        execution_mode: TxPoolVmExecutionMode,
    ) -> Result<ResumableVerificationOutcome, ScriptError> {
        let mut scheduler = self.create_scheduler(script_group)?;
        if active_budget
            .as_ref()
            .is_some_and(|budget| budget.exceeded())
        {
            return Ok(ResumableVerificationOutcome::DeadlineExceeded);
        }
        let mut pause = VMPause::new();
        let child_pause = pause.clone();

        // Each group inherits the outer command level. In particular, a
        // previously published Suspend or Stop must not allow the next root
        // program load to start while waiting for a new edge.
        let initial_command = signal.borrow().to_owned();
        let (child_tx, mut child_rx) = watch::channel(initial_command.clone());
        let initial_slice_state = active_budget
            .as_ref()
            .map_or_else(VmSliceState::default, |budget| {
                budget.state.next_group_idle()
            });
        let (slice_tx, mut slice_rx) = watch::channel(initial_slice_state);
        #[cfg(test)]
        let test_probe = VM_CHILD_TEST_PROBE.try_with(Arc::clone).ok();
        #[cfg(test)]
        if let Some(probe) = test_probe.as_ref() {
            let load_probe = Arc::clone(probe);
            scheduler.set_program_load_test_hook(Arc::new(move |dynamic| {
                if !load_probe.slice_running.load(Ordering::SeqCst) {
                    load_probe.load_outside_slice.store(true, Ordering::SeqCst);
                }
                if dynamic {
                    load_probe
                        .dynamic_load_started
                        .store(true, Ordering::SeqCst);
                } else {
                    load_probe.root_load_started.store(true, Ordering::SeqCst);
                }
            }));
        }
        #[cfg(test)]
        if let Some(probe) = test_probe.as_ref() {
            *probe
                .pause
                .lock()
                .expect("the VM test probe is not poisoned") = Some(pause.clone());
        }
        let jh = tokio::spawn(async move {
            #[cfg(test)]
            let _active_vm_child = test_probe
                .as_ref()
                .map(|probe| ActiveVmChildGuard::new(Arc::clone(probe)));
            child_rx.mark_changed();
            let mut slice_state = initial_slice_state;
            let mut initial_load_limit = initial_load_limit;
            loop {
                let pause_cloned = child_pause.clone();
                if child_rx.changed().await.is_err() {
                    slice_state = slice_state.finish();
                    slice_tx.send_replace(slice_state);
                    return Err(ckb_vm::Error::External("command channel closed".into()));
                }
                match *child_rx.borrow() {
                    ChunkCommand::Stop => {
                        slice_state = slice_state.finish();
                        slice_tx.send_replace(slice_state);
                        return Err(ckb_vm::Error::External("stopped".into()));
                    }
                    ChunkCommand::Suspend => {
                        continue;
                    }
                    ChunkCommand::Resume => {
                        //info!("[verify-test] run_vms_child: resume");
                        slice_state = slice_state.begin(Instant::now())?;
                        slice_tx.send_replace(slice_state);
                        #[cfg(test)]
                        if let Some(probe) = test_probe.as_ref() {
                            probe.slice_running.store(true, Ordering::SeqCst);
                            probe.slice_started.store(true, Ordering::SeqCst);
                        }
                        let run_slice = || -> Result<VmChildOutcome, VMInternalError> {
                            if let Some(limit) = initial_load_limit.take() {
                                let receipt = scheduler.prepare_root_program_load()?;
                                if !receipt.is_some_and(|receipt| limit.admits(receipt)) {
                                    return Ok(VmChildOutcome::InitialLoadExceeded);
                                }
                            }
                            scheduler
                                .run(RunMode::Pause(pause_cloned, max_cycles))
                                .map(VmChildOutcome::Terminated)
                        };
                        let execution = match execution_mode {
                            TxPoolVmExecutionMode::Inline => run_slice(),
                            TxPoolVmExecutionMode::YieldRuntimeWorker => {
                                debug_assert_eq!(
                                    Handle::current().runtime_flavor(),
                                    RuntimeFlavor::MultiThread,
                                    "the yielding tx-pool VM mode requires a multi-thread runtime"
                                );
                                tokio::task::block_in_place(run_slice)
                            }
                        };
                        #[cfg(test)]
                        if let Some(probe) = test_probe.as_ref() {
                            probe.slice_running.store(false, Ordering::SeqCst);
                        }
                        slice_state = slice_state.end(
                            Instant::now(),
                            !matches!(&execution, Err(VMInternalError::Pause)),
                        )?;
                        slice_tx.send_replace(slice_state);
                        match execution {
                            Ok(outcome) => return Ok(outcome),
                            Err(VMInternalError::Pause) => {
                                #[cfg(test)]
                                if let Some(probe) = test_probe.as_ref() {
                                    probe.paused.store(true, Ordering::SeqCst);
                                }
                                // continue to wait for
                                debug_assert!(
                                    scheduler.consumed_cycles() <= max_cycles,
                                    "Consumed cycles ({}) exceeded max_cycles ({})",
                                    scheduler.consumed_cycles(),
                                    max_cycles
                                );
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }
            }
        });
        let mut child = VmChildTask::new(pause.clone(), child_tx, jh);
        let mut latest_slice_state = initial_slice_state;
        let mut desired_command = initial_command.clone();
        let mut run_requested = initial_command == ChunkCommand::Resume;
        loop {
            let armed_timer = if desired_command == ChunkCommand::Stop {
                None
            } else {
                active_budget
                    .as_ref()
                    .and_then(|budget| budget.timer_deadline())
            };
            let armed_seq = armed_timer.map(|(seq, _)| seq);
            tokio::select! {
                biased;
                result = child.join() => {
                    latest_slice_state = *slice_rx.borrow_and_update();
                    if let Some(budget) = active_budget.as_deref_mut() {
                        budget.observe(latest_slice_state);
                    }
                    let res = match result {
                        Ok(res) => res,
                        Err(error) if error.is_panic() => {
                            std::panic::resume_unwind(error.into_panic())
                        }
                        Err(_) => return Err(ScriptError::Interrupts),
                    };
                    let budget_exceeded =
                        active_budget.as_ref().is_some_and(|budget| budget.exceeded());
                    match res {
                        Ok(VmChildOutcome::InitialLoadExceeded) => {
                            return Ok(ResumableVerificationOutcome::InitialLoadExceeded);
                        }
                        _ if budget_exceeded => {
                            return Ok(ResumableVerificationOutcome::DeadlineExceeded);
                        }
                        Ok(VmChildOutcome::Terminated(TerminatedResult {
                            exit_code: 0,
                            consumed_cycles: cycles,
                        })) => {
                            return Ok(ResumableVerificationOutcome::Completed(cycles));
                        }
                        Ok(VmChildOutcome::Terminated(TerminatedResult { exit_code, .. })) => {
                            return Err(ScriptError::validation_failure(
                                &script_group.script,
                                exit_code
                            ))},
                        Err(err) => {
                            return Err(self.map_vm_internal_error(err, max_cycles));
                        }
                    }
                }
                Ok(_) = slice_rx.changed() => {
                    latest_slice_state = *slice_rx.borrow_and_update();
                    if let Some(budget) = active_budget.as_deref_mut() {
                        budget.observe(latest_slice_state);
                    }
                    match latest_slice_state.phase {
                        VmSlicePhase::Running { .. } => {
                            run_requested = true;
                            if desired_command == ChunkCommand::Suspend {
                                pause.interrupt();
                            }
                        }
                        VmSlicePhase::Idle => {
                            run_requested = false;
                            if active_budget.as_ref().is_some_and(|budget| budget.exceeded()) {
                                if desired_command != ChunkCommand::Stop {
                                    desired_command = ChunkCommand::Stop;
                                    child.send(ChunkCommand::Stop);
                                }
                            } else if desired_command == ChunkCommand::Resume {
                                pause.free();
                                child.send(ChunkCommand::Resume);
                                run_requested = true;
                            }
                        }
                        VmSlicePhase::Finished => {
                            run_requested = false;
                        }
                    }
                }
                _ = async move {
                    match armed_timer {
                        Some((_, deadline)) => tokio::time::sleep_until(deadline.into()).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let Some(armed_seq) = armed_seq else {
                        continue;
                    };
                    let current = *slice_rx.borrow();
                    if let Some(budget) = active_budget.as_deref_mut() {
                        budget.observe(current);
                        if budget.timer_still_applies(armed_seq)
                            && desired_command != ChunkCommand::Stop
                        {
                            desired_command = ChunkCommand::Stop;
                            pause.interrupt();
                            child.send(ChunkCommand::Stop);
                        }
                    }
                }
                Ok(_) = signal.changed(), if desired_command != ChunkCommand::Stop => {
                    let command = signal.borrow().to_owned();
                    //info!("[verify-test] run_vms_with_signal: {:?}", command);
                    match command {
                        ChunkCommand::Suspend => {
                            desired_command = ChunkCommand::Suspend;
                            if run_requested {
                                pause.interrupt();
                            }
                        }
                        ChunkCommand::Stop => {
                            desired_command = ChunkCommand::Stop;
                            if run_requested {
                                pause.interrupt();
                            }
                            child.send(ChunkCommand::Stop);
                        }
                        ChunkCommand::Resume => {
                            desired_command = ChunkCommand::Resume;
                            if !run_requested && latest_slice_state.phase == VmSlicePhase::Idle {
                                pause.free();
                                child.send(ChunkCommand::Resume);
                                run_requested = true;
                            }
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
