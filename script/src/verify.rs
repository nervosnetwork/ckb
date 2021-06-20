use crate::{
    cost_model::{instruction_cycles, transferred_byte_cycles},
    error::{ScriptError, TransactionScriptError},
    syscalls::{
        CurrentCycles, Debugger, Exec, LoadCell, LoadCellData, LoadHeader, LoadInput, LoadScript,
        LoadScriptHash, LoadTx, LoadWitness, VMVersion,
    },
    type_id::TypeIdSystemScript,
    types::{
        set_vm_max_cycles, CoreMachineType, Machine, ScriptGroup, ScriptGroupType,
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

#[cfg(has_asm)]
use ckb_vm::machine::asm::{AsmCoreMachine, AsmMachine};
use ckb_vm::snapshot::{resume, Snapshot};
use ckb_vm::{
    machine::{VERSION0, VERSION1},
    DefaultMachineBuilder, Error as VMInternalError, InstructionCycleFunc, SupportMachine,
    Syscalls, ISA_B, ISA_IMC, ISA_MOP,
};
#[cfg(not(has_asm))]
use ckb_vm::{DefaultCoreMachine, SparseMemory, TraceMachine, WXorXMemory};

use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::TryFrom;

pub enum ChunkState<'a> {
    VM(Machine<'a>),
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
    Unique((Byte32, LazyData)),
    Duplicate((Byte32, LazyData)),
    Multiple,
}

impl Binaries {
    fn new(data_hash: Byte32, data: LazyData) -> Self {
        Self::Unique((data_hash, data))
    }

    fn merge(&mut self, data_hash: &Byte32) {
        match self {
            Self::Unique(ref old) | Self::Duplicate(ref old) => {
                if old.0 != *data_hash {
                    *self = Self::Multiple;
                } else {
                    *self = Self::Duplicate(old.to_owned());
                }
            }
            Self::Multiple => {
                *self = Self::Multiple;
            }
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
            ScriptHashType::Data(_) => {
                if let Some(lazy) = self.binaries_by_data_hash.get(&script.code_hash()) {
                    Ok(lazy.access(self.data_loader))
                } else {
                    Err(ScriptError::InvalidCodeHash)
                }
            }
            ScriptHashType::Type => {
                if let Some(ref bin) = self.binaries_by_type_hash.get(&script.code_hash()) {
                    match bin {
                        Binaries::Unique((_, ref lazy)) => Ok(lazy.access(self.data_loader)),
                        Binaries::Duplicate((_, ref lazy)) => {
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

    /// Select the ISA and the version number of the new machine.
    pub fn select_machine_options(&self, script: &'a Script) -> Result<(u8, u32), ScriptError> {
        let proposal_window = self.consensus.tx_proposal_window();
        let epoch_number = self.tx_env.epoch_number(proposal_window);
        let hardfork_switch = self.consensus.hardfork_switch();
        let is_vm_version_1_and_syscalls_2_enabled =
            hardfork_switch.is_vm_version_1_and_syscalls_2_enabled(epoch_number);
        let script_hash_type = ScriptHashType::try_from(script.hash_type())
            .map_err(|err| ScriptError::InvalidScriptHashType(err.to_string()))?;
        match script_hash_type {
            ScriptHashType::Data(version) => {
                if !is_vm_version_1_and_syscalls_2_enabled && version > 0 {
                    Err(ScriptError::InvalidVmVersion(version))
                } else {
                    match version {
                        0 => Ok((ISA_IMC, VERSION0)),
                        1 => Ok((ISA_IMC | ISA_B | ISA_MOP, VERSION1)),
                        _ => Err(ScriptError::InvalidVmVersion(version)),
                    }
                }
            }
            ScriptHashType::Type => {
                if is_vm_version_1_and_syscalls_2_enabled {
                    Ok((ISA_IMC | ISA_B | ISA_MOP, VERSION1))
                } else {
                    Ok((ISA_IMC, VERSION0))
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
        for (_ty, hash, group) in self.groups() {
            // max_cycles must reduce by each group exec
            let cycle = self
                .verify_script_group(group, max_cycles - cycles)
                .map_err(|e| {
                    #[cfg(feature = "logging")]
                    info!(
                        "Error validating script group {} of transaction {}: {}",
                        hash,
                        self.hash(),
                        e
                    );
                    e.source(group)
                })?;
            cycles = cycles
                .checked_add(cycle)
                .ok_or_else(|| ScriptError::ExceededMaximumCycles(max_cycles).source(group))?;
        }
        Ok(cycles)
    }

    fn build_state(
        &self,
        vm: Machine<'a>,
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
            let step_cycle = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(&group, step_cycle, None) {
                Ok(ChunkState::Completed(cycle)) => {
                    cycles = wrapping_cycles_add(cycles, cycle, limit_cycles, group)?;
                }
                Ok(ChunkState::VM(vm)) => {
                    cycles = wrapping_cycles_add(cycles, vm.machine.cycles(), limit_cycles, group)?;

                    let current = (*ty, (*hash).to_owned());

                    let remain = groups
                        .iter()
                        .skip(idx)
                        .map(|(ty, hash, _g)| (*ty, (*hash).to_owned()))
                        .collect();

                    let state = self.build_state(vm, current, remain, cycles, limit_cycles);
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

        let current_group = self
            .find_script_group(snap.current.0, &snap.current.1)
            .ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {:?}", snap.current))
                    .unknown_source()
            })?;

        if limit_cycles < cycles {
            return Err(ScriptError::ExceededMaximumCycles(limit_cycles)
                .source(current_group)
                .into());
        }

        // continue snapshot current script
        // limit_cycles - cycles checked
        match self.verify_group_with_chunk(&current_group, limit_cycles - cycles, Some(&snap.snap))
        {
            Ok(ChunkState::Completed(cycle)) => {
                cycles = wrapping_cycles_add(cycles, cycle, limit_cycles, &current_group)?;
            }
            Ok(ChunkState::VM(vm)) => {
                cycles =
                    wrapping_cycles_add(cycles, vm.machine.cycles(), limit_cycles, &current_group)?;
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

            let step_cycle = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, step_cycle, None) {
                Ok(ChunkState::Completed(cycle)) => {
                    cycles = wrapping_cycles_add(cycles, cycle, limit_cycles, &current_group)?;
                }
                Ok(ChunkState::VM(vm)) => {
                    cycles = wrapping_cycles_add(
                        cycles,
                        vm.machine.cycles(),
                        limit_cycles,
                        &current_group,
                    )?;

                    let current = (*ty, hash.to_owned());
                    let remain = snap
                        .remain
                        .iter()
                        .skip(idx)
                        .map(|(ty, hash)| (*ty, hash.to_owned()))
                        .collect();

                    let state = self.build_state(vm, current, remain, cycles, limit_cycles);
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

        let mut cycles = current_cycles;

        if limit_cycles < current_cycles {
            return Err(ScriptError::ExceededMaximumCycles(limit_cycles)
                .unknown_source()
                .into());
        }

        let current_group = self
            .find_script_group(current.0, &current.1)
            .ok_or_else(|| {
                ScriptError::VMInternalError(format!("snapshot group missing {:?}", current))
                    .unknown_source()
            })?;

        set_vm_max_cycles(&mut vm, limit_cycles - cycles);
        vm.machine.set_cycles(0);
        match vm.run() {
            Ok(code) => {
                if code == 0 {
                    cycles = wrapping_cycles_add(
                        cycles,
                        vm.machine.cycles(),
                        limit_cycles,
                        &current_group,
                    )?;
                } else {
                    return Err(ScriptError::ValidationFailure(code)
                        .source(&current_group)
                        .into());
                }
            }
            Err(error) => match error {
                VMInternalError::InvalidCycles => {
                    cycles = wrapping_cycles_add(
                        cycles,
                        vm.machine.cycles(),
                        limit_cycles,
                        &current_group,
                    )?;

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

            let step_cycle = limit_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    limit_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, step_cycle, None) {
                Ok(ChunkState::Completed(cycle)) => {
                    cycles = wrapping_cycles_add(cycles, cycle, limit_cycles, &current_group)?;
                }
                Ok(ChunkState::VM(vm)) => {
                    cycles = wrapping_cycles_add(
                        cycles,
                        vm.machine.cycles(),
                        limit_cycles,
                        &current_group,
                    )?;

                    let current = (*ty, hash.to_owned());
                    let remain = remain
                        .iter()
                        .skip(idx)
                        .map(|(ty, hash)| (*ty, hash.to_owned()))
                        .collect();

                    let state = self.build_state(vm, current, remain, cycles, limit_cycles);
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

        if max_cycles < snap.current_cycles {
            return Err(ScriptError::ExceededMaximumCycles(max_cycles)
                .source(current_group)
                .into());
        }

        // continue snapshot current script
        // max_cycles - cycles checked
        match self.verify_group_with_chunk(&current_group, max_cycles - cycles, Some(&snap.snap)) {
            Ok(ChunkState::Completed(cycle)) => {
                cycles = wrapping_cycles_add(cycles, cycle, max_cycles, &current_group)?;
            }
            Ok(ChunkState::VM(_)) => {
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

            let step_cycle = max_cycles.checked_sub(cycles).ok_or_else(|| {
                ScriptError::VMInternalError(format!(
                    "expect invalid cycles {} {}",
                    max_cycles, cycles
                ))
                .source(group)
            })?;

            match self.verify_group_with_chunk(group, step_cycle, None) {
                Ok(ChunkState::Completed(cycle)) => {
                    cycles = wrapping_cycles_add(cycles, cycle, max_cycles, &current_group)?;
                }
                Ok(ChunkState::VM(_)) => {
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

    /// Returns all script groups.
    pub(crate) fn verify_group_with_chunk(
        &'a self,
        group: &'a ScriptGroup,
        max_cycles: Cycle,
        snap: Option<&Snapshot>,
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
        version: u32,
        script_group: &'a ScriptGroup,
    ) -> Vec<Box<(dyn Syscalls<CoreMachineType> + 'a)>> {
        let current_script_hash = script_group.script.calc_script_hash();
        let mut syscalls: Vec<Box<(dyn Syscalls<CoreMachineType> + 'a)>> = vec![
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
        if version >= VERSION1 {
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
        let (isa, version) = self.select_machine_options(&script_group.script)?;
        #[cfg(has_asm)]
        let core_machine = AsmCoreMachine::new(isa, version, max_cycles);
        #[cfg(not(has_asm))]
        let core_machine = CoreMachineType::new(isa, version, max_cycles);
        let machine_builder = DefaultMachineBuilder::<CoreMachineType>::new(core_machine)
            .instruction_cycle_func(self.cost_model());
        let machine_builder = self
            .generate_syscalls(version, script_group)
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
            .add_cycles(transferred_byte_cycles(bytes))
            .map_err(map_vm_internal_error)?;
        let code = machine.run().map_err(map_vm_internal_error)?;
        if code == 0 {
            Ok(machine.machine.cycles())
        } else {
            Err(ScriptError::ValidationFailure(code))
        }
    }

    fn chunk_run(
        &'a self,
        script_group: &'a ScriptGroup,
        max_cycles: Cycle,
        snap: Option<&Snapshot>,
    ) -> Result<ChunkState<'a>, ScriptError> {
        let mut machine = self.build_machine(script_group, max_cycles)?;

        let map_vm_internal_error = |error: VMInternalError| match error {
            VMInternalError::InvalidCycles => ScriptError::ExceededMaximumCycles(max_cycles),
            _ => ScriptError::VMInternalError(format!("{:?}", error)),
        };

        if let Some(sp) = snap {
            resume(&mut machine.machine, sp).map_err(map_vm_internal_error)?;
        } else {
            let program = self.extract_script(&script_group.script)?;
            let bytes = machine
                .load_program(&program, &[])
                .map_err(map_vm_internal_error)?;
            machine
                .machine
                .add_cycles(transferred_byte_cycles(bytes))
                .map_err(map_vm_internal_error)?;
        }
        match machine.run() {
            Ok(code) => {
                if code == 0 {
                    Ok(ChunkState::Completed(machine.machine.cycles()))
                } else {
                    Err(ScriptError::ValidationFailure(code))
                }
            }
            Err(error) => match error {
                VMInternalError::InvalidCycles => Ok(ChunkState::VM(machine)),
                _ => Err(ScriptError::VMInternalError(format!("{:?}", error))),
            },
        }
    }
}

fn wrapping_cycles_add(
    lhs: Cycle,
    rhs: Cycle,
    limit_cycles: Cycle,
    group: &ScriptGroup,
) -> Result<Cycle, TransactionScriptError> {
    lhs.checked_add(rhs)
        .ok_or_else(|| ScriptError::ExceededMaximumCycles(limit_cycles).source(group))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_id::TYPE_ID_CYCLES;
    use byteorder::{ByteOrder, LittleEndian};
    use ckb_crypto::secp::{Generator, Privkey, Pubkey, Signature};
    use ckb_db::RocksDB;
    use ckb_db_schema::COLUMNS;
    use ckb_hash::{blake2b_256, new_blake2b};
    use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB};
    use ckb_types::{
        core::{
            capacity_bytes, cell::CellMetaBuilder, hardfork::HardForkSwitch, Capacity, Cycle,
            DepType, EpochNumberWithFraction, HeaderView, ScriptHashType, TransactionBuilder,
            TransactionInfo,
        },
        h256,
        packed::{
            Byte32, CellDep, CellInput, CellOutputBuilder, OutPoint, Script,
            TransactionInfoBuilder, TransactionKeyBuilder, WitnessArgs,
        },
        H256,
    };
    use faster_hex::hex_encode;

    use ckb_chain_spec::consensus::{
        ConsensusBuilder, TWO_IN_TWO_OUT_BYTES, TWO_IN_TWO_OUT_CYCLES,
    };
    use ckb_error::assert_error_eq;
    use ckb_test_chain_utils::{
        always_success_cell, ckb_testnet_consensus, secp256k1_blake160_sighash_cell,
        secp256k1_data_cell, type_lock_script_code_hash,
    };
    use std::fs::File;
    use std::io::Read;
    use std::path::Path;

    const ALWAYS_SUCCESS_SCRIPT_CYCLE: u64 = 537;
    const CYCLE_BOUND: Cycle = 200_000;

    fn sha3_256<T: AsRef<[u8]>>(s: T) -> [u8; 32] {
        use tiny_keccak::{Hasher, Sha3};
        let mut output = [0; 32];
        let mut sha3 = Sha3::v256();
        sha3.update(s.as_ref());
        sha3.finalize(&mut output);
        output
    }

    // NOTE: `verify` binary is outdated and most related unit tests are testing `script` crate functions
    // I try to keep unit test code unmodified as much as possible, and may add it back in future PR.
    // fn open_cell_verify() -> File {
    //     File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/verify")).unwrap()
    // }

    fn open_cell_always_success() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/always_success"))
            .unwrap()
    }

    fn open_cell_always_failure() -> File {
        File::open(Path::new(env!("CARGO_MANIFEST_DIR")).join("../script/testdata/always_failure"))
            .unwrap()
    }

    fn new_store() -> ChainDB {
        ChainDB::new(RocksDB::open_tmp(COLUMNS), Default::default())
    }

    fn random_keypair() -> (Privkey, Pubkey) {
        Generator::random_keypair()
    }

    fn to_hex_pubkey(pubkey: &Pubkey) -> Vec<u8> {
        let pubkey = pubkey.serialize();
        let mut hex_pubkey = vec![0; pubkey.len() * 2];
        hex_encode(&pubkey, &mut hex_pubkey).expect("hex pubkey");
        hex_pubkey
    }

    fn to_hex_signature(signature: &Signature) -> Vec<u8> {
        let signature_der = signature.serialize_der();
        let mut hex_signature = vec![0; signature_der.len() * 2];
        hex_encode(&signature_der, &mut hex_signature).expect("hex signature");
        hex_signature
    }

    fn sign_args(args: &[u8], privkey: &Privkey) -> Signature {
        let hash = sha3_256(sha3_256(args));
        privkey.sign_recoverable(&hash.into()).unwrap()
    }

    fn default_transaction_info() -> TransactionInfo {
        TransactionInfoBuilder::default()
            .block_number(1u64.pack())
            .block_epoch(0u64.pack())
            .key(
                TransactionKeyBuilder::default()
                    .block_hash(Byte32::zero())
                    .index(1u32.pack())
                    .build(),
            )
            .build()
            .unpack()
    }

    #[test]
    fn check_always_success_hash() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input).build();

        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![always_success_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);
        assert!(verifier.verify(600).is_ok());
    }

    #[test]
    fn check_signature() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let code_hash = blake2b_256(&buffer);
        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data(0).into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert!(verifier.verify(100_000_000).is_ok());

        // Not enough cycles
        assert_error_eq!(
            verifier
                .verify(ALWAYS_SUCCESS_SCRIPT_CYCLE - 1)
                .unwrap_err(),
            ScriptError::ExceededMaximumCycles(ALWAYS_SUCCESS_SCRIPT_CYCLE - 1)
                .input_lock_script(0),
        );

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_referenced_via_type_hash() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .type_(
                Some(
                    Script::new_builder()
                        .code_hash(h256!("0x123456abcd90").pack())
                        .hash_type(ScriptHashType::Data(0).into())
                        .build(),
                )
                .pack(),
            )
            .build();
        let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_signature_referenced_via_type_hash_failure_with_multiple_matches() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();
        let data = Bytes::from(buffer);

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .type_(
                Some(
                    Script::new_builder()
                        .code_hash(h256!("0x123456abcd90").pack())
                        .hash_type(ScriptHashType::Data(0).into())
                        .build(),
                )
                .pack(),
            )
            .build();
        let type_hash = output.type_().to_opt().as_ref().unwrap().calc_script_hash();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data.clone())
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
            .build();

        let dep_out_point2 = OutPoint::new(h256!("0x1234").pack(), 8);
        let cell_dep2 = CellDep::new_builder()
            .out_point(dep_out_point2.clone())
            .build();
        let output2 = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .type_(
                Some(
                    Script::new_builder()
                        .code_hash(h256!("0x123456abcd90").pack())
                        .hash_type(ScriptHashType::Data(0).into())
                        .build(),
                )
                .pack(),
            )
            .build();
        let dep_cell2 = CellMetaBuilder::from_cell_output(output2, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point2)
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(type_hash)
            .hash_type(ScriptHashType::Type.into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
            .cell_dep(cell_dep)
            .cell_dep(cell_dep2)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell, dep_cell2],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::MultipleMatches.input_lock_script(0),
        );
    }

    #[test]
    fn check_invalid_signature() {
        let mut file = open_cell_always_failure();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);

        // This line makes the verification invalid
        args.extend(&b"extrastring".to_vec());
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let code_hash = blake2b_256(&buffer);
        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(code_hash.pack())
            .hash_type(ScriptHashType::Data(0).into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::ValidationFailure(-1).input_lock_script(0),
        );
    }

    #[test]
    fn check_invalid_dep_reference() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();
        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data(0).into())
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
            .cell_dep(cell_dep)
            .build();

        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script)
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::InvalidCodeHash.input_lock_script(0),
        );
    }

    #[test]
    fn check_output_contract() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();
        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data(0).into())
            .build();
        let output_data = Bytes::default();
        let output = CellOutputBuilder::default()
            .lock(
                Script::new_builder()
                    .hash_type(ScriptHashType::Data(0).into())
                    .build(),
            )
            .type_(Some(script).pack())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let dep_cell = {
            let data = Bytes::from(buffer);
            let output = CellOutputBuilder::default()
                .capacity(Capacity::bytes(data.len()).unwrap().pack())
                .build();
            CellMetaBuilder::from_cell_output(output, data)
                .transaction_info(default_transaction_info())
                .out_point(dep_out_point)
                .build()
        };

        let transaction = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(output_data.pack())
            .cell_dep(cell_dep)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert!(verifier.verify(100_000_000).is_ok());
    }

    #[test]
    fn check_invalid_output_contract() {
        let mut file = open_cell_always_failure();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let (privkey, pubkey) = random_keypair();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        // This line makes the verification invalid
        args.extend(&b"extrastring".to_vec());
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let input = CellInput::new(OutPoint::null(), 0);
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(always_success_script.clone())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.to_owned(),
            always_success_cell_data.to_owned(),
        )
        .transaction_info(default_transaction_info())
        .build();

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data(0).into())
            .build();
        let output = CellOutputBuilder::default()
            .type_(Some(script).pack())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder().out_point(dep_out_point).build();
        let dep_cell = {
            let dep_cell_data = Bytes::from(buffer);
            let output = CellOutputBuilder::default()
                .capacity(Capacity::bytes(dep_cell_data.len()).unwrap().pack())
                .build();
            CellMetaBuilder::from_cell_output(output, dep_cell_data)
                .transaction_info(default_transaction_info())
                .build()
        };

        let transaction = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(Bytes::new().pack())
            .cell_dep(cell_dep)
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell, always_success_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(100_000_000).unwrap_err(),
            ScriptError::ValidationFailure(-1).output_type_script(0),
        );
    }

    #[test]
    fn check_same_lock_and_type_script_are_executed_twice() {
        let mut file = open_cell_always_success();
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer).unwrap();

        let privkey = Privkey::from_slice(&[1; 32][..]);
        let pubkey = privkey.pubkey().unwrap();
        let mut args = b"foobar".to_vec();

        let signature = sign_args(&args, &privkey);
        args.extend(&to_hex_pubkey(&pubkey));
        args.extend(&to_hex_signature(&signature));

        let script = Script::new_builder()
            .args(Bytes::from(args).pack())
            .code_hash(blake2b_256(&buffer).pack())
            .hash_type(ScriptHashType::Data(0).into())
            .build();

        let dep_out_point = OutPoint::new(h256!("0x123").pack(), 8);
        let cell_dep = CellDep::new_builder()
            .out_point(dep_out_point.clone())
            .build();
        let data = Bytes::from(buffer);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(data.len()).unwrap().pack())
            .build();
        let dep_cell = CellMetaBuilder::from_cell_output(output, data)
            .transaction_info(default_transaction_info())
            .out_point(dep_out_point)
            .build();

        let transaction = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::null(), 0))
            .cell_dep(cell_dep)
            .build();

        // The lock and type scripts here are both executed.
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(script.clone())
            .type_(Some(script).pack())
            .build();
        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![dep_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        // Cycles can tell that both lock and type scripts are executed
        assert_eq!(
            verifier.verify(100_000_000).ok(),
            Some(ALWAYS_SUCCESS_SCRIPT_CYCLE * 2)
        );
    }

    #[test]
    fn check_type_id_one_in_one_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        if let Err(err) = verifier.verify(TYPE_ID_CYCLES * 2) {
            panic!("expect verification ok, got: {:?}", err);
        }
    }

    #[test]
    fn check_type_id_one_in_one_out_not_enough_cycles() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        // two groups need exec, so cycles not TYPE_ID_CYCLES - 1
        assert_error_eq!(
            verifier.verify(TYPE_ID_CYCLES - 1).unwrap_err(),
            ScriptError::ExceededMaximumCycles(TYPE_ID_CYCLES - ALWAYS_SUCCESS_SCRIPT_CYCLE - 1)
                .input_type_script(0),
        );
    }

    #[test]
    fn check_type_id_creation() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(input.as_slice());
            blake2b.update(&0u64.to_le_bytes());
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            Bytes::from(ret.to_vec())
        };

        let type_id_script = Script::new_builder()
            .args(input_hash.pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_termination() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert!(verifier.verify(1_001_000).is_ok());
    }

    #[test]
    fn check_type_id_invalid_creation() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(&input.previous_output().tx_hash().as_bytes());
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output().index().unpack());
            blake2b.update(&buf[..]);
            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, 0);
            blake2b.update(&buf[..]);
            blake2b.update(b"unnecessary data");
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);
            Bytes::from(ret.to_vec())
        };

        let type_id_script = Script::new_builder()
            .args(input_hash.pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-3).output_type_script(0),
        );
    }

    #[test]
    fn check_type_id_invalid_creation_length() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .build();

        let input_hash = {
            let mut blake2b = new_blake2b();
            blake2b.update(&input.previous_output().tx_hash().as_bytes());
            let mut buf = [0; 4];
            LittleEndian::write_u32(&mut buf, input.previous_output().index().unpack());
            blake2b.update(&buf[..]);
            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, 0);
            blake2b.update(&buf[..]);
            let mut ret = [0; 32];
            blake2b.finalize(&mut ret);

            let mut buf = vec![];
            buf.extend_from_slice(&ret[..]);
            buf.extend_from_slice(b"abc");
            Bytes::from(buf)
        };

        let type_id_script = Script::new_builder()
            .args(input_hash.pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(1_001_000).unwrap_err(),
            ScriptError::ValidationFailure(-1).output_type_script(0),
        );
    }

    #[test]
    fn check_type_id_one_in_two_out() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(2000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();
        let output_cell2 = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .output(output_cell2)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        assert_error_eq!(
            verifier.verify(TYPE_ID_CYCLES * 2).unwrap_err(),
            ScriptError::ValidationFailure(-2).input_type_script(0),
        );
    }

    #[test]
    fn check_typical_secp256k1_blake160_2_in_2_out_tx() {
        let consensus = ckb_testnet_consensus();
        let dep_group_tx_hash = consensus.genesis_block().transactions()[1].hash();
        let secp_out_point = OutPoint::new(dep_group_tx_hash, 0);

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.into())
            .build();

        let input1 = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 0), 0);
        let input2 = CellInput::new(OutPoint::new(h256!("0x1111").pack(), 0), 0);

        let mut generator = Generator::non_crypto_safe_prng(42);
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from((&blake2b_256(&pubkey_data)[0..20]).to_owned());
        let privkey2 = generator.gen_privkey();
        let pubkey_data2 = privkey2.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg2 = Bytes::from((&blake2b_256(&pubkey_data2)[0..20]).to_owned());

        let lock = Script::new_builder()
            .args(lock_arg.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let lock2 = Script::new_builder()
            .args(lock_arg2.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock.clone())
            .build();
        let output2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock2.clone())
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input1.clone())
            .input(input2.clone())
            .output(output1)
            .output(output2)
            .output_data(Default::default())
            .output_data(Default::default())
            .build();

        let tx_hash: H256 = tx.hash().unpack();
        // sign input1
        let witness = {
            WitnessArgs::new_builder()
                .lock(Some(Bytes::from(vec![0u8; 65])).pack())
                .build()
        };
        let witness_len: u64 = witness.as_bytes().len() as u64;
        let mut hasher = new_blake2b();
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness_len.to_le_bytes());
        hasher.update(&witness.as_bytes());
        let message = {
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig = privkey.sign_recoverable(&message).expect("sign");
        let witness = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(sig.serialize())).pack())
            .build();
        // sign input2
        let witness2 = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build();
        let witness2_len: u64 = witness2.as_bytes().len() as u64;
        let mut hasher = new_blake2b();
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness2_len.to_le_bytes());
        hasher.update(&witness2.as_bytes());
        let message2 = {
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig2 = privkey2.sign_recoverable(&message2).expect("sign");
        let witness2 = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(sig2.serialize())).pack())
            .build();
        let tx = tx
            .as_advanced_builder()
            .witness(witness.as_bytes().pack())
            .witness(witness2.as_bytes().pack())
            .build();

        let serialized_size = tx.data().as_slice().len() as u64;

        assert_eq!(
            serialized_size, TWO_IN_TWO_OUT_BYTES,
            "2 in 2 out tx serialized size changed, PLEASE UPDATE consensus"
        );

        let (secp256k1_blake160_cell, secp256k1_blake160_cell_data) =
            secp256k1_blake160_sighash_cell(consensus.clone());

        let (secp256k1_data_cell, secp256k1_data_cell_data) = secp256k1_data_cell(consensus);

        let input_cell1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock)
            .build();

        let resolved_input_cell1 =
            CellMetaBuilder::from_cell_output(input_cell1, Default::default())
                .out_point(input1.previous_output())
                .build();

        let input_cell2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock2)
            .build();

        let resolved_input_cell2 =
            CellMetaBuilder::from_cell_output(input_cell2, Default::default())
                .out_point(input2.previous_output())
                .build();

        let resolved_secp256k1_blake160_cell = CellMetaBuilder::from_cell_output(
            secp256k1_blake160_cell,
            secp256k1_blake160_cell_data,
        )
        .build();

        let resolved_secp_data_cell =
            CellMetaBuilder::from_cell_output(secp256k1_data_cell, secp256k1_data_cell_data)
                .build();

        let rtx = ResolvedTransaction {
            transaction: tx,
            resolved_cell_deps: vec![resolved_secp256k1_blake160_cell, resolved_secp_data_cell],
            resolved_inputs: vec![resolved_input_cell1, resolved_input_cell2],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        let cycle = verifier.verify(TWO_IN_TWO_OUT_CYCLES).unwrap();
        assert!(cycle <= TWO_IN_TWO_OUT_CYCLES);
        assert!(cycle >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
    }

    #[test]
    fn check_vm_version() {
        let vm_version_cell_data = Bytes::from(
            std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/vm_version"))
                .unwrap(),
        );
        let vm_version_cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(vm_version_cell_data.len()).unwrap().pack())
            .build();
        let vm_version_script = Script::new_builder()
            .hash_type(ScriptHashType::Data(1).into())
            .code_hash(CellOutput::calc_data_hash(&vm_version_cell_data))
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(vm_version_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input).build();

        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let vm_version_cell =
            CellMetaBuilder::from_cell_output(vm_version_cell, vm_version_cell_data)
                .transaction_info(default_transaction_info())
                .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![vm_version_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let fork_at = 10;
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_pr_0237(fork_at)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let epoch = EpochNumberWithFraction::new(fork_at, 0, 1);
            let header = HeaderView::new_advanced_builder()
                .epoch(epoch.pack())
                .build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);
        assert!(verifier.verify(6000).is_ok());
    }

    #[test]
    fn check_exec_from_cell_data() {
        let exec_caller_cell_data = Bytes::from(
            std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_caller_from_cell_data"),
            )
            .unwrap(),
        );
        let exec_caller_cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
            .build();

        let exec_callee_cell_data = Bytes::from(
            std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_callee"))
                .unwrap(),
        );
        let exec_callee_cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(exec_callee_cell_data.len()).unwrap().pack())
            .build();

        let exec_caller_script = Script::new_builder()
            .hash_type(ScriptHashType::Data(1).into())
            .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(exec_caller_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default().input(input).build();

        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let exec_caller_cell =
            CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
                .transaction_info(default_transaction_info())
                .build();

        let exec_callee_cell =
            CellMetaBuilder::from_cell_output(exec_callee_cell, exec_callee_cell_data)
                .transaction_info(default_transaction_info())
                .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![exec_caller_cell, exec_callee_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let fork_at = 10;
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_pr_0237(fork_at)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let epoch = EpochNumberWithFraction::new(fork_at, 0, 1);
            let header = HeaderView::new_advanced_builder()
                .epoch(epoch.pack())
                .build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);
        assert!(verifier.verify(600000).is_ok());
    }

    #[test]
    fn check_exec_from_witness() {
        let exec_caller_cell_data = Bytes::from(
            std::fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_caller_from_witness"),
            )
            .unwrap(),
        );
        let exec_caller_cell = CellOutput::new_builder()
            .capacity(Capacity::bytes(exec_caller_cell_data.len()).unwrap().pack())
            .build();

        let exec_callee = Bytes::from(
            std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/exec_callee"))
                .unwrap(),
        )
        .pack();

        let exec_caller_script = Script::new_builder()
            .hash_type(ScriptHashType::Data(1).into())
            .code_hash(CellOutput::calc_data_hash(&exec_caller_cell_data))
            .build();
        let output = CellOutputBuilder::default()
            .capacity(capacity_bytes!(100).pack())
            .lock(exec_caller_script)
            .build();
        let input = CellInput::new(OutPoint::null(), 0);

        let transaction = TransactionBuilder::default()
            .input(input)
            .set_witnesses(vec![exec_callee])
            .build();

        let dummy_cell = CellMetaBuilder::from_cell_output(output, Bytes::new())
            .transaction_info(default_transaction_info())
            .build();
        let exec_caller_cell =
            CellMetaBuilder::from_cell_output(exec_caller_cell, exec_caller_cell_data)
                .transaction_info(default_transaction_info())
                .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![exec_caller_cell],
            resolved_inputs: vec![dummy_cell],
            resolved_dep_groups: vec![],
        };

        let fork_at = 10;
        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_pr_0237(fork_at)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let epoch = EpochNumberWithFraction::new(fork_at, 0, 1);
            let header = HeaderView::new_advanced_builder()
                .epoch(epoch.pack())
                .build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);
        assert!(verifier.verify(600000).is_ok());
    }

    #[test]
    fn check_type_id_one_in_one_out_chunk() {
        let (always_success_cell, always_success_cell_data, always_success_script) =
            always_success_cell();
        let always_success_out_point = OutPoint::new(h256!("0x11").pack(), 0);

        let type_id_script = Script::new_builder()
            .args(Bytes::from(h256!("0x1111").as_ref()).pack())
            .code_hash(TYPE_ID_CODE_HASH.pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let input = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 8), 0);
        let input_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(1000).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script.clone()).pack())
            .build();

        let output_cell = CellOutputBuilder::default()
            .capacity(capacity_bytes!(990).pack())
            .lock(always_success_script.clone())
            .type_(Some(type_id_script).pack())
            .build();

        let transaction = TransactionBuilder::default()
            .input(input.clone())
            .output(output_cell)
            .cell_dep(
                CellDep::new_builder()
                    .out_point(always_success_out_point.clone())
                    .build(),
            )
            .build();

        let resolved_input_cell = CellMetaBuilder::from_cell_output(input_cell, Bytes::new())
            .out_point(input.previous_output())
            .build();
        let resolved_always_success_cell = CellMetaBuilder::from_cell_output(
            always_success_cell.clone(),
            always_success_cell_data.to_owned(),
        )
        .out_point(always_success_out_point)
        .build();

        let rtx = ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![resolved_always_success_cell],
            resolved_inputs: vec![resolved_input_cell],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        let mut groups: Vec<_> = verifier.groups().collect();
        let mut cycles = 0;
        let mut tmp: Option<Machine<'_>> = None;

        loop {
            if let Some(mut vm) = tmp.take() {
                cycles += vm.machine.cycles();
                vm.machine.set_cycles(0);
                match vm.run() {
                    Ok(code) => {
                        if code == 0 {
                            cycles += vm.machine.cycles();
                        } else {
                            unreachable!()
                        }
                    }
                    Err(error) => match error {
                        VMInternalError::InvalidCycles => {
                            tmp = Some(vm);
                            continue;
                        }
                        _ => unreachable!(),
                    },
                }
            }
            while let Some((ty, _, group)) = groups.pop() {
                let max = match ty {
                    ScriptGroupType::Lock => ALWAYS_SUCCESS_SCRIPT_CYCLE - 10,
                    ScriptGroupType::Type => TYPE_ID_CYCLES,
                };
                match verifier.verify_group_with_chunk(&group, max, None).unwrap() {
                    ChunkState::Completed(cycle) => {
                        cycles += cycle;
                    }
                    ChunkState::VM(vm) => {
                        tmp = Some(vm);
                        break;
                    }
                }
            }

            if tmp.is_none() {
                break;
            }
        }

        assert_eq!(cycles, TYPE_ID_CYCLES + ALWAYS_SUCCESS_SCRIPT_CYCLE);
    }

    #[test]
    fn check_typical_secp256k1_blake160_2_in_2_out_tx_with_chunk() {
        let consensus = ckb_testnet_consensus();
        let dep_group_tx_hash = consensus.genesis_block().transactions()[1].hash();
        let secp_out_point = OutPoint::new(dep_group_tx_hash, 0);

        let cell_dep = CellDep::new_builder()
            .out_point(secp_out_point)
            .dep_type(DepType::DepGroup.into())
            .build();

        let input1 = CellInput::new(OutPoint::new(h256!("0x1234").pack(), 0), 0);
        let input2 = CellInput::new(OutPoint::new(h256!("0x1111").pack(), 0), 0);

        let mut generator = Generator::non_crypto_safe_prng(42);
        let privkey = generator.gen_privkey();
        let pubkey_data = privkey.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg = Bytes::from((&blake2b_256(&pubkey_data)[0..20]).to_owned());
        let privkey2 = generator.gen_privkey();
        let pubkey_data2 = privkey2.pubkey().expect("Get pubkey failed").serialize();
        let lock_arg2 = Bytes::from((&blake2b_256(&pubkey_data2)[0..20]).to_owned());

        let lock = Script::new_builder()
            .args(lock_arg.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let lock2 = Script::new_builder()
            .args(lock_arg2.pack())
            .code_hash(type_lock_script_code_hash().pack())
            .hash_type(ScriptHashType::Type.into())
            .build();

        let output1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock.clone())
            .build();
        let output2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock2.clone())
            .build();
        let tx = TransactionBuilder::default()
            .cell_dep(cell_dep)
            .input(input1.clone())
            .input(input2.clone())
            .output(output1)
            .output(output2)
            .output_data(Default::default())
            .output_data(Default::default())
            .build();

        let tx_hash: H256 = tx.hash().unpack();
        // sign input1
        let witness = {
            WitnessArgs::new_builder()
                .lock(Some(Bytes::from(vec![0u8; 65])).pack())
                .build()
        };
        let witness_len: u64 = witness.as_bytes().len() as u64;
        let mut hasher = new_blake2b();
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness_len.to_le_bytes());
        hasher.update(&witness.as_bytes());
        let message = {
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig = privkey.sign_recoverable(&message).expect("sign");
        let witness = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(sig.serialize())).pack())
            .build();
        // sign input2
        let witness2 = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(vec![0u8; 65])).pack())
            .build();
        let witness2_len: u64 = witness2.as_bytes().len() as u64;
        let mut hasher = new_blake2b();
        hasher.update(tx_hash.as_bytes());
        hasher.update(&witness2_len.to_le_bytes());
        hasher.update(&witness2.as_bytes());
        let message2 = {
            let mut buf = [0u8; 32];
            hasher.finalize(&mut buf);
            H256::from(buf)
        };
        let sig2 = privkey2.sign_recoverable(&message2).expect("sign");
        let witness2 = WitnessArgs::new_builder()
            .lock(Some(Bytes::from(sig2.serialize())).pack())
            .build();
        let tx = tx
            .as_advanced_builder()
            .witness(witness.as_bytes().pack())
            .witness(witness2.as_bytes().pack())
            .build();

        let serialized_size = tx.data().as_slice().len() as u64;

        assert_eq!(
            serialized_size, TWO_IN_TWO_OUT_BYTES,
            "2 in 2 out tx serialized size changed, PLEASE UPDATE consensus"
        );

        let (secp256k1_blake160_cell, secp256k1_blake160_cell_data) =
            secp256k1_blake160_sighash_cell(consensus.clone());

        let (secp256k1_data_cell, secp256k1_data_cell_data) = secp256k1_data_cell(consensus);

        let input_cell1 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock)
            .build();

        let resolved_input_cell1 =
            CellMetaBuilder::from_cell_output(input_cell1, Default::default())
                .out_point(input1.previous_output())
                .build();

        let input_cell2 = CellOutput::new_builder()
            .capacity(capacity_bytes!(100).pack())
            .lock(lock2)
            .build();

        let resolved_input_cell2 =
            CellMetaBuilder::from_cell_output(input_cell2, Default::default())
                .out_point(input2.previous_output())
                .build();

        let resolved_secp256k1_blake160_cell = CellMetaBuilder::from_cell_output(
            secp256k1_blake160_cell,
            secp256k1_blake160_cell_data,
        )
        .build();

        let resolved_secp_data_cell =
            CellMetaBuilder::from_cell_output(secp256k1_data_cell, secp256k1_data_cell_data)
                .build();

        let rtx = ResolvedTransaction {
            transaction: tx,
            resolved_cell_deps: vec![resolved_secp256k1_blake160_cell, resolved_secp_data_cell],
            resolved_inputs: vec![resolved_input_cell1, resolved_input_cell2],
            resolved_dep_groups: vec![],
        };

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let hardfork_switch = HardForkSwitch::new_without_any_enabled();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let tx_env = {
            let header = HeaderView::new_advanced_builder().build();
            TxVerifyEnv::new_commit(&header)
        };

        let verifier = TransactionScriptsVerifier::new(&rtx, &consensus, &data_loader, &tx_env);

        let mut groups: Vec<_> = verifier.groups().collect();
        let mut cycles = 0;
        let mut tmp: Option<Machine<'_>> = None;

        loop {
            if let Some(mut vm) = tmp.take() {
                cycles += vm.machine.cycles();
                vm.machine.set_cycles(0);
                match vm.run() {
                    Ok(code) => {
                        if code == 0 {
                            cycles += vm.machine.cycles();
                        } else {
                            unreachable!()
                        }
                    }
                    Err(error) => match error {
                        VMInternalError::InvalidCycles => {
                            tmp = Some(vm);
                            continue;
                        }
                        _ => unreachable!(),
                    },
                }
            }
            while let Some((_, _, group)) = groups.pop() {
                match verifier
                    .verify_group_with_chunk(&group, TWO_IN_TWO_OUT_CYCLES / 10, None)
                    .unwrap()
                {
                    ChunkState::Completed(cycle) => {
                        cycles += cycle;
                    }
                    ChunkState::VM(vm) => {
                        tmp = Some(vm);
                        break;
                    }
                }
            }

            if tmp.is_none() {
                break;
            }
        }

        let cycles_once = verifier.verify(TWO_IN_TWO_OUT_CYCLES).unwrap();

        assert!(cycles <= TWO_IN_TWO_OUT_CYCLES);
        assert!(cycles >= TWO_IN_TWO_OUT_CYCLES - CYCLE_BOUND);
        assert_eq!(cycles, cycles_once);
    }
}
