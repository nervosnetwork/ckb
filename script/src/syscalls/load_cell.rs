use crate::syscalls::{Source, ITEM_MISSING, LOAD_CELL_SYSCALL_NUMBER, SUCCESS};
use ckb_core::transaction::CellOutput;
use ckb_protocol::CellOutput as FbsCellOutput;
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A3, A4, A7};
use flatbuffers::FlatBufferBuilder;
use std::cmp;

#[derive(Debug)]
pub struct LoadCell<'a> {
    outputs: &'a [&'a CellOutput],
    input_cells: &'a [&'a CellOutput],
    current: &'a CellOutput,
}

impl<'a> LoadCell<'a> {
    pub fn new(
        outputs: &'a [&'a CellOutput],
        input_cells: &'a [&'a CellOutput],
        current: &'a CellOutput,
    ) -> LoadCell<'a> {
        LoadCell {
            outputs,
            input_cells,
            current,
        }
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Option<&CellOutput> {
        match source {
            Source::Input => self.input_cells.get(index).cloned(),
            Source::Output => self.outputs.get(index).cloned(),
            Source::Current => Some(self.current),
        }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for LoadCell<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_CELL_SYSCALL_NUMBER {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let size = machine.memory_mut().load64(size_addr)? as usize;

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let cell = self.fetch_cell(source, index);
        if cell.is_none() {
            machine.registers_mut()[A0] = R::from_u8(ITEM_MISSING);
            return Ok(true);
        }
        let cell = cell.unwrap();

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
        let offset = FbsCellOutput::build(&mut builder, cell);
        builder.finish(offset, None);
        let data = builder.finished_data();

        let offset = machine.registers()[A2].to_usize();
        let full_size = data.len() - offset;
        let real_size = cmp::min(size, full_size);
        machine.memory_mut().store64(size_addr, full_size as u64)?;
        machine
            .memory_mut()
            .store_bytes(addr, &data[offset..offset + real_size])?;
        machine.registers_mut()[A0] = R::from_u8(SUCCESS);
        Ok(true)
    }
}
