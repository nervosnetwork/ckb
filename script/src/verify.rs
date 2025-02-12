use crate::scheduler::Scheduler;
#[cfg(test)]
use crate::syscalls::Pause;
use crate::syscalls::{InheritedFd, ProcessID};
use crate::types::{DataPieceId, FullSuspendedState, Message, RunMode, TxData, VmId, FIRST_VM_ID};
#[cfg(not(target_family = "wasm"))]
use crate::ChunkCommand;
use crate::{
    error::{ScriptError, TransactionScriptError},
    syscalls::{
        Close, CurrentCycles, Debugger, Exec, ExecV2, LoadBlockExtension, LoadCell, LoadCellData,
        LoadHeader, LoadInput, LoadScript, LoadScriptHash, LoadTx, LoadWitness, Pipe, Read, Spawn,
        VMVersion, Wait, Write,
    },
    type_id::TypeIdSystemScript,
    types::{
        CoreMachine, DebugPrinter, Indices, ScriptGroup, ScriptGroupType, ScriptVersion,
        TransactionState, VerifyResult,
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
    core::{
        cell::{CellMeta, ResolvedTransaction},
        Cycle, ScriptHashType,
    },
    packed::{Byte32, CellOutput, OutPoint, Script},
    prelude::*,
};
#[cfg(not(target_family = "wasm"))]
use ckb_vm::machine::Pause as VMPause;
use ckb_vm::{snapshot2::Snapshot2Context, Error as VMInternalError, Syscalls};
use std::sync::{Arc, Mutex};
use std::{
    collections::{BTreeMap, HashMap},
    sync::RwLock,
};
#[cfg(not(target_family = "wasm"))]
use tokio::sync::{
    oneshot,
    watch::{self, Receiver},
};

#[cfg(test)]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
mod tests;

pub enum ChunkState {
    Suspended(Option<FullSuspendedState>),
    // (total_cycles, consumed_cycles in last chunk)
    Completed(Cycle, Cycle),
}

impl ChunkState {
    pub fn suspended(state: FullSuspendedState) -> Self {
        ChunkState::Suspended(Some(state))
    }

    pub fn suspended_type_id() -> Self {
        ChunkState::Suspended(None)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum DataGuard {
    NotLoaded(OutPoint),
    Loaded(Bytes),
}

/// LazyData wrapper make sure not-loaded data will be loaded only after one access
#[derive(Debug, Clone)]
struct LazyData(Arc<RwLock<DataGuard>>);

impl LazyData {
    fn from_cell_meta(cell_meta: &CellMeta) -> LazyData {
        match &cell_meta.mem_cell_data {
            Some(data) => LazyData(Arc::new(RwLock::new(DataGuard::Loaded(data.to_owned())))),
            None => LazyData(Arc::new(RwLock::new(DataGuard::NotLoaded(
                cell_meta.out_point.clone(),
            )))),
        }
    }

    fn access<DL: CellDataProvider>(&self, data_loader: &DL) -> Result<Bytes, ScriptError> {
        let guard = self
            .0
            .read()
            .map_err(|_| ScriptError::Other("RwLock poisoned".into()))?
            .to_owned();
        match guard {
            DataGuard::NotLoaded(out_point) => {
                let data = data_loader
                    .get_cell_data(&out_point)
                    .ok_or(ScriptError::Other("cell data not found".into()))?;
                let mut write_guard = self
                    .0
                    .write()
                    .map_err(|_| ScriptError::Other("RwLock poisoned".into()))?;
                *write_guard = DataGuard::Loaded(data.clone());
                Ok(data)
            }
            DataGuard::Loaded(bytes) => Ok(bytes),
        }
    }
}

#[derive(Debug, Clone)]
enum Binaries {
    Unique(Byte32, LazyData),
    Duplicate(Byte32, LazyData),
    Multiple,
}

impl Binaries {
    fn new(data_hash: Byte32, data: LazyData) -> Self {
        Self::Unique(data_hash, data)
    }

    fn merge(&mut self, data_hash: &Byte32) {
        match self {
            Self::Unique(ref hash, data) | Self::Duplicate(ref hash, data) => {
                if hash != data_hash {
                    *self = Self::Multiple;
                } else {
                    *self = Self::Duplicate(hash.to_owned(), data.to_owned());
                }
            }
            Self::Multiple => {}
        }
    }
}

/// Syscalls can be generated individually by TransactionScriptsSyscallsGenerator.
///
/// TransactionScriptsSyscallsGenerator can be cloned.
#[derive(Clone)]
pub struct TransactionScriptsSyscallsGenerator<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    pub(crate) base_cycles: Arc<Mutex<u64>>,
    pub(crate) data_loader: DL,
    pub(crate) debug_printer: DebugPrinter,
    pub(crate) message_box: Arc<Mutex<Vec<Message>>>,
    pub(crate) outputs: Arc<Vec<CellMeta>>,
    pub(crate) rtx: Arc<ResolvedTransaction>,
    #[cfg(test)]
    pub(crate) skip_pause: Arc<AtomicBool>,
    pub(crate) vm_id: VmId,
}

impl<DL> TransactionScriptsSyscallsGenerator<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    /// Build syscall: current_cycles
    pub fn build_current_cycles(&self) -> CurrentCycles {
        CurrentCycles::new(Arc::clone(&self.base_cycles))
    }

    /// Build syscall: vm_version
    pub fn build_vm_version(&self) -> VMVersion {
        VMVersion::new()
    }

    /// Build syscall: exec
    pub fn build_exec(&self, group_inputs: Indices, group_outputs: Indices) -> Exec<DL> {
        Exec::new(
            self.data_loader.clone(),
            Arc::clone(&self.rtx),
            Arc::clone(&self.outputs),
            group_inputs,
            group_outputs,
        )
    }

    /// Build syscall: exec. When script version >= V2, this exec implementation is used.
    pub fn build_exec_v2(&self) -> ExecV2 {
        ExecV2::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Build syscall: load_tx
    pub fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(Arc::clone(&self.rtx))
    }

    /// Build syscall: load_cell
    pub fn build_load_cell(&self, group_inputs: Indices, group_outputs: Indices) -> LoadCell<DL> {
        LoadCell::new(
            self.data_loader.clone(),
            Arc::clone(&self.rtx),
            Arc::clone(&self.outputs),
            group_inputs,
            group_outputs,
        )
    }

    /// Build syscall: load_cell_data
    pub fn build_load_cell_data(
        &self,
        snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    ) -> LoadCellData<DL> {
        LoadCellData::new(snapshot2_context)
    }

    ///Build syscall: load_input
    pub fn build_load_input(&self, group_inputs: Indices) -> LoadInput {
        LoadInput::new(Arc::clone(&self.rtx), group_inputs)
    }

    /// Build syscall: load_script_hash
    pub fn build_load_script_hash(&self, hash: Byte32) -> LoadScriptHash {
        LoadScriptHash::new(hash)
    }

    /// Build syscall: load_header
    pub fn build_load_header(&self, group_inputs: Indices) -> LoadHeader<DL> {
        LoadHeader::new(
            self.data_loader.clone(),
            Arc::clone(&self.rtx),
            group_inputs,
        )
    }

    /// Build syscall: load_block_extension
    pub fn build_load_block_extension(&self, group_inputs: Indices) -> LoadBlockExtension<DL> {
        LoadBlockExtension::new(
            self.data_loader.clone(),
            Arc::clone(&self.rtx),
            group_inputs,
        )
    }

    /// Build syscall: load_witness
    pub fn build_load_witness(&self, group_inputs: Indices, group_outputs: Indices) -> LoadWitness {
        LoadWitness::new(Arc::clone(&self.rtx), group_inputs, group_outputs)
    }

    /// Build syscall: load_script
    pub fn build_load_script(&self, script: Script) -> LoadScript {
        LoadScript::new(script)
    }

    /// Build syscall: spawn
    pub fn build_spawn(
        &self,
        snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    ) -> Spawn<DL> {
        Spawn::new(self.vm_id, Arc::clone(&self.message_box), snapshot2_context)
    }

    /// Build syscall: wait
    pub fn build_wait(&self) -> Wait {
        Wait::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Build syscall: process_id
    pub fn build_process_id(&self) -> ProcessID {
        ProcessID::new(self.vm_id)
    }

    /// Build syscall: pipe
    pub fn build_pipe(&self) -> Pipe {
        Pipe::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Build syscall: write
    pub fn build_write(&self) -> Write {
        Write::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Build syscall: read
    pub fn build_read(&self) -> Read {
        Read::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Build syscall: inherited_fd
    pub fn inherited_fd(&self) -> InheritedFd {
        InheritedFd::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Build syscall: close
    pub fn close(&self) -> Close {
        Close::new(self.vm_id, Arc::clone(&self.message_box))
    }

    /// Generate syscalls.
    pub fn generate_syscalls(
        &self,
        script_version: ScriptVersion,
        script_group: &ScriptGroup,
        snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    ) -> Vec<Box<(dyn Syscalls<CoreMachine>)>> {
        let current_script_hash = script_group.script.calc_script_hash();
        let script_group_input_indices = Arc::new(script_group.input_indices.clone());
        let script_group_output_indices = Arc::new(script_group.output_indices.clone());
        let mut syscalls: Vec<Box<(dyn Syscalls<CoreMachine>)>> = vec![
            Box::new(self.build_load_script_hash(current_script_hash.clone())),
            Box::new(self.build_load_tx()),
            Box::new(self.build_load_cell(
                Arc::clone(&script_group_input_indices),
                Arc::clone(&script_group_output_indices),
            )),
            Box::new(self.build_load_input(Arc::clone(&script_group_input_indices))),
            Box::new(self.build_load_header(Arc::clone(&script_group_input_indices))),
            Box::new(self.build_load_witness(
                Arc::clone(&script_group_input_indices),
                Arc::clone(&script_group_output_indices),
            )),
            Box::new(self.build_load_script(script_group.script.clone())),
            Box::new(self.build_load_cell_data(Arc::clone(&snapshot2_context))),
            Box::new(Debugger::new(
                current_script_hash,
                Arc::clone(&self.debug_printer),
            )),
        ];
        if script_version >= ScriptVersion::V1 {
            syscalls.append(&mut vec![
                Box::new(self.build_vm_version()),
                if script_version >= ScriptVersion::V2 {
                    Box::new(self.build_exec_v2())
                } else {
                    Box::new(self.build_exec(
                        Arc::clone(&script_group_input_indices),
                        Arc::clone(&script_group_output_indices),
                    ))
                },
                Box::new(self.build_current_cycles()),
            ]);
        }
        if script_version >= ScriptVersion::V2 {
            syscalls.append(&mut vec![
                Box::new(self.build_load_block_extension(Arc::clone(&script_group_input_indices))),
                Box::new(self.build_spawn(Arc::clone(&snapshot2_context))),
                Box::new(self.build_process_id()),
                Box::new(self.build_pipe()),
                Box::new(self.build_wait()),
                Box::new(self.build_write()),
                Box::new(self.build_read()),
                Box::new(self.inherited_fd()),
                Box::new(self.close()),
            ]);
        }
        #[cfg(test)]
        syscalls.push(Box::new(Pause::new(Arc::clone(&self.skip_pause))));
        syscalls
    }
}

/// This struct leverages CKB VM to verify transaction inputs.
///
/// FlatBufferBuilder owned `Vec<u8>` that grows as needed, in the
/// future, we might refactor this to share buffer to achieve zero-copy
pub struct TransactionScriptsVerifier<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    data_loader: DL,

    rtx: Arc<ResolvedTransaction>,

    binaries_by_data_hash: HashMap<Byte32, LazyData>,
    binaries_by_type_hash: HashMap<Byte32, Binaries>,

    lock_groups: BTreeMap<Byte32, ScriptGroup>,
    type_groups: BTreeMap<Byte32, ScriptGroup>,

    #[cfg(test)]
    skip_pause: Arc<AtomicBool>,

    consensus: Arc<Consensus>,
    tx_env: Arc<TxVerifyEnv>,

    syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
}

impl<DL> TransactionScriptsVerifier<DL>
where
    DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
{
    /// Creates a script verifier for the transaction.
    ///
    /// ## Params
    ///
    /// * `rtx` - transaction which cell out points have been resolved.
    /// * `data_loader` - used to load cell data.
    pub fn new(
        rtx: Arc<ResolvedTransaction>,
        data_loader: DL,
        consensus: Arc<Consensus>,
        tx_env: Arc<TxVerifyEnv>,
    ) -> TransactionScriptsVerifier<DL> {
        let tx_hash = rtx.transaction.hash();
        let resolved_cell_deps = &rtx.resolved_cell_deps;
        let resolved_inputs = &rtx.resolved_inputs;
        let outputs = Arc::new(
            rtx.transaction
                .outputs_with_data_iter()
                .enumerate()
                .map(|(index, (cell_output, data))| {
                    let out_point = OutPoint::new_builder()
                        .tx_hash(tx_hash.clone())
                        .index(index.pack())
                        .build();
                    let data_hash = CellOutput::calc_data_hash(&data);
                    CellMeta {
                        cell_output,
                        out_point,
                        transaction_info: None,
                        data_bytes: data.len() as u64,
                        mem_cell_data: Some(data),
                        mem_cell_data_hash: Some(data_hash),
                    }
                })
                .collect(),
        );

        let mut binaries_by_data_hash: HashMap<Byte32, LazyData> = HashMap::default();
        let mut binaries_by_type_hash: HashMap<Byte32, Binaries> = HashMap::default();
        for cell_meta in resolved_cell_deps {
            let data_hash = data_loader
                .load_cell_data_hash(cell_meta)
                .expect("cell data hash");
            let lazy = LazyData::from_cell_meta(cell_meta);
            binaries_by_data_hash.insert(data_hash.to_owned(), lazy.to_owned());

            if let Some(t) = &cell_meta.cell_output.type_().to_opt() {
                binaries_by_type_hash
                    .entry(t.calc_script_hash())
                    .and_modify(|bin| bin.merge(&data_hash))
                    .or_insert_with(|| Binaries::new(data_hash.to_owned(), lazy.to_owned()));
            }
        }

        let mut lock_groups = BTreeMap::default();
        let mut type_groups = BTreeMap::default();
        for (i, cell_meta) in resolved_inputs.iter().enumerate() {
            // here we are only pre-processing the data, verify method validates
            // each input has correct script setup.
            let output = &cell_meta.cell_output;
            let lock_group_entry = lock_groups
                .entry(output.calc_lock_hash())
                .or_insert_with(|| ScriptGroup::from_lock_script(&output.lock()));
            lock_group_entry.input_indices.push(i);
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(t));
                type_group_entry.input_indices.push(i);
            }
        }
        for (i, output) in rtx.transaction.outputs().into_iter().enumerate() {
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(t));
                type_group_entry.output_indices.push(i);
            }
        }

        let debug_printer: DebugPrinter = Arc::new(
            #[allow(unused_variables)]
            |hash: &Byte32, message: &str| {
                #[cfg(feature = "logging")]
                debug!("script group: {} DEBUG OUTPUT: {}", hash, message);
            },
        );
        #[cfg(test)]
        let skip_pause = Arc::new(AtomicBool::new(false));

        let syscalls_generator = TransactionScriptsSyscallsGenerator {
            base_cycles: Arc::new(Mutex::new(0)),
            data_loader: data_loader.clone(),
            debug_printer: Arc::clone(&debug_printer),
            message_box: Arc::new(Mutex::new(Vec::new())),
            outputs: Arc::clone(&outputs),
            rtx: Arc::clone(&rtx),
            #[cfg(test)]
            skip_pause: Arc::clone(&skip_pause),
            vm_id: FIRST_VM_ID,
        };

        TransactionScriptsVerifier {
            data_loader,
            binaries_by_data_hash,
            binaries_by_type_hash,
            rtx,
            lock_groups,
            type_groups,
            #[cfg(test)]
            skip_pause,
            consensus,
            tx_env,
            syscalls_generator,
        }
    }

    /// Sets a callback to handle the debug syscall.
    ///
    ///
    /// Script can print a message using the [debug syscall](github.com/nervosnetwork/rfcs/blob/master/rfcs/0009-vm-syscalls/0009-vm-syscalls.md#debug).
    ///
    /// The callback receives two parameters:
    ///
    /// * `hash: &Byte32`: this is the script hash of currently running script group.
    /// * `message: &str`: message passed to the debug syscall.
    pub fn set_debug_printer<F: Fn(&Byte32, &str) + Sync + Send + 'static>(&mut self, func: F) {
        self.syscalls_generator.debug_printer = Arc::new(func);
    }

    #[cfg(test)]
    pub(crate) fn set_skip_pause(&self, skip_pause: bool) {
        self.skip_pause.store(skip_pause, Ordering::SeqCst);
    }

    #[inline]
    #[allow(dead_code)]
    fn hash(&self) -> Byte32 {
        self.rtx.transaction.hash()
    }

    /// Extracts actual script binary either in dep cells.
    pub fn extract_script(&self, script: &Script) -> Result<Bytes, ScriptError> {
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data | ScriptHashType::Data1 | ScriptHashType::Data2 => {
                if let Some(lazy) = self.binaries_by_data_hash.get(&script.code_hash()) {
                    Ok(lazy.access(&self.data_loader)?)
                } else {
                    Err(ScriptError::ScriptNotFound(script.code_hash()))
                }
            }
            ScriptHashType::Type => {
                if let Some(ref bin) = self.binaries_by_type_hash.get(&script.code_hash()) {
                    match bin {
                        Binaries::Unique(_, ref lazy) => Ok(lazy.access(&self.data_loader)?),
                        Binaries::Duplicate(_, ref lazy) => Ok(lazy.access(&self.data_loader)?),
                        Binaries::Multiple => Err(ScriptError::MultipleMatches),
                    }
                } else {
                    Err(ScriptError::ScriptNotFound(script.code_hash()))
                }
            }
        }
    }

    fn is_vm_version_1_and_syscalls_2_enabled(&self) -> bool {
        // If the proposal window is allowed to prejudge on the vm version,
        // it will cause proposal tx to start a new vm in the blocks before hardfork,
        // destroying the assumption that the transaction execution only uses the old vm
        // before hardfork, leading to unexpected network splits.
        let epoch_number = self.tx_env.epoch_number_without_proposal_window();
        let hardfork_switch = self.consensus.hardfork_switch();
        hardfork_switch
            .ckb2021
            .is_vm_version_1_and_syscalls_2_enabled(epoch_number)
    }

    fn is_vm_version_2_and_syscalls_3_enabled(&self) -> bool {
        // If the proposal window is allowed to prejudge on the vm version,
        // it will cause proposal tx to start a new vm in the blocks before hardfork,
        // destroying the assumption that the transaction execution only uses the old vm
        // before hardfork, leading to unexpected network splits.
        let epoch_number = self.tx_env.epoch_number_without_proposal_window();
        let hardfork_switch = self.consensus.hardfork_switch();
        hardfork_switch
            .ckb2023
            .is_vm_version_2_and_syscalls_3_enabled(epoch_number)
    }

    /// Returns the version of the machine based on the script and the consensus rules.
    pub fn select_version(&self, script: &Script) -> Result<ScriptVersion, ScriptError> {
        let is_vm_version_2_and_syscalls_3_enabled = self.is_vm_version_2_and_syscalls_3_enabled();
        let is_vm_version_1_and_syscalls_2_enabled = self.is_vm_version_1_and_syscalls_2_enabled();
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data => Ok(ScriptVersion::V0),
            ScriptHashType::Data1 => {
                if is_vm_version_1_and_syscalls_2_enabled {
                    Ok(ScriptVersion::V1)
                } else {
                    Err(ScriptError::InvalidVmVersion(1))
                }
            }
            ScriptHashType::Data2 => {
                if is_vm_version_2_and_syscalls_3_enabled {
                    Ok(ScriptVersion::V2)
                } else {
                    Err(ScriptError::InvalidVmVersion(2))
                }
            }
            ScriptHashType::Type => {
                if is_vm_version_2_and_syscalls_3_enabled {
                    Ok(ScriptVersion::V2)
                } else if is_vm_version_1_and_syscalls_2_enabled {
                    Ok(ScriptVersion::V1)
                } else {
                    Ok(ScriptVersion::V0)
                }
            }
        }
    }

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

    /// Performing a resumable verification on the transaction scripts.
    ///
    /// ## Params
    ///
    /// * `limit_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    ///   when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a state will returned.
    pub fn resumable_verify(&self, limit_cycles: Cycle) -> Result<VerifyResult, Error> {
        let mut cycles = 0;
        let mut current_consumed_cycles = 0;

        let groups: Vec<_> = self.groups().collect();
        for (idx, (_hash, group)) in groups.iter().enumerate() {
            // vm should early return invalid cycles
            let remain_cycles = limit_cycles
                .checked_sub(current_consumed_cycles)
                .ok_or_else(|| {
                    ScriptError::Other(format!("expect invalid cycles {limit_cycles} {cycles}"))
                        .source(group)
                })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles, consumed_cycles)) => {
                    current_consumed_cycles =
                        wrapping_cycles_add(current_consumed_cycles, consumed_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(state)) => {
                    let current = idx;
                    let state = TransactionState::new(state, current, cycles, remain_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
                    #[cfg(feature = "logging")]
                    logging::on_script_error(_hash, &self.hash(), &e);
                    return Err(e.source(group).into());
                }
            }
        }

        Ok(VerifyResult::Completed(cycles))
    }

    /// Performing a resumable verification on the transaction scripts with signal channel,
    /// if `Suspend` comes from `command_rx`, the process will be hang up until `Resume` comes,
    /// otherwise, it will return until the verification is completed.
    #[cfg(not(target_family = "wasm"))]
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

    /// Resuming an suspended verify from snapshot
    ///
    /// ## Params
    ///
    /// * `snap` - Captured transaction verification state.
    ///
    /// * `limit_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    ///   when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a borrowed state will returned.
    pub fn resume_from_snap(
        &self,
        snap: &TransactionState,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult, Error> {
        let mut cycles = snap.current_cycles;
        let mut current_used = 0;

        let (_hash, current_group) = self.groups().nth(snap.current).ok_or_else(|| {
            ScriptError::Other(format!("snapshot group missing {:?}", snap.current))
                .unknown_source()
        })?;

        // continue snapshot current script
        match self.verify_group_with_chunk(current_group, limit_cycles, &snap.state) {
            Ok(ChunkState::Completed(used_cycles, consumed_cycles)) => {
                current_used = wrapping_cycles_add(current_used, consumed_cycles, current_group)?;
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(state)) => {
                let current = snap.current;
                let state = TransactionState::new(state, current, cycles, limit_cycles);
                return Ok(VerifyResult::Suspended(state));
            }
            Err(e) => {
                #[cfg(feature = "logging")]
                logging::on_script_error(_hash, &self.hash(), &e);
                return Err(e.source(current_group).into());
            }
        }

        for (idx, (_hash, group)) in self.groups().enumerate().skip(snap.current + 1) {
            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::Other(format!("expect invalid cycles {limit_cycles} {cycles}"))
                    .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles, consumed_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, consumed_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(state)) => {
                    let current = idx;
                    let state = TransactionState::new(state, current, cycles, remain_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
                    #[cfg(feature = "logging")]
                    logging::on_script_error(_hash, &self.hash(), &e);
                    return Err(e.source(group).into());
                }
            }
        }

        Ok(VerifyResult::Completed(cycles))
    }

    /// Resuming an suspended verify from vm state
    ///
    /// ## Params
    ///
    /// * `state` - vm state.
    ///
    /// * `limit_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    ///   when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a borrowed state will returned.
    pub fn resume_from_state(
        &self,
        state: TransactionState,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult, Error> {
        let TransactionState {
            current,
            state,
            current_cycles,
            ..
        } = state;

        let mut current_used = 0;
        let mut cycles = current_cycles;

        let (_hash, current_group) = self.groups().nth(current).ok_or_else(|| {
            ScriptError::Other(format!("snapshot group missing {current:?}")).unknown_source()
        })?;

        let resumed_script_result =
            self.verify_group_with_chunk(current_group, limit_cycles, &state);

        match resumed_script_result {
            Ok(ChunkState::Completed(used_cycles, consumed_cycles)) => {
                current_used = wrapping_cycles_add(current_used, consumed_cycles, current_group)?;
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(state)) => {
                let state = TransactionState::new(state, current, cycles, limit_cycles);
                return Ok(VerifyResult::Suspended(state));
            }
            Err(e) => {
                #[cfg(feature = "logging")]
                logging::on_script_error(_hash, &self.hash(), &e);
                return Err(e.source(current_group).into());
            }
        }

        for (idx, (_hash, group)) in self.groups().enumerate().skip(current + 1) {
            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::Other(format!(
                    "expect invalid cycles {limit_cycles} {current_used} {cycles}"
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(_, consumed_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, consumed_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, consumed_cycles, group)?;
                }
                Ok(ChunkState::Suspended(state)) => {
                    let current = idx;
                    let state = TransactionState::new(state, current, cycles, remain_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
                    #[cfg(feature = "logging")]
                    logging::on_script_error(_hash, &self.hash(), &e);
                    return Err(e.source(group).into());
                }
            }
        }

        Ok(VerifyResult::Completed(cycles))
    }

    /// Complete an suspended verify
    ///
    /// ## Params
    ///
    /// * `snap` - Captured transaction verification state.
    ///
    /// * `max_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    ///   when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles on completed, Otherwise it returns the verification error.
    pub fn complete(&self, snap: &TransactionState, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles = snap.current_cycles;

        let (_hash, current_group) = self.groups().nth(snap.current).ok_or_else(|| {
            ScriptError::Other(format!("snapshot group missing {:?}", snap.current))
                .unknown_source()
        })?;

        if max_cycles < cycles {
            return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                .source(current_group)
                .into());
        }

        // continue snapshot current script
        // max_cycles - cycles checked
        match self.verify_group_with_chunk(current_group, max_cycles - cycles, &snap.state) {
            Ok(ChunkState::Completed(used_cycles, _consumed_cycles)) => {
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(_)) => {
                return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                    .source(current_group)
                    .into());
            }
            Err(e) => {
                #[cfg(feature = "logging")]
                logging::on_script_error(_hash, &self.hash(), &e);
                return Err(e.source(current_group).into());
            }
        }

        for (_hash, group) in self.groups().skip(snap.current + 1) {
            let remain_cycles = max_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::Other(format!("expect invalid cycles {max_cycles} {cycles}"))
                    .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles, _consumed_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
                }
                Ok(ChunkState::Suspended(_)) => {
                    return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                        .source(group)
                        .into());
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
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: &self.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify()
        } else {
            self.run(group, max_cycles)
        }
    }
    /// Returns all script groups.
    pub fn groups(&self) -> impl Iterator<Item = (&'_ Byte32, &'_ ScriptGroup)> {
        self.lock_groups.iter().chain(self.type_groups.iter())
    }

    /// Returns all script groups with type.
    pub fn groups_with_type(
        &self,
    ) -> impl Iterator<Item = (ScriptGroupType, &'_ Byte32, &'_ ScriptGroup)> {
        self.lock_groups
            .iter()
            .map(|(hash, group)| (ScriptGroupType::Lock, hash, group))
            .chain(
                self.type_groups
                    .iter()
                    .map(|(hash, group)| (ScriptGroupType::Type, hash, group)),
            )
    }

    fn verify_group_with_chunk(
        &self,
        group: &ScriptGroup,
        max_cycles: Cycle,
        state: &Option<FullSuspendedState>,
    ) -> Result<ChunkState, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: &self.rtx,
                script_group: group,
                max_cycles,
            };
            match verifier.verify() {
                Ok(cycles) => Ok(ChunkState::Completed(cycles, cycles)),
                Err(ScriptError::ExceededMaximumCycles(_)) => Ok(ChunkState::suspended_type_id()),
                Err(e) => Err(e),
            }
        } else {
            self.chunk_run(group, max_cycles, state)
        }
    }

    fn chunk_run(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
        state: &Option<FullSuspendedState>,
    ) -> Result<ChunkState, ScriptError> {
        let program = self.extract_script(&script_group.script)?;
        let tx_data = TxData {
            rtx: Arc::clone(&self.rtx),
            data_loader: self.data_loader.clone(),
            program,
            script_group: Arc::new(script_group.clone()),
        };
        let version = self.select_version(&script_group.script)?;
        let mut scheduler = if let Some(state) = state {
            Scheduler::resume(
                tx_data,
                version,
                self.syscalls_generator.clone(),
                state.clone(),
            )
        } else {
            Scheduler::new(tx_data, version, self.syscalls_generator.clone())
        };
        let previous_cycles = scheduler.consumed_cycles();
        let res = scheduler.run(RunMode::LimitCycles(max_cycles));
        match res {
            Ok((exit_code, cycles)) => {
                if exit_code == 0 {
                    Ok(ChunkState::Completed(
                        cycles,
                        scheduler.consumed_cycles() - previous_cycles,
                    ))
                } else {
                    Err(ScriptError::validation_failure(
                        &script_group.script,
                        exit_code,
                    ))
                }
            }
            Err(error) => match error {
                VMInternalError::CyclesExceeded | VMInternalError::Pause => {
                    let snapshot = scheduler
                        .suspend()
                        .map_err(|err| self.map_vm_internal_error(err, max_cycles))?;
                    Ok(ChunkState::suspended(snapshot))
                }
                _ => Err(self.map_vm_internal_error(error, max_cycles)),
            },
        }
    }

    #[cfg(not(target_family = "wasm"))]
    async fn verify_group_with_signal(
        &self,
        group: &ScriptGroup,
        max_cycles: Cycle,
        command_rx: &mut Receiver<ChunkCommand>,
    ) -> Result<Cycle, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: &self.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify()
        } else {
            self.chunk_run_with_signal(group, max_cycles, command_rx)
                .await
        }
    }

    /// Finds the script group from cell deps.
    pub fn find_script_group(
        &self,
        script_group_type: ScriptGroupType,
        script_hash: &Byte32,
    ) -> Option<&ScriptGroup> {
        match script_group_type {
            ScriptGroupType::Lock => self.lock_groups.get(script_hash),
            ScriptGroupType::Type => self.type_groups.get(script_hash),
        }
    }

    /// Prepares syscalls.
    pub fn generate_syscalls(
        &self,
        script_version: ScriptVersion,
        script_group: &ScriptGroup,
        snapshot2_context: Arc<Mutex<Snapshot2Context<DataPieceId, TxData<DL>>>>,
    ) -> Vec<Box<(dyn Syscalls<CoreMachine>)>> {
        self.syscalls_generator
            .generate_syscalls(script_version, script_group, snapshot2_context)
    }

    /// Create a scheduler to manage virtual machine instances.
    pub fn create_scheduler(
        &self,
        script_group: &ScriptGroup,
    ) -> Result<Scheduler<DL>, ScriptError> {
        let program = self.extract_script(&script_group.script)?;
        let tx_data = TxData {
            rtx: Arc::clone(&self.rtx),
            data_loader: self.data_loader.clone(),
            program,
            script_group: Arc::new(script_group.clone()),
        };
        let version = self.select_version(&script_group.script)?;
        Ok(Scheduler::new(
            tx_data,
            version,
            self.syscalls_generator.clone(),
        ))
    }

    /// Runs a single program, then returns the exit code together with the entire
    /// machine to the caller for more inspections.
    pub fn detailed_run(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<(i8, Cycle), ScriptError> {
        let mut scheduler = self.create_scheduler(script_group)?;
        scheduler
            .run(RunMode::LimitCycles(max_cycles))
            .map_err(|err| self.map_vm_internal_error(err, max_cycles))
    }

    fn run(&self, script_group: &ScriptGroup, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let (code, cycles) = self.detailed_run(script_group, max_cycles)?;

        if code == 0 {
            Ok(cycles)
        } else {
            Err(ScriptError::validation_failure(&script_group.script, code))
        }
    }

    fn map_vm_internal_error(&self, error: VMInternalError, max_cycles: Cycle) -> ScriptError {
        match error {
            VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
            VMInternalError::External(reason) if reason.eq("stopped") => ScriptError::Interrupts,
            _ => ScriptError::VMInternalError(error),
        }
    }

    #[cfg(not(target_family = "wasm"))]
    async fn chunk_run_with_signal(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
        signal: &mut Receiver<ChunkCommand>,
    ) -> Result<Cycle, ScriptError> {
        let program = self.extract_script(&script_group.script)?;
        let tx_data = TxData {
            rtx: Arc::clone(&self.rtx),
            data_loader: self.data_loader.clone(),
            program,
            script_group: Arc::new(script_group.clone()),
        };
        let version = self.select_version(&script_group.script)?;
        let mut scheduler = Scheduler::new(tx_data, version, self.syscalls_generator.clone());
        let mut pause = VMPause::new();
        let child_pause = pause.clone();
        let (finish_tx, mut finish_rx) = oneshot::channel::<Result<(i8, Cycle), ckb_vm::Error>>();

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
                        let res = scheduler.run(RunMode::Pause(pause_cloned));
                        match res {
                            Ok(_) => {
                                let _ = finish_tx.send(res);
                                return;
                            }
                            Err(VMInternalError::Pause) => {
                                // continue to wait for
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
                        Ok((0, cycles)) => {
                            return Ok(cycles);
                        }
                        Ok((exit_code, _cycles)) => {
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
    use super::{info, Byte32, ScriptError};

    pub fn on_script_error(group: &Byte32, tx: &Byte32, error: &ScriptError) {
        info!(
            "Error validating script group {} of transaction {}: {}",
            group, tx, error
        );
    }
}
