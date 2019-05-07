use crate::common::{CurrentCell, LazyLoadCellOutput};
use crate::syscalls::{Source, ITEM_MISSING, LOAD_CELL_SYSCALL_NUMBER, SUCCESS};
use ckb_core::cell::{CellMeta, ResolvedOutPoint};
use ckb_protocol::CellOutput as FbsCellOutput;
use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use flatbuffers::FlatBufferBuilder;
use std::cmp;
use std::sync::Arc;

pub struct LoadCell<'a, CS> {
    store: Arc<CS>,
    outputs: &'a [CellMeta],
    resolved_inputs: &'a [&'a ResolvedOutPoint],
    current: CurrentCell,
    resolved_deps: &'a [&'a ResolvedOutPoint],
}

impl<'a, CS: LazyLoadCellOutput + 'a> LoadCell<'a, CS> {
    pub fn new(
        store: Arc<CS>,
        outputs: &'a [CellMeta],
        resolved_inputs: &'a [&'a ResolvedOutPoint],
        current: CurrentCell,
        resolved_deps: &'a [&'a ResolvedOutPoint],
    ) -> LoadCell<'a, CS> {
        LoadCell {
            store,
            outputs,
            resolved_inputs,
            current,
            resolved_deps,
        }
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Option<&'a CellMeta> {
        match source {
            Source::Input => self.resolved_inputs.get(index).and_then(|r| r.cell()),
            Source::Output => self.outputs.get(index),
            Source::Current => match self.current {
                CurrentCell::Input(index) => self.resolved_inputs.get(index).and_then(|r| r.cell()),
                CurrentCell::Output(index) => self.outputs.get(index),
            },
            Source::Dep => self.resolved_deps.get(index).and_then(|r| r.cell()),
        }
    }
}

impl<'a, Mac: SupportMachine, CS: LazyLoadCellOutput> Syscalls<Mac> for LoadCell<'a, CS> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_CELL_SYSCALL_NUMBER {
            return Ok(false);
        }
        machine.add_cycles(100)?;

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let size = machine
            .memory_mut()
            .load64(&Mac::REG::from_usize(size_addr))?
            .to_usize();

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let cell = self.fetch_cell(source, index);
        if cell.is_none() {
            machine.set_register(A0, Mac::REG::from_u8(ITEM_MISSING));
            return Ok(true);
        }
        let cell = cell.unwrap();
        let output = self.store.lazy_load_cell_output(&cell);

        // NOTE: this is a very expensive operation here since we need to copy
        // everything in a cell to a flatbuffer object, serialize the object
        // into a buffer, and then copy requested data to VM memory space. So
        // we should charge cycles proportional to the full Cell size no matter
        // how much data the actual script is requesting, the per-byte cycle charged
        // here, should also be significantly higher than LOAD_CELL_BY_FIELD.
        // Also, while this is debatable, I suggest we charge full cycles for
        // subsequent calls even if we have cache implemented here.
        // TODO: find a way to cache this without consuming too much memory
        let mut builder = FlatBufferBuilder::new();
        let offset = FbsCellOutput::build(&mut builder, &output);
        builder.finish(offset, None);
        let data = builder.finished_data();

        let offset = machine.registers()[A2].to_usize();
        let full_size = data.len() - offset;
        let real_size = cmp::min(size, full_size);
        machine.memory_mut().store64(
            &Mac::REG::from_usize(size_addr),
            &Mac::REG::from_usize(full_size),
        )?;
        machine
            .memory_mut()
            .store_bytes(addr, &data[offset..offset + real_size])?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 100)?;
        Ok(true)
    }
}
