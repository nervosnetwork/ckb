#[cfg(test)]
use crate::syscalls::Pause;
use crate::{
    cost_model::{instruction_cycles, transferred_byte_cycles},
    error::{ScriptError, TransactionScriptError},
    syscalls::{
        CurrentCycles, Debugger, Exec, LoadCell, LoadCellData, LoadHeader, LoadInput, LoadScript,
        LoadScriptHash, LoadTx, LoadWitness, VMVersion,
    },
    type_id::TypeIdSystemScript,
    types::{
        CoreMachine, Machine, ResumableMachine, ScriptGroup, ScriptGroupType, ScriptVersion,
        TransactionSnapshot, TransactionState, VerifyResult,
    },
};
use ckb_chain_spec::consensus::TYPE_ID_CODE_HASH;
use ckb_error::Error;
#[cfg(feature = "logging")]
use ckb_logger::{debug, info};
use ckb_traits::{CellDataProvider, HeaderProvider};
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
    snapshot::{resume, Snapshot},
    DefaultMachineBuilder, Error as VMInternalError, InstructionCycleFunc, SupportMachine,
    Syscalls,
};
use std::rc::Rc;

type Indices = Rc<Vec<usize>>;

#[cfg(has_asm)]
use ckb_vm::machine::asm::AsmMachine;

#[cfg(not(has_asm))]
use ckb_vm::TraceMachine;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
mod tests;

pub enum ChunkState {
    Suspended(Option<ResumableMachine>),
    Completed(Cycle),
}

