#[cfg(test)]
use crate::syscalls::Pause;
use crate::{
    cost_model::transferred_byte_cycles,
    error::{ScriptError, TransactionScriptError},
    syscalls::{
        spawn::{build_child_machine, update_caller_machine},
        CurrentCycles, Debugger, Exec, GetMemoryLimit, LoadCell, LoadCellData, LoadExtension,
        LoadHeader, LoadInput, LoadScript, LoadScriptHash, LoadTx, LoadWitness, PeakMemory,
        SetContent, Spawn, VMVersion,
    },
    type_id::TypeIdSystemScript,
    types::{
        CoreMachine, DebugPrinter, Indices, Machine, MachineContext, ResumableMachine, ResumePoint,
        ScriptGroup, ScriptGroupType, ScriptVersion, SpawnData, TransactionSnapshot,
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
use ckb_vm::{
    cost_model::estimate_cycles,
    snapshot::{resume, Snapshot},
    DefaultMachineBuilder, Error as VMInternalError, SupportMachine, Syscalls,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

#[cfg(test)]
use core::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
mod tests;

pub enum ChunkState {
    Suspended(Vec<ResumableMachine>, Arc<Mutex<MachineContext>>),
    Completed(Cycle),
}

impl ChunkState {
    pub fn suspended(machines: Vec<ResumableMachine>, context: Arc<Mutex<MachineContext>>) -> Self {
        ChunkState::Suspended(machines, context)
    }

    pub fn suspended_type_id() -> Self {
        ChunkState::Suspended(vec![], Default::default())
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum DataGuard {
    NotLoaded(OutPoint),
    Loaded(Bytes),
}

/// LazyData wrapper make sure not-loaded data will be loaded only after one access
#[derive(Debug, PartialEq, Eq, Clone)]
struct LazyData(RefCell<DataGuard>);

impl LazyData {
    fn from_cell_meta(cell_meta: &CellMeta) -> LazyData {
        match &cell_meta.mem_cell_data {
            Some(data) => LazyData(RefCell::new(DataGuard::Loaded(data.to_owned()))),
            None => LazyData(RefCell::new(DataGuard::NotLoaded(
                cell_meta.out_point.clone(),
            ))),
        }
    }

    fn access<DL: CellDataProvider>(&self, data_loader: &DL) -> Bytes {
        let guard = self.0.borrow().to_owned();
        match guard {
            DataGuard::NotLoaded(out_point) => {
                let data = data_loader.get_cell_data(&out_point).expect("cell data");
                self.0.replace(DataGuard::Loaded(data.to_owned()));
                data
            }
            DataGuard::Loaded(bytes) => bytes,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
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
pub struct TransactionScriptsSyscallsGenerator<DL> {
    pub(crate) data_loader: DL,
    debug_printer: DebugPrinter,
    pub(crate) outputs: Arc<Vec<CellMeta>>,
    pub(crate) rtx: Arc<ResolvedTransaction>,
    #[cfg(test)]
    skip_pause: Arc<AtomicBool>,
}

impl<DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static>
    TransactionScriptsSyscallsGenerator<DL>
{
    /// Build syscall: current_cycles
    pub fn build_current_cycles(&self) -> CurrentCycles {
        CurrentCycles::new()
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
        group_inputs: Indices,
        group_outputs: Indices,
    ) -> LoadCellData<DL> {
        LoadCellData::new(
            self.data_loader.clone(),
            Arc::clone(&self.rtx),
            Arc::clone(&self.outputs),
            group_inputs,
            group_outputs,
        )
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

    /// Build syscall: load_extension
    pub fn build_load_extension(&self, group_inputs: Indices) -> LoadExtension<DL> {
        LoadExtension::new(
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

    /// Build syscall: get_memory_limit
    pub fn build_get_memory_limit(&self, memory_limit: u64) -> GetMemoryLimit {
        GetMemoryLimit::new(memory_limit)
    }

    /// Build syscall: set_content
    pub fn build_set_content(
        &self,
        content: Arc<Mutex<Vec<u8>>>,
        content_length: u64,
    ) -> SetContent {
        SetContent::new(content, content_length)
    }

    /// Build syscall: spawn
    pub fn build_spawn(
        &self,
        script_version: ScriptVersion,
        script_group: &ScriptGroup,
        peak_memory: u64,
        context: Arc<Mutex<MachineContext>>,
    ) -> Spawn<DL> {
        Spawn::new(
            script_group.clone(),
            script_version,
            self.clone(),
            peak_memory,
            context,
        )
    }

    /// Build syscall: peak_memory
    pub fn build_peak_memory(&self, peak_memory: u64) -> PeakMemory {
        PeakMemory::new(peak_memory)
    }

    /// Generate same syscalls. The result does not contain spawn syscalls.
    pub fn generate_same_syscalls(
        &self,
        script_version: ScriptVersion,
        script_group: &ScriptGroup,
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
            Box::new(self.build_load_cell_data(
                Arc::clone(&script_group_input_indices),
                Arc::clone(&script_group_output_indices),
            )),
            Box::new(Debugger::new(
                current_script_hash,
                Arc::clone(&self.debug_printer),
            )),
        ];
        #[cfg(test)]
        syscalls.push(Box::new(Pause::new(Arc::clone(&self.skip_pause))));
        if script_version >= ScriptVersion::V1 {
            syscalls.append(&mut vec![
                Box::new(self.build_vm_version()),
                Box::new(self.build_current_cycles()),
                Box::new(self.build_exec(
                    Arc::clone(&script_group_input_indices),
                    Arc::clone(&script_group_output_indices),
                )),
            ]);
        }

        if script_version >= ScriptVersion::V2 {
            syscalls.push(Box::new(
                self.build_load_extension(Arc::clone(&script_group_input_indices)),
            ));
        }
        syscalls
    }

    /// Generate root syscalls.
    pub fn generate_root_syscalls(
        &self,
        script_version: ScriptVersion,
        script_group: &ScriptGroup,
        context: Arc<Mutex<MachineContext>>,
    ) -> Vec<Box<(dyn Syscalls<CoreMachine>)>> {
        let mut syscalls = self.generate_same_syscalls(script_version, script_group);
        if script_version >= ScriptVersion::V2 {
            syscalls.append(&mut vec![
                Box::new(self.build_get_memory_limit(8)),
                Box::new(self.build_set_content(Arc::new(Mutex::new(vec![])), 0)),
                Box::new(self.build_spawn(script_version, script_group, 8, Arc::clone(&context))),
                Box::new(self.build_peak_memory(8)),
            ])
        }
        syscalls
    }
}

/// This struct leverages CKB VM to verify transaction inputs.
///
/// FlatBufferBuilder owned `Vec<u8>` that grows as needed, in the
/// future, we might refactor this to share buffer to achieve zero-copy
pub struct TransactionScriptsVerifier<DL> {
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

    generator: TransactionScriptsSyscallsGenerator<DL>,
}

impl<DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static>
    TransactionScriptsVerifier<DL>
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

        let generator = TransactionScriptsSyscallsGenerator {
            data_loader: data_loader.clone(),
            debug_printer: Arc::clone(&debug_printer),
            outputs: Arc::clone(&outputs),
            rtx: Arc::clone(&rtx),
            #[cfg(test)]
            skip_pause: Arc::clone(&skip_pause),
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
            generator,
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
        self.generator.debug_printer = Arc::new(func);
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
                    Ok(lazy.access(&self.data_loader))
                } else {
                    Err(ScriptError::ScriptNotFound(script.code_hash()))
                }
            }
            ScriptHashType::Type => {
                if let Some(ref bin) = self.binaries_by_type_hash.get(&script.code_hash()) {
                    match bin {
                        Binaries::Unique(_, ref lazy) => Ok(lazy.access(&self.data_loader)),
                        Binaries::Duplicate(_, ref lazy) => Ok(lazy.access(&self.data_loader)),
                        Binaries::Multiple => Err(ScriptError::MultipleMatches),
                    }
                } else {
                    Err(ScriptError::ScriptNotFound(script.code_hash()))
                }
            }
        }
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
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data => Ok(ScriptVersion::V0),
            ScriptHashType::Data1 => Ok(ScriptVersion::V1),
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
                } else {
                    Ok(ScriptVersion::V1)
                }
            }
        }
    }

    /// Verifies the transaction by running scripts.
    ///
    /// ## Params
    ///
    /// * `max_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
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
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a state will returned.
    pub fn resumable_verify(&self, limit_cycles: Cycle) -> Result<VerifyResult, Error> {
        let mut cycles = 0;

        let groups: Vec<_> = self.groups().collect();
        for (idx, (_hash, group)) in groups.iter().enumerate() {
            // vm should early return invalid cycles
            let remain_cycles = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {limit_cycles} {cycles}"
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &[]) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vms, context)) => {
                    let current = idx;
                    let state = TransactionState::new(vms, context, current, cycles, remain_cycles);
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

    /// Resuming an suspended verify from snapshot
    ///
    /// ## Params
    ///
    /// * `snap` - Captured transaction verification snapshot.
    ///
    /// * `limit_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a borrowed state will returned.
    pub fn resume_from_snap(
        &self,
        snap: &TransactionSnapshot,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult, Error> {
        let current_group_used = snap.snaps.iter().map(|s| s.1).sum();
        let mut cycles = snap.current_cycles;
        let mut current_used = 0;

        let (_hash, current_group) = self.groups().nth(snap.current).ok_or_else(|| {
            ScriptError::VMInternalError(format!("snapshot group missing {:?}", snap.current))
                .unknown_source()
        })?;

        // continue snapshot current script
        match self.verify_group_with_chunk(current_group, limit_cycles, &snap.snaps) {
            Ok(ChunkState::Completed(used_cycles)) => {
                current_used = wrapping_cycles_add(
                    current_used,
                    wrapping_cycles_sub(used_cycles, current_group_used, current_group)?,
                    current_group,
                )?;
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(vms, context)) => {
                let current = snap.current;
                let state = TransactionState::new(vms, context, current, cycles, limit_cycles);
                return Ok(VerifyResult::Suspended(state));
            }
            Err(e) => {
                #[cfg(feature = "logging")]
                logging::on_script_error(_hash, &self.hash(), &e);
                return Err(e.source(current_group).into());
            }
        }

        let skip = snap.current + 1;
        for (idx, (_hash, group)) in self.groups().enumerate().skip(skip) {
            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {limit_cycles} {cycles}"
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &[]) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vms, context)) => {
                    let current = idx;
                    let state = TransactionState::new(vms, context, current, cycles, remain_cycles);
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
    /// when the consumed cycles exceed the limit.
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
            mut vms,
            current_cycles,
            machine_context,
            ..
        } = state;

        let mut current_used = 0;
        let mut cycles = current_cycles;

        let (_hash, current_group) = self.groups().nth(current).ok_or_else(|| {
            ScriptError::VMInternalError(format!("snapshot group missing {current:?}"))
                .unknown_source()
        })?;

        let resumed_script_result = if vms.is_empty() {
            self.verify_group_with_chunk(current_group, limit_cycles, &[])
        } else {
            vms.iter_mut()
                .for_each(|vm| vm.set_max_cycles(limit_cycles));
            run_vms(current_group, limit_cycles, vms, &machine_context)
        };
        match resumed_script_result {
            Ok(ChunkState::Completed(used_cycles)) => {
                current_used = wrapping_cycles_add(current_used, used_cycles, current_group)?;
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(vms, context)) => {
                let state = TransactionState::new(vms, context, current, cycles, limit_cycles);
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
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {limit_cycles} {cycles}"
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &[]) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vms, context)) => {
                    let current = idx;
                    let state = TransactionState::new(vms, context, current, cycles, remain_cycles);
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
    /// * `snap` - Captured transaction verification snapshot.
    ///
    /// * `max_cycles` - Maximum allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles on completed, Otherwise it returns the verification error.
    pub fn complete(&self, snap: &TransactionSnapshot, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles = snap.current_cycles;

        let (_hash, current_group) = self.groups().nth(snap.current).ok_or_else(|| {
            ScriptError::VMInternalError(format!("snapshot group missing {:?}", snap.current))
                .unknown_source()
        })?;

        if max_cycles < cycles {
            return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                .source(current_group)
                .into());
        }

        // continue snapshot current script
        // max_cycles - cycles checked
        match self.verify_group_with_chunk(current_group, max_cycles - cycles, &snap.snaps) {
            Ok(ChunkState::Completed(used_cycles)) => {
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(_, _)) => {
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
                ScriptError::VMInternalError(format!("expect invalid cycles {max_cycles} {cycles}"))
                    .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &[]) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
                }
                Ok(ChunkState::Suspended(_, _)) => {
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
        snaps: &[(Snapshot, Cycle, ResumePoint)],
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
                Ok(cycles) => Ok(ChunkState::Completed(cycles)),
                Err(ScriptError::ExceededMaximumCycles(_)) => Ok(ChunkState::suspended_type_id()),
                Err(e) => Err(e),
            }
        } else {
            self.chunk_run(group, max_cycles, snaps)
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
        context: Arc<Mutex<MachineContext>>,
    ) -> Vec<Box<(dyn Syscalls<CoreMachine>)>> {
        self.generator
            .generate_root_syscalls(script_version, script_group, context)
    }

    fn build_machine(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
        context: Arc<Mutex<MachineContext>>,
    ) -> Result<Machine, ScriptError> {
        let script_version = self.select_version(&script_group.script)?;
        let core_machine = script_version.init_core_machine(max_cycles);
        let machine_builder = DefaultMachineBuilder::<CoreMachine>::new(core_machine)
            .instruction_cycle_func(Box::new(estimate_cycles));
        let syscalls = self.generate_syscalls(script_version, script_group, context);
        let machine_builder = syscalls
            .into_iter()
            .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
        let default_machine = machine_builder.build();
        let machine = Machine::new(default_machine);
        Ok(machine)
    }

    fn run(&self, script_group: &ScriptGroup, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let program = self.extract_script(&script_group.script)?;
        let context = Default::default();
        let mut machine = self.build_machine(script_group, max_cycles, context)?;

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{error:?}")),
        };

        let bytes = machine
            .load_program(&program, &[])
            .map_err(map_vm_internal_error)?;
        machine
            .machine
            .add_cycles_no_checking(transferred_byte_cycles(bytes))
            .map_err(map_vm_internal_error)?;
        let code = machine.run().map_err(map_vm_internal_error)?;
        if code == 0 {
            Ok(machine.machine.cycles())
        } else {
            Err(ScriptError::validation_failure(&script_group.script, code))
        }
    }

    fn chunk_run(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
        snaps: &[(Snapshot, Cycle, ResumePoint)],
    ) -> Result<ChunkState, ScriptError> {
        let context: Arc<Mutex<MachineContext>> = Default::default();

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{error:?}")),
        };

        let machines = if !snaps.is_empty() {
            // Resume machines from snapshots
            let mut machines = vec![];
            for (sp, current_cycle, resume_point) in snaps {
                let mut machine = match resume_point {
                    ResumePoint::Initial => ResumableMachine::initial(self.build_machine(
                        script_group,
                        max_cycles,
                        Arc::clone(&context),
                    )?),
                    ResumePoint::Spawn {
                        callee_peak_memory,
                        callee_memory_limit,
                        content,
                        content_length,
                        caller_exit_code_addr,
                        caller_content_addr,
                        caller_content_length_addr,
                    } => {
                        let spawn_data = SpawnData {
                            callee_peak_memory: *callee_peak_memory,
                            callee_memory_limit: *callee_memory_limit,
                            content: Arc::new(Mutex::new(content.clone())),
                            content_length: *content_length,
                            caller_exit_code_addr: *caller_exit_code_addr,
                            caller_content_addr: *caller_content_addr,
                            caller_content_length_addr: *caller_content_length_addr,
                        };
                        let machine = build_child_machine(
                            script_group,
                            self.select_version(&script_group.script)?,
                            &self.generator,
                            max_cycles,
                            &spawn_data,
                            &context,
                        )
                        .map_err(map_vm_internal_error)?;
                        ResumableMachine::spawn(machine, spawn_data)
                    }
                };
                resume(&mut machine.machine_mut().machine, sp).map_err(map_vm_internal_error)?;
                machine.machine_mut().machine.set_cycles(*current_cycle);
                machines.push(machine);
            }
            machines
        } else {
            // No shapshots are available, create machine from scratch
            let mut machine = self.build_machine(script_group, max_cycles, Arc::clone(&context))?;
            let program = self.extract_script(&script_group.script)?;
            let bytes = machine
                .load_program(&program, &[])
                .map_err(map_vm_internal_error)?;
            let program_bytes_cycles = transferred_byte_cycles(bytes);
            // NOTE: previously, we made a distinction between machines
            // that completes program loading without errors, and machines
            // that fail program loading due to cycle limits. For the latter
            // one, we won't generate any snapshots. Starting from this version,
            // we will remove this distinction: when loading program exceeds
            // maximum cycles, the error will be triggered when executing the
            // first instruction. As a result, now all ResumableMachine will
            // be transformed to snapshots. This is due to several considerations:
            //
            // * Let's do a little bit math: right now CKB has a block limit of
            // ~570KB, a single transaction is further limited to 512KB in RPC,
            // the biggest program one can load is either 512KB or ~570KB depending
            // on which limit to use. The cycles consumed to load a program, is
            // thus at most 131072 or ~145920, which is far less than the cycle
            // limit for running a single transaction (70 million or more). In
            // reality it might be extremely rare that loading a program would
            // result in exceeding cycle limits. Removing the distinction here,
            // would help simply the code.
            // * If you pay attention to the code now, we already have this behavior
            // in the code: most syscalls use +add_cycles_no_checking+ in the code,
            // meaning an error would not be immediately generated when cycle limit
            // is reached, the error would be raised when executing the first instruction
            // after the syscall. What's more, when spawn is loading a program
            // to its child machine, it also uses +add_cycles_no_checking+ so it
            // won't generate errors immediately. This means that all spawned machines
            // will be in a state that a program is loaded, regardless of the fact if
            // loading a program in spawn reaches the cycle limit or not. As a
            // result, we definitely want to pull the trigger, so we can have unified
            // behavior everywhere.
            machine
                .machine
                .add_cycles_no_checking(program_bytes_cycles)
                .map_err(|e| ScriptError::VMInternalError(format!("{e:?}")))?;
            vec![ResumableMachine::initial(machine)]
        };

        run_vms(script_group, max_cycles, machines, &context)
    }
}

