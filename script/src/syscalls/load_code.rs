use crate::syscalls::{
    Source, SourceEntry, INDEX_OUT_OF_BOUND, ITEM_MISSING, LOAD_CODE_SYSCALL_NUMBER,
    SLICE_OUT_OF_BOUND, SUCCESS,
};
use ckb_core::cell::{CellMeta, ResolvedOutPoint};
use ckb_store::LazyLoadCellOutput;
use ckb_vm::{
    memory::{Memory, FLAG_EXECUTABLE, FLAG_FREEZED},
    registers::{A0, A1, A2, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

pub struct LoadCode<'a, CS> {
    store: Arc<CS>,
    outputs: &'a [CellMeta],
    resolved_inputs: &'a [&'a ResolvedOutPoint],
    resolved_deps: &'a [&'a ResolvedOutPoint],
    group_inputs: &'a [usize],
    group_outputs: &'a [usize],
}

impl<'a, CS: LazyLoadCellOutput + 'a> LoadCode<'a, CS> {
    pub fn new(
        store: Arc<CS>,
        outputs: &'a [CellMeta],
        resolved_inputs: &'a [&'a ResolvedOutPoint],
        resolved_deps: &'a [&'a ResolvedOutPoint],
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
    ) -> LoadCode<'a, CS> {
        LoadCode {
            store,
            outputs,
            resolved_inputs,
            resolved_deps,
            group_inputs,
            group_outputs,
        }
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Result<&'a CellMeta, u8> {
        match source {
            Source::Transaction(SourceEntry::Input) => self
                .resolved_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|r| r.cell().ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::Output) => {
                self.outputs.get(index).ok_or(INDEX_OUT_OF_BOUND)
            }
            Source::Transaction(SourceEntry::Dep) => self
                .resolved_deps
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|r| r.cell().ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| {
                    self.resolved_inputs
                        .get(*actual_index)
                        .ok_or(INDEX_OUT_OF_BOUND)
                })
                .and_then(|r| r.cell().ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Output) => self
                .group_outputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| self.outputs.get(*actual_index).ok_or(INDEX_OUT_OF_BOUND)),
            Source::Group(SourceEntry::Dep) => Err(INDEX_OUT_OF_BOUND),
        }
    }
}

impl<'a, Mac: SupportMachine, CS: LazyLoadCellOutput> Syscalls<Mac> for LoadCode<'a, CS> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_CODE_SYSCALL_NUMBER {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let memory_size = machine.registers()[A1].to_usize();
        let content_offset = machine.registers()[A2].to_usize();
        let content_size = machine.registers()[A3].to_usize();

        let index = machine.registers()[A4].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A5].to_u64())?;

        let cell = self.fetch_cell(source, index);
        if cell.is_err() {
            machine.set_register(A0, Mac::REG::from_u8(cell.unwrap_err()));
            return Ok(true);
        }
        let cell = cell.unwrap();
        let output = self.store.lazy_load_cell_output(&cell);

        if content_offset >= output.data.len()
            || (content_offset + content_size) > output.data.len()
            || content_size > memory_size
        {
            machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
            return Ok(true);
        }
        machine.memory_mut().init_pages(
            addr,
            memory_size,
            FLAG_EXECUTABLE | FLAG_FREEZED,
            Some(
                output
                    .data
                    .slice(content_offset, content_offset + content_size),
            ),
            0,
        )?;

        machine.add_cycles(output.data.len() as u64 * 10)?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
