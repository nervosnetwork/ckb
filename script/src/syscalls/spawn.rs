use crate::cost_model::transferred_byte_cycles;
use crate::syscalls::utils::load_c_string;
use crate::syscalls::{
    Source, SourceEntry, INDEX_OUT_OF_BOUND, SLICE_OUT_OF_BOUND, SPAWN,
    SPAWN_EXCEEDED_MAX_CONTENT_LENGTH, SPAWN_EXCEEDED_MAX_PEAK_MEMORY, SPAWN_MAX_CONTENT_LENGTH,
    SPAWN_MAX_MEMORY, SPAWN_MAX_PEAK_MEMORY, SPAWN_MEMORY_PAGE_SIZE, SPAWN_WRONG_MEMORY_LIMIT,
    WRONG_FORMAT,
};
use crate::types::Indices;
use crate::types::{set_vm_max_cycles, CoreMachineType, Machine};
use crate::TransactionScriptsSyscallsGenerator;
use crate::{ScriptGroup, ScriptVersion};
use ckb_traits::{CellDataProvider, HeaderProvider};
use ckb_types::core::cell::{CellMeta, ResolvedTransaction};
use ckb_vm::{
    cost_model::estimate_cycles,
    registers::{A0, A1, A2, A3, A4, A5, A7},
    DefaultMachineBuilder, Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

pub struct Spawn<DL> {
    data_loader: DL,
    group_inputs: Indices,
    group_outputs: Indices,
    rtx: Arc<ResolvedTransaction>,
    script_group: ScriptGroup,
    script_version: ScriptVersion,
    syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
    outputs: Arc<Vec<CellMeta>>,
    peak_memory: u64,
}

#[allow(clippy::too_many_arguments)]
impl<DL: CellDataProvider + Clone + HeaderProvider + Send + Sync + 'static> Spawn<DL> {
    pub fn new(
        data_loader: DL,
        group_inputs: Indices,
        group_outputs: Indices,
        rtx: Arc<ResolvedTransaction>,
        script_group: ScriptGroup,
        script_version: ScriptVersion,
        syscalls_generator: TransactionScriptsSyscallsGenerator<DL>,
        outputs: Arc<Vec<CellMeta>>,
        peak_memory: u64,
    ) -> Self {
        Self {
            data_loader,
            group_inputs,
            group_outputs,
            rtx,
            script_group,
            script_version,
            syscalls_generator,
            outputs,
            peak_memory,
        }
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_cell_deps
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Result<&CellMeta, u8> {
        let cell_opt = match source {
            Source::Transaction(SourceEntry::Input) => self.resolved_inputs().get(index),
            Source::Transaction(SourceEntry::Output) => self.outputs.get(index),
            Source::Transaction(SourceEntry::CellDep) => self.resolved_cell_deps().get(index),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .and_then(|actual_index| self.resolved_inputs().get(*actual_index)),
            Source::Group(SourceEntry::Output) => self
                .group_outputs
                .get(index)
                .and_then(|actual_index| self.outputs.get(*actual_index)),
            Source::Transaction(SourceEntry::HeaderDep)
            | Source::Group(SourceEntry::CellDep)
            | Source::Group(SourceEntry::HeaderDep) => {
                return Err(INDEX_OUT_OF_BOUND);
            }
        };

        cell_opt.ok_or(INDEX_OUT_OF_BOUND)
    }
}

impl<Mac, DL> Syscalls<Mac> for Spawn<DL>
where
    Mac: SupportMachine,
    DL: CellDataProvider + HeaderProvider + Send + Sync + Clone + 'static,
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != SPAWN {
            return Ok(false);
        }
        // Arguments for positioning child programs.
        let index = machine.registers()[A0].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A1].to_u64())?;
        let bounds = machine.registers()[A2].to_u64();
        let offset = (bounds >> 32) as usize;
        let length = bounds as u32 as usize;
        // Arguments for childs.
        let argc = machine.registers()[A3].to_u64();
        let argv_addr = machine.registers()[A4].to_u64();
        let spgs_addr = machine.registers()[A5].to_u64();
        let memory_limit_addr = spgs_addr;
        let exit_code_addr_addr = spgs_addr.wrapping_add(8);
        let content_addr_addr = spgs_addr.wrapping_add(16);
        let content_length_addr_addr = spgs_addr.wrapping_add(24);
        // Arguments for limiting.
        let memory_limit = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(memory_limit_addr))?
            .to_u64();
        let cycles_limit = machine.max_cycles() - machine.cycles();
        // Arguments for saving outputs from child programs.
        let exit_code_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(exit_code_addr_addr))?;
        let content_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(content_addr_addr))?;
        let content_length_addr = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(content_length_addr_addr))?;
        let content_length = machine.memory_mut().load64(&content_length_addr)?.to_u64();
        // Arguments check.
        if content_length > SPAWN_MAX_CONTENT_LENGTH {
            machine.set_register(A0, Mac::REG::from_u8(SPAWN_EXCEEDED_MAX_CONTENT_LENGTH));
            return Ok(true);
        }
        if memory_limit > SPAWN_MAX_MEMORY || memory_limit == 0 {
            machine.set_register(A0, Mac::REG::from_u8(SPAWN_WRONG_MEMORY_LIMIT));
            return Ok(true);
        }
        if self.peak_memory + memory_limit > SPAWN_MAX_PEAK_MEMORY {
            machine.set_register(A0, Mac::REG::from_u8(SPAWN_EXCEEDED_MAX_PEAK_MEMORY));
            return Ok(true);
        }
        // Build child machine.
        let machine_content = Arc::new(Mutex::new(Vec::<u8>::new()));
        let mut machine_child = {
            let machine_isa = self.script_version.vm_isa();
            let machine_version = self.script_version.vm_version();
            let machine_core = CoreMachineType::new_with_memory(
                machine_isa,
                machine_version,
                cycles_limit,
                (memory_limit * SPAWN_MEMORY_PAGE_SIZE) as usize,
            );
            let machine_builder = DefaultMachineBuilder::new(machine_core)
                .instruction_cycle_func(Box::new(estimate_cycles));
            let machine_syscalls = self
                .syscalls_generator
                .generate_same_syscalls(self.script_version, &self.script_group);
            let machine_builder = machine_syscalls
                .into_iter()
                .fold(machine_builder, |builder, syscall| builder.syscall(syscall));
            let machine_builder = machine_builder.syscall(Box::new(
                self.syscalls_generator.build_get_memory_limit(memory_limit),
            ));
            let machine_builder = machine_builder.syscall(Box::new(
                self.syscalls_generator
                    .build_set_content(Arc::clone(&machine_content), content_length.to_u64()),
            ));
            let machine_builder = machine_builder.syscall(Box::new(Spawn::new(
                self.data_loader.clone(),
                Arc::clone(&self.group_inputs),
                Arc::clone(&self.group_outputs),
                Arc::clone(&self.rtx),
                self.script_group.clone(),
                self.script_version,
                self.syscalls_generator.clone(),
                Arc::clone(&self.outputs),
                self.peak_memory + memory_limit,
            )));
            let mut machine_child = Machine::new(machine_builder.build());
            set_vm_max_cycles(&mut machine_child, cycles_limit);
            machine_child
        };
        // Get binary.
        let program = {
            let cell = self.fetch_cell(source, index as usize);
            if let Err(err) = cell {
                machine.set_register(A0, Mac::REG::from_u8(err));
                return Ok(true);
            }
            let cell = cell.unwrap();
            let data = self.data_loader.load_cell_data(cell).ok_or_else(|| {
                VMError::Unexpected(format!(
                    "Unexpected load_cell_data failed {}",
                    cell.out_point,
                ))
            })?;
            let size = data.len();
            if offset >= size {
                machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                return Ok(true);
            };
            if length == 0 {
                data.slice(offset..size)
            } else {
                let end = offset.checked_add(length).ok_or(VMError::MemOutOfBound)?;
                if end > size {
                    machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                    return Ok(true);
                }
                data.slice(offset..end)
            }
        };
        // Build arguments.
        let mut addr = argv_addr.to_u64();
        let mut argv_vec = Vec::new();
        for _ in 0..argc {
            let target_addr = machine
                .memory_mut()
                .load64(&Mac::REG::from_u64(addr))?
                .to_u64();
            let cstr = load_c_string(machine, target_addr)?;
            argv_vec.push(cstr);
            addr += 8;
        }
        // Run child machine.
        match machine_child.load_program(&program, &argv_vec) {
            Ok(size) => {
                machine_child
                    .machine
                    .add_cycles_no_checking(transferred_byte_cycles(size))?;
            }
            Err(_) => {
                machine.set_register(A0, Mac::REG::from_u8(WRONG_FORMAT));
                return Ok(true);
            }
        }
        // Check result.
        match machine_child.run() {
            Ok(data) => {
                machine.set_register(A0, Mac::REG::from_u32(0));
                machine
                    .memory_mut()
                    .store8(&exit_code_addr, &Mac::REG::from_i8(data))?;
                machine
                    .memory_mut()
                    .store_bytes(content_addr.to_u64(), &machine_content.lock().unwrap())?;
                machine.memory_mut().store64(
                    &content_length_addr,
                    &Mac::REG::from_u64(machine_content.lock().unwrap().len() as u64),
                )?;
                machine.add_cycles_no_checking(machine_child.machine.cycles())?;
                Ok(true)
            }
            Err(err) => Err(err),
        }
    }
}