// Run a series of VMs that are just freshly resumed
fn run_vms(
    script_group: &ScriptGroup,
    max_cycles: Cycle,
    mut machines: Vec<ResumableMachine>,
    context: &Arc<Mutex<MachineContext>>,
) -> Result<ChunkState, ScriptError> {
    let (mut exit_code, mut cycles, mut spawn_data) = (0, 0, None);

    if machines.is_empty() {
        return Err(ScriptError::VMInternalError(
            "To resume VMs, at least one VM must be available!".to_string(),
        ));
    }

    let map_vm_internal_error = |error: VMInternalError| match error {
        VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
        _ => ScriptError::VMInternalError(format!("{error:?}")),
    };

    while let Some(mut machine) = machines.pop() {
        if let Some(callee_spawn_data) = &spawn_data {
            update_caller_machine(
                &mut machine.machine_mut().machine,
                exit_code,
                cycles,
                callee_spawn_data,
            )
            .map_err(map_vm_internal_error)?;
        }

        match machine.run() {
            Ok(code) => {
                exit_code = code;
                cycles = machine.cycles();
                if let ResumableMachine::Spawn(_, data) = machine {
                    spawn_data = Some(data);
                } else {
                    spawn_data = None;
                }
            }
            Err(error) => match error {
                VMInternalError::CyclesExceeded => {
                    let mut new_suspended_machines: Vec<_> = {
                        let mut context = context.lock().map_err(|e| {
                            ScriptError::VMInternalError(format!("Failed to acquire lock: {}", e))
                        })?;
                        context.suspended_machines.drain(..).collect()
                    };
                    // The inner most machine lives at the top of the vector,
                    // reverse the list for natural order.
                    new_suspended_machines.reverse();
                    machines.push(machine);
                    machines.append(&mut new_suspended_machines);
                    return Ok(ChunkState::suspended(machines, Arc::clone(context)));
                }
                _ => return Err(ScriptError::VMInternalError(format!("{error:?}"))),
            },
        };
    }

    if exit_code == 0 {
        Ok(ChunkState::Completed(cycles))
    } else {
        Err(ScriptError::validation_failure(
            &script_group.script,
            exit_code,
        ))
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

fn wrapping_cycles_sub(
    lhs: Cycle,
    rhs: Cycle,
    group: &ScriptGroup,
) -> Result<Cycle, TransactionScriptError> {
    lhs.checked_sub(rhs)
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