impl ChunkState {
    pub fn suspended(machine: ResumableMachine) -> Self {
        ChunkState::Suspended(Some(machine))
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

/// This struct leverages CKB VM to verify transaction inputs.
///
/// FlatBufferBuilder owned `Vec<u8>` that grows as needed, in the
/// future, we might refactor this to share buffer to achieve zero-copy
pub struct TransactionScriptsVerifier<DL> {
    data_loader: DL,
    debug_printer: Rc<dyn Fn(&Byte32, &str)>,

    outputs: Rc<Vec<CellMeta>>,
    rtx: Rc<ResolvedTransaction>,

    binaries_by_data_hash: HashMap<Byte32, LazyData>,
    binaries_by_type_hash: HashMap<Byte32, Binaries>,

    lock_groups: BTreeMap<Byte32, ScriptGroup>,
    type_groups: BTreeMap<Byte32, ScriptGroup>,

    #[cfg(test)]
    skip_pause: Rc<RefCell<bool>>,
}

impl<DL: CellDataProvider + HeaderProvider + Clone + 'static> TransactionScriptsVerifier<DL> {
    /// Creates a script verifier for the transaction.
    ///
    /// ## Params
    ///
    /// * `rtx` - transaction which cell out points have been resolved.
    /// * `data_loader` - used to load cell data.
    pub fn new(rtx: &ResolvedTransaction, data_loader: DL) -> TransactionScriptsVerifier<DL> {
        let tx_hash = rtx.transaction.hash();
        let resolved_cell_deps = &rtx.resolved_cell_deps;
        let resolved_inputs = &rtx.resolved_inputs;
        let outputs = Rc::new(
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

        TransactionScriptsVerifier {
            data_loader,
            binaries_by_data_hash,
            binaries_by_type_hash,
            outputs,
            lock_groups,
            type_groups,
            rtx: Rc::new(rtx.clone()),
            debug_printer: Rc::new(
                #[allow(unused_variables)]
                |hash: &Byte32, message: &str| {
                    #[cfg(feature = "logging")]
                    debug!("script group: {} DEBUG OUTPUT: {}", hash, message);
                },
            ),
            #[cfg(test)]
            skip_pause: Rc::new(RefCell::new(false)),
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
    pub fn set_debug_printer<F: Fn(&Byte32, &str) + 'static>(&mut self, func: F) {
        self.debug_printer = Rc::new(func);
    }

    #[cfg(test)]
    pub(crate) fn set_skip_pause(&mut self, skip_pause: bool) {
        *self.skip_pause.borrow_mut() = skip_pause;
    }

    #[inline]
    #[allow(dead_code)]
    fn hash(&self) -> Byte32 {
        self.rtx.transaction.hash()
    }

    fn build_current_cycles(&self) -> CurrentCycles {
        CurrentCycles::new()
    }

    fn build_vm_version(&self) -> VMVersion {
        VMVersion::new()
    }

    fn build_exec(&self, group_inputs: Indices, group_outputs: Indices) -> Exec<DL> {
        Exec::new(
            self.data_loader.clone(),
            Rc::clone(&self.rtx),
            Rc::clone(&self.outputs),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(Rc::clone(&self.rtx))
    }

    fn build_load_cell(&self, group_inputs: Indices, group_outputs: Indices) -> LoadCell<DL> {
        LoadCell::new(
            self.data_loader.clone(),
            Rc::clone(&self.rtx),
            Rc::clone(&self.outputs),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_cell_data(
        &self,
        group_inputs: Indices,
        group_outputs: Indices,
    ) -> LoadCellData<DL> {
        LoadCellData::new(
            self.data_loader.clone(),
            Rc::clone(&self.rtx),
            Rc::clone(&self.outputs),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_input(&self, group_inputs: Indices) -> LoadInput {
        LoadInput::new(Rc::clone(&self.rtx), group_inputs)
    }

    fn build_load_script_hash(&self, hash: Byte32) -> LoadScriptHash {
        LoadScriptHash::new(hash)
    }

    fn build_load_header(&self, group_inputs: Indices) -> LoadHeader<DL> {
        LoadHeader::new(
            self.data_loader.clone(),
            Rc::clone(&self.rtx),
            // self.header_deps(),
            // self.resolved_inputs(),
            // self.resolved_cell_deps(),
            group_inputs,
        )
    }

    fn build_load_witness(&self, group_inputs: Indices, group_outputs: Indices) -> LoadWitness {
        LoadWitness::new(Rc::clone(&self.rtx), group_inputs, group_outputs)
    }

    fn build_load_script(&self, script: Script) -> LoadScript {
        LoadScript::new(script)
    }

    /// Extracts actual script binary either in dep cells.
    pub fn extract_script(&self, script: &Script) -> Result<Bytes, ScriptError> {
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data | ScriptHashType::Data1 => {
                if let Some(lazy) = self.binaries_by_data_hash.get(&script.code_hash()) {
                    Ok(lazy.access(&self.data_loader))
                } else {
                    Err(ScriptError::InvalidCodeHash)
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
                    Err(ScriptError::InvalidCodeHash)
                }
            }
        }
    }

    /// Returns the version of the machine based on the script and the consensus rules.
    pub fn select_version(&self, script: &Script) -> Result<ScriptVersion, ScriptError> {
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data => Ok(ScriptVersion::V0),
            ScriptHashType::Data1 => Ok(ScriptVersion::V1),
            ScriptHashType::Type => Ok(ScriptVersion::V1),
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
                    info!(
                        "Error validating script group {} of transaction {}: {}",
                        _hash,
                        self.hash(),
                        e
                    );
                    e.source(group)
                })?;

            cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
        }
        Ok(cycles)
    }

    fn build_state(
        &self,
        vm: Option<ResumableMachine>,
        current: usize,
        current_cycles: Cycle,
        limit_cycles: Cycle,
    ) -> TransactionState {
        TransactionState {
            current,
            vm,
            current_cycles,
            limit_cycles,
        }
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
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let current = idx;
                    let state = self.build_state(vm, current, cycles, remain_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
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
        let current_group_used = snap.snap.as_ref().map(|s| s.1).unwrap_or_default();
        let mut cycles = snap.current_cycles;
        let mut current_used = 0;

        let current_group = self
            .groups()
            .nth(snap.current)
            .map(|(_hash, group)| group)
            .ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {:?}", snap.current))
                    .unknown_source()
            })?;

        // continue snapshot current script
        match self.verify_group_with_chunk(current_group, limit_cycles, &snap.snap) {
            Ok(ChunkState::Completed(used_cycles)) => {
                current_used = wrapping_cycles_add(
                    current_used,
                    wrapping_cycles_sub(used_cycles, current_group_used, current_group)?,
                    current_group,
                )?;
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(vm)) => {
                let current = snap.current;
                let state = self.build_state(vm, current, cycles, limit_cycles);
                return Ok(VerifyResult::Suspended(state));
            }
            Err(e) => {
                return Err(e.source(current_group).into());
            }
        }

        let skip = snap.current + 1;
        for (idx, (_hash, group)) in self.groups().enumerate().skip(skip) {
            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let current = idx;
                    let state = self.build_state(vm, current, cycles, remain_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
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
            vm,
            current_cycles,
            ..
        } = state;

        let mut current_used = 0;
        let mut cycles = current_cycles;

        let current_group = self
            .groups()
            .nth(current)
            .map(|(_hash, group)| group)
            .ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {:?}", current))
                    .unknown_source()
            })?;

        if let Some(mut vm) = vm {
            vm.set_max_cycles(limit_cycles);
            match vm.run() {
                Ok(code) => {
                    if code == 0 {
                        current_used =
                            wrapping_cycles_add(current_used, vm.cycles(), current_group)?;
                        cycles = wrapping_cycles_add(cycles, vm.cycles(), current_group)?;
                    } else {
                        return Err(ScriptError::validation_failure(&current_group.script, code)
                            .source(current_group)
                            .into());
                    }
                }
                Err(error) => match error {
                    VMInternalError::CyclesExceeded => {
                        let state = self.build_state(Some(vm), current, cycles, limit_cycles);
                        return Ok(VerifyResult::Suspended(state));
                    }
                    error => {
                        return Err(ScriptError::VMInternalError(format!("{:?}", error))
                            .source(current_group)
                            .into());
                    }
                },
            }
        } else {
            match self.verify_group_with_chunk(current_group, limit_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, current_group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let state = self.build_state(vm, current, cycles, limit_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
                    return Err(e.source(current_group).into());
                }
            }
        }

        for (idx, (_hash, group)) in self.groups().enumerate().skip(current + 1) {
            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let current = idx;
                    let state = self.build_state(vm, current, cycles, remain_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                Err(e) => {
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

        let current_group = self
            .groups()
            .nth(snap.current)
            .map(|(_hash, group)| group)
            .ok_or_else(|| {
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
        match self.verify_group_with_chunk(current_group, max_cycles - cycles, &snap.snap) {
            Ok(ChunkState::Completed(used_cycles)) => {
                cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
            }
            Ok(ChunkState::Suspended(_)) => {
                return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                    .source(current_group)
                    .into());
            }
            Err(e) => {
                return Err(e.source(current_group).into());
            }
        }

        for (_hash, group) in self.groups().skip(snap.current + 1) {
            let remain_cycles = max_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    max_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, current_group)?;
                }
                Ok(ChunkState::Suspended(_)) => {
                    return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                        .source(group)
                        .into());
                }
                Err(e) => {
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
            None => Err(ScriptError::InvalidCodeHash),
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
                rtx: Rc::clone(&self.rtx),
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
        snap: &Option<(Snapshot, Cycle)>,
    ) -> Result<ChunkState, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: Rc::clone(&self.rtx),
                script_group: group,
                max_cycles,
            };
            match verifier.verify() {
                Ok(cycles) => Ok(ChunkState::Completed(cycles)),
                Err(ScriptError::ExceededMaximumCycles(_)) => Ok(ChunkState::suspended_type_id()),
                Err(e) => Err(e),
            }
        } else {
            self.chunk_run(group, max_cycles, snap)
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

    /// Gets the cost model.
    ///
    /// Cost model is used to evaluate consumed cycles.
    pub fn cost_model(&self) -> Box<InstructionCycleFunc> {
        Box::new(instruction_cycles)
    }

    /// Prepares syscalls.
    pub fn generate_syscalls(
        &self,
        script_version: ScriptVersion,
        script_group: &ScriptGroup,
    ) -> Vec<Box<(dyn Syscalls<CoreMachine>)>> {
        let current_script_hash = script_group.script.calc_script_hash();
        let script_group_input_indices = Rc::new(script_group.input_indices.clone());
        let script_group_output_indices = Rc::new(script_group.output_indices.clone());
        let mut syscalls: Vec<Box<(dyn Syscalls<CoreMachine>)>> = vec![
            Box::new(self.build_load_script_hash(current_script_hash.clone())),
            Box::new(self.build_load_tx()),
            Box::new(self.build_load_cell(
                Rc::clone(&script_group_input_indices),
                Rc::clone(&script_group_output_indices),
            )),
            Box::new(self.build_load_input(Rc::clone(&script_group_input_indices))),
            Box::new(self.build_load_header(Rc::clone(&script_group_input_indices))),
            Box::new(self.build_load_witness(
                Rc::clone(&script_group_input_indices),
                Rc::clone(&script_group_output_indices),
            )),
            Box::new(self.build_load_script(script_group.script.clone())),
            Box::new(self.build_load_cell_data(
                Rc::clone(&script_group_input_indices),
                Rc::clone(&script_group_output_indices),
            )),
            Box::new(Debugger::new(
                current_script_hash,
                Rc::clone(&self.debug_printer),
            )),
        ];
        #[cfg(test)]
        syscalls.push(Box::new(Pause::new(Rc::clone(&self.skip_pause))));
        if script_version >= ScriptVersion::V1 {
            syscalls.append(&mut vec![
                Box::new(self.build_vm_version()),
                Box::new(self.build_current_cycles()),
                Box::new(self.build_exec(
                    Rc::clone(&script_group_input_indices),
                    Rc::clone(&script_group_output_indices),
                )),
            ])
        }
        syscalls
    }

    fn build_machine(
        &self,
        script_group: &ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Machine, ScriptError> {
        let script_version = self.select_version(&script_group.script)?;
        let core_machine = script_version.init_core_machine(max_cycles);
        let machine_builder = DefaultMachineBuilder::<CoreMachine>::new(core_machine)
            .instruction_cycle_func(self.cost_model());
        let machine_builder = self
            .generate_syscalls(script_version, script_group)
            .into_iter()
            .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
        let default_machine = machine_builder.build();

        #[cfg(has_asm)]
        let machine = AsmMachine::new(default_machine, None);
        #[cfg(not(has_asm))]
        let machine = TraceMachine::new(default_machine);

        Ok(machine)
    }

    fn run(&self, script_group: &ScriptGroup, max_cycles: Cycle) -> Result<Cycle, ScriptError> {
        let program = self.extract_script(&script_group.script)?;
        let mut machine = self.build_machine(script_group, max_cycles)?;

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{:?}", error)),
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
        snap: &Option<(Snapshot, Cycle)>,
    ) -> Result<ChunkState, ScriptError> {
        let mut machine = self.build_machine(script_group, max_cycles)?;

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::CyclesExceeded => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{:?}", error)),
        };

        // we should not capture snapshot if load program failed by exceeded cycles
        if let Some((sp, current_cycle)) = snap {
            resume(&mut machine.machine, sp).map_err(map_vm_internal_error)?;
            machine.machine.set_cycles(*current_cycle)
        } else {
            let program = self.extract_script(&script_group.script)?;
            let bytes = machine
                .load_program(&program, &[])
                .map_err(map_vm_internal_error)?;
            let program_bytes_cycles = transferred_byte_cycles(bytes);
            let load_ret = machine.machine.add_cycles(program_bytes_cycles);
            if matches!(load_ret, Err(ref error) if error == &VMInternalError::CyclesExceeded) {
                return Ok(ChunkState::suspended(ResumableMachine::new(
                    machine,
                    Some(program_bytes_cycles),
                )));
            }
            load_ret.map_err(|e| ScriptError::VMInternalError(format!("{:?}", e)))?;
        }

        match machine.run() {
            Ok(code) => {
                if code == 0 {
                    Ok(ChunkState::Completed(machine.machine.cycles()))
                } else {
                    Err(ScriptError::validation_failure(&script_group.script, code))
                }
            }
            Err(error) => match error {
                VMInternalError::CyclesExceeded => {
                    Ok(ChunkState::suspended(ResumableMachine::new(machine, None)))
                }
                _ => Err(ScriptError::VMInternalError(format!("{:?}", error))),
            },
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

fn wrapping_cycles_sub(
    lhs: Cycle,
    rhs: Cycle,
    group: &ScriptGroup,
) -> Result<Cycle, TransactionScriptError> {
    lhs.checked_sub(rhs)
        .ok_or_else(|| ScriptError::CyclesOverflow(lhs, rhs).source(group))
}
