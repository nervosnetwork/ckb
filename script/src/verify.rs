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
    verify_env::TxVerifyEnv,
};
use ckb_chain_spec::consensus::{Consensus, TYPE_ID_CODE_HASH};
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
    packed::{Byte32, Byte32Vec, BytesVec, CellInputVec, CellOutput, OutPoint, Script},
    prelude::*,
};

use ckb_vm::{
    snapshot::{resume, Snapshot},
    DefaultMachineBuilder, Error as VMInternalError, InstructionCycleFunc, SupportMachine,
    Syscalls,
};

#[cfg(has_asm)]
use ckb_vm::machine::asm::AsmMachine;

#[cfg(not(has_asm))]
use ckb_vm::TraceMachine;

use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::TryFrom;

#[cfg(test)]
mod tests;

pub enum ChunkState<'a> {
    Suspended(ResumableMachine<'a>),
    Completed(Cycle),
}

#[derive(Debug, PartialEq, Eq, Clone)]
enum DataGurad {
    NotLoaded(OutPoint),
    Loaded(Bytes),
}

/// LazyData wrapper make sure not-loaded data will be loaded only after one access
#[derive(Debug, PartialEq, Eq, Clone)]
struct LazyData(RefCell<DataGurad>);

impl LazyData {
    fn from_cell_meta(cell_meta: &CellMeta) -> LazyData {
        match &cell_meta.mem_cell_data {
            Some(data) => LazyData(RefCell::new(DataGurad::Loaded(data.to_owned()))),
            None => LazyData(RefCell::new(DataGurad::NotLoaded(
                cell_meta.out_point.clone(),
            ))),
        }
    }

    fn access<DL: CellDataProvider>(&self, data_loader: &DL) -> Bytes {
        let guard = self.0.borrow().to_owned();
        match guard {
            DataGurad::NotLoaded(out_point) => {
                let data = data_loader.get_cell_data(&out_point).expect("cell data");
                self.0.replace(DataGurad::Loaded(data.to_owned()));
                data
            }
            DataGurad::Loaded(bytes) => bytes,
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
pub struct TransactionScriptsVerifier<'a, DL> {
    data_loader: &'a DL,
    consensus: &'a Consensus,
    tx_env: &'a TxVerifyEnv,

    debug_printer: Box<dyn Fn(&Byte32, &str)>,

    outputs: Vec<CellMeta>,
    rtx: &'a ResolvedTransaction,

    binaries_by_data_hash: HashMap<Byte32, LazyData>,
    binaries_by_type_hash: HashMap<Byte32, Binaries>,

    lock_groups: HashMap<Byte32, ScriptGroup>,
    type_groups: HashMap<Byte32, ScriptGroup>,
}

impl<'a, DL: CellDataProvider + HeaderProvider> TransactionScriptsVerifier<'a, DL> {
    /// Creates a script verifier for the transaction.
    ///
    /// ## Params
    ///
    /// * `rtx` - transaction which cell out points have been resolved.
    /// * `data_loader` - used to load cell data.
    pub fn new(
        rtx: &'a ResolvedTransaction,
        consensus: &'a Consensus,
        data_loader: &'a DL,
        tx_env: &'a TxVerifyEnv,
    ) -> TransactionScriptsVerifier<'a, DL> {
        let tx_hash = rtx.transaction.hash();
        let resolved_cell_deps = &rtx.resolved_cell_deps;
        let resolved_inputs = &rtx.resolved_inputs;
        let outputs = rtx
            .transaction
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
            .collect();

        let mut binaries_by_data_hash: HashMap<Byte32, LazyData> = HashMap::default();
        let mut binaries_by_type_hash: HashMap<Byte32, Binaries> = HashMap::default();
        for cell_meta in resolved_cell_deps {
            let data_hash = data_loader
                .load_cell_data_hash(cell_meta)
                .expect("cell data hash");
            let lazy = LazyData::from_cell_meta(&cell_meta);
            binaries_by_data_hash.insert(data_hash.to_owned(), lazy.to_owned());

            if let Some(t) = &cell_meta.cell_output.type_().to_opt() {
                binaries_by_type_hash
                    .entry(t.calc_script_hash())
                    .and_modify(|bin| bin.merge(&data_hash))
                    .or_insert_with(|| Binaries::new(data_hash.to_owned(), lazy.to_owned()));
            }
        }

        let mut lock_groups = HashMap::default();
        let mut type_groups = HashMap::default();
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
                    .or_insert_with(|| ScriptGroup::from_type_script(&t));
                type_group_entry.input_indices.push(i);
            }
        }
        for (i, output) in rtx.transaction.outputs().into_iter().enumerate() {
            if let Some(t) = &output.type_().to_opt() {
                let type_group_entry = type_groups
                    .entry(t.calc_script_hash())
                    .or_insert_with(|| ScriptGroup::from_type_script(&t));
                type_group_entry.output_indices.push(i);
            }
        }

        TransactionScriptsVerifier {
            data_loader,
            consensus,
            tx_env,
            binaries_by_data_hash,
            binaries_by_type_hash,
            outputs,
            rtx,
            lock_groups,
            type_groups,
            debug_printer: Box::new(
                #[allow(unused_variables)]
                |hash: &Byte32, message: &str| {
                    #[cfg(feature = "logging")]
                    debug!("script group: {} DEBUG OUTPUT: {}", hash, message);
                },
            ),
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
        self.debug_printer = Box::new(func);
    }

    #[inline]
    fn inputs(&self) -> CellInputVec {
        self.rtx.transaction.inputs()
    }

    #[inline]
    fn header_deps(&self) -> Byte32Vec {
        self.rtx.transaction.header_deps()
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_cell_deps
    }

    #[inline]
    fn witnesses(&self) -> BytesVec {
        self.rtx.transaction.witnesses()
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

    fn build_exec(&'a self, group_inputs: &'a [usize], group_outputs: &'a [usize]) -> Exec<'a, DL> {
        Exec::new(
            &self.data_loader,
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_cell_deps(),
            group_inputs,
            group_outputs,
            self.witnesses(),
        )
    }

    fn build_load_tx(&self) -> LoadTx {
        LoadTx::new(&self.rtx.transaction)
    }

    fn build_load_cell(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCell<'a, DL> {
        LoadCell::new(
            &self.data_loader,
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_cell_deps(),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_cell_data(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCellData<'a, DL> {
        LoadCellData::new(
            &self.data_loader,
            &self.outputs,
            self.resolved_inputs(),
            self.resolved_cell_deps(),
            group_inputs,
            group_outputs,
        )
    }

    fn build_load_input(&self, group_inputs: &'a [usize]) -> LoadInput {
        LoadInput::new(self.inputs(), group_inputs)
    }

    fn build_load_script_hash(&self, hash: Byte32) -> LoadScriptHash {
        LoadScriptHash::new(hash)
    }

    fn build_load_header(&'a self, group_inputs: &'a [usize]) -> LoadHeader<'a, DL> {
        LoadHeader::new(
            &self.data_loader,
            self.header_deps(),
            self.resolved_inputs(),
            self.resolved_cell_deps(),
            group_inputs,
        )
    }

    fn build_load_witness(
        &'a self,
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadWitness<'a> {
        LoadWitness::new(self.witnesses(), group_inputs, group_outputs)
    }

    fn build_load_script(&self, script: Script) -> LoadScript {
        LoadScript::new(script)
    }

    /// Extracts actual script binary either in dep cells.
    pub fn extract_script(&self, script: &'a Script) -> Result<Bytes, ScriptError> {
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data | ScriptHashType::Data1 => {
                if let Some(lazy) = self.binaries_by_data_hash.get(&script.code_hash()) {
                    Ok(lazy.access(self.data_loader))
                } else {
                    Err(ScriptError::InvalidCodeHash)
                }
            }
            ScriptHashType::Type => {
                if let Some(ref bin) = self.binaries_by_type_hash.get(&script.code_hash()) {
                    match bin {
                        Binaries::Unique(_, ref lazy) => Ok(lazy.access(self.data_loader)),
                        Binaries::Duplicate(_, ref lazy) => {
                            let proposal_window = self.consensus.tx_proposal_window();
                            let epoch_number = self.tx_env.epoch_number(proposal_window);
                            if self
                                .consensus
                                .hardfork_switch()
                                .is_allow_multiple_matches_on_identical_data_enabled(epoch_number)
                            {
                                Ok(lazy.access(self.data_loader))
                            } else {
                                Err(ScriptError::MultipleMatches)
                            }
                        }
                        Binaries::Multiple => Err(ScriptError::MultipleMatches),
                    }
                } else {
                    Err(ScriptError::InvalidCodeHash)
                }
            }
        }
    }

    /// Returns the version of the machine based on the script and the consensus rules.
    pub fn select_version(&self, script: &'a Script) -> Result<ScriptVersion, ScriptError> {
        // If the proposal window is allowed to prejudge on the vm version,
        // it will cause proposal tx to start a new vm in the blocks before hardfork,
        // destroying the assumption that the transaction execution only uses the old vm
        // before hardfork, leading to unexpected network splits.
        let epoch_number = self.tx_env.current_epoch_number();
        let hardfork_switch = self.consensus.hardfork_switch();
        let is_vm_version_1_and_syscalls_2_enabled =
            hardfork_switch.is_vm_version_1_and_syscalls_2_enabled(epoch_number);
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
            ScriptHashType::Type => {
                if is_vm_version_1_and_syscalls_2_enabled {
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
    /// * `max_cycles` - Maximium allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles on success, Otherwise it returns the verification error.
    pub fn verify(&self, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles: Cycle = 0;

        // Now run each script group
        for (_ty, _hash, group) in self.groups() {
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

            cycles = wrapping_cycles_add(cycles, used_cycles, &group)?;
        }
        Ok(cycles)
    }

    fn build_state(
        &self,
        vm: ResumableMachine<'a>,
        current: (ScriptGroupType, Byte32),
        remain: Vec<(ScriptGroupType, Byte32)>,
        current_cycles: Cycle,
        limit_cycles: Cycle,
    ) -> TransactionState<'a> {
        TransactionState {
            current,
            remain,
            vm,
            current_cycles,
            limit_cycles,
        }
    }

    /// Performing a resumable verification on the transaction scripts.
    ///
    /// ## Params
    ///
    /// * `limit_cycles` - Maximium allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a state will retruned.
    pub fn resumable_verify(&self, limit_cycles: Cycle) -> Result<VerifyResult, Error> {
        let mut cycles = 0;

        let groups: Vec<_> = self.groups().collect();
        for (idx, (ty, hash, group)) in groups.iter().enumerate() {
            // vm should early return invalid cycles
            let remain_cycles = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(&group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let current = (*ty, (*hash).to_owned());

                    let remain = groups
                        .iter()
                        .skip(idx + 1)
                        .map(|(ty, hash, _g)| (*ty, (*hash).to_owned()))
                        .collect();

                    let state = self.build_state(vm, current, remain, cycles, remain_cycles);
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
    /// * `limit_cycles` - Maximium allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a borrowed state will retruned.
    pub fn resume_from_snap(
        &self,
        snap: &TransactionSnapshot,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult, Error> {
        let mut cycles = snap.current_cycles;
        let mut current_used = 0;

        let current_group = self
            .find_script_group(snap.current.0, &snap.current.1)
            .ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {:?}", snap.current))
                    .unknown_source()
            })?;

        // continue snapshot current script
        match self.verify_group_with_chunk(&current_group, limit_cycles, &snap.snap) {
            Ok(ChunkState::Completed(used_cycles)) => {
                current_used = wrapping_cycles_add(current_used, used_cycles, &current_group)?;
                cycles = wrapping_cycles_add(cycles, used_cycles, &current_group)?;
            }
            Ok(ChunkState::Suspended(vm)) => {
                let current = snap.current.to_owned();
                let remain = snap.remain.to_owned();
                let state = self.build_state(vm, current, remain, cycles, limit_cycles);
                return Ok(VerifyResult::Suspended(state));
            }
            Err(e) => {
                return Err(e.source(&current_group).into());
            }
        }

        for (idx, (ty, hash)) in snap.remain.iter().enumerate() {
            let group = self.find_script_group(*ty, hash).ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {} {}", ty, hash))
                    .unknown_source()
            })?;

            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, &group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, &group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let current = (*ty, hash.to_owned());
                    let remain = snap
                        .remain
                        .iter()
                        .skip(idx + 1)
                        .map(|(ty, hash)| (*ty, hash.to_owned()))
                        .collect();

                    let state = self.build_state(vm, current, remain, cycles, remain_cycles);
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
    /// * `limit_cycles` - Maximium allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles if verification completed,
    /// If verify is suspended, a borrowed state will retruned.
    pub fn resume_from_state(
        &'a self,
        state: TransactionState<'a>,
        limit_cycles: Cycle,
    ) -> Result<VerifyResult<'a>, Error> {
        let TransactionState {
            current,
            remain,
            mut vm,
            current_cycles,
            ..
        } = state;

        let mut current_used = 0;
        let mut cycles = current_cycles;

        let current_group = self
            .find_script_group(current.0, &current.1)
            .ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {:?}", current))
                    .unknown_source()
            })?;

        vm.set_max_cycles(limit_cycles);
        match vm.machine.run() {
            Ok(code) => {
                if code == 0 {
                    current_used = wrapping_cycles_add(current_used, vm.cycles(), &current_group)?;
                    cycles = wrapping_cycles_add(cycles, vm.cycles(), &current_group)?;
                } else {
                    return Err(ScriptError::validation_failure(&current_group.script, code)
                        .source(&current_group)
                        .into());
                }
            }
            Err(error) => match error {
                VMInternalError::InvalidCycles => {
                    let state = self.build_state(vm, current, remain, cycles, limit_cycles);
                    return Ok(VerifyResult::Suspended(state));
                }
                error => {
                    return Err(ScriptError::VMInternalError(format!("{:?}", error))
                        .source(&current_group)
                        .into())
                }
            },
        }

        for (idx, (ty, hash)) in remain.iter().enumerate() {
            let group = self.find_script_group(*ty, hash).ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {} {}", ty, hash))
                    .unknown_source()
            })?;

            let remain_cycles = limit_cycles.checked_sub(current_used).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    current_used = wrapping_cycles_add(current_used, used_cycles, &group)?;
                    cycles = wrapping_cycles_add(cycles, used_cycles, &group)?;
                }
                Ok(ChunkState::Suspended(vm)) => {
                    let current = (*ty, hash.to_owned());
                    let remain = remain
                        .iter()
                        .skip(idx + 1)
                        .map(|(ty, hash)| (*ty, hash.to_owned()))
                        .collect();

                    let state = self.build_state(vm, current, remain, cycles, remain_cycles);
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
    /// * `max_cycles` - Maximium allowed cycles to run the scripts. The verification quits early
    /// when the consumed cycles exceed the limit.
    ///
    /// ## Returns
    ///
    /// It returns the total consumed cycles on completed, Otherwise it returns the verification error.
    pub fn complete(&self, snap: &TransactionSnapshot, max_cycles: Cycle) -> Result<Cycle, Error> {
        let mut cycles = snap.current_cycles;

        let current_group = self
            .find_script_group(snap.current.0, &snap.current.1)
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
        match self.verify_group_with_chunk(&current_group, max_cycles - cycles, &snap.snap) {
            Ok(ChunkState::Completed(used_cycles)) => {
                cycles = wrapping_cycles_add(cycles, used_cycles, &current_group)?;
            }
            Ok(ChunkState::Suspended(_)) => {
                return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                    .source(&current_group)
                    .into());
            }
            Err(e) => {
                return Err(e.source(&current_group).into());
            }
        }

        for (ty, hash) in &snap.remain {
            let group = self.find_script_group(*ty, hash).ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {} {}", ty, hash))
                    .unknown_source()
            })?;

            let remain_cycles = max_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    max_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, remain_cycles, &None) {
                Ok(ChunkState::Completed(used_cycles)) => {
                    cycles = wrapping_cycles_add(cycles, used_cycles, &current_group)?;
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
                rtx: self.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify()
        } else {
            self.run(&group, max_cycles)
        }
    }
    /// Returns all script groups.
    pub fn groups(&self) -> impl Iterator<Item = (ScriptGroupType, &'_ Byte32, &'_ ScriptGroup)> {
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
        &'a self,
        group: &'a ScriptGroup,
        max_cycles: Cycle,
        snap: &Option<Snapshot>,
    ) -> Result<ChunkState<'a>, ScriptError> {
        if group.script.code_hash() == TYPE_ID_CODE_HASH.pack()
            && Into::<u8>::into(group.script.hash_type()) == Into::<u8>::into(ScriptHashType::Type)
        {
            let verifier = TypeIdSystemScript {
                rtx: self.rtx,
                script_group: group,
                max_cycles,
            };
            verifier.verify().map(ChunkState::Completed)
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
        &'a self,
        script_version: ScriptVersion,
        script_group: &'a ScriptGroup,
    ) -> Vec<Box<(dyn Syscalls<CoreMachine> + 'a)>> {
        let current_script_hash = script_group.script.calc_script_hash();
        let mut syscalls: Vec<Box<(dyn Syscalls<CoreMachine> + 'a)>> = vec![
            Box::new(self.build_load_script_hash(current_script_hash.clone())),
            Box::new(self.build_load_tx()),
            Box::new(
                self.build_load_cell(&script_group.input_indices, &script_group.output_indices),
            ),
            Box::new(self.build_load_input(&script_group.input_indices)),
            Box::new(self.build_load_header(&script_group.input_indices)),
            Box::new(
                self.build_load_witness(&script_group.input_indices, &script_group.output_indices),
            ),
            Box::new(self.build_load_script(script_group.script.clone())),
            Box::new(
                self.build_load_cell_data(
                    &script_group.input_indices,
                    &script_group.output_indices,
                ),
            ),
            Box::new(Debugger::new(current_script_hash, &self.debug_printer)),
        ];
        if script_version >= ScriptVersion::V1 {
            syscalls.append(&mut vec![
                Box::new(self.build_vm_version()),
                Box::new(self.build_current_cycles()),
                Box::new(
                    self.build_exec(&script_group.input_indices, &script_group.output_indices),
                ),
            ])
        }
        syscalls
    }

    fn build_machine(
        &'a self,
        script_group: &'a ScriptGroup,
        max_cycles: Cycle,
    ) -> Result<Machine<'a>, ScriptError> {
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
            VMInternalError::InvalidCycles => ScriptError::ExceededMaximumCycles(max_cycles),
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
        &'a self,
        script_group: &'a ScriptGroup,
        max_cycles: Cycle,
        snap: &Option<Snapshot>,
    ) -> Result<ChunkState<'a>, ScriptError> {
        let mut machine = self.build_machine(script_group, max_cycles)?;

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::InvalidCycles => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{:?}", error)),
        };

        let program = self.extract_script(&script_group.script)?;
        let bytes = machine
            .load_program(&program, &[])
            .map_err(map_vm_internal_error)?;

        // we should not capture snapshot if load program failed by exceeded cycles
        if let Some(sp) = snap {
            resume(&mut machine.machine, sp).map_err(map_vm_internal_error)?;
        } else {
            let load_ret = machine.machine.add_cycles(transferred_byte_cycles(bytes));
            if matches!(load_ret, Err(error) if error == VMInternalError::InvalidCycles) {
                return Ok(ChunkState::Suspended(ResumableMachine::new(machine, false)));
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
                VMInternalError::InvalidCycles => {
                    Ok(ChunkState::Suspended(ResumableMachine::new(machine, true)))
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
