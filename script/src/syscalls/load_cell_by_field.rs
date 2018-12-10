use byteorder::{LittleEndian, WriteBytesExt};
use ckb_core::transaction::CellOutput;
use ckb_protocol::Script as FbsScript;
use crate::syscalls::{Field, Source, ITEM_MISSING, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER, SUCCESS};
use ckb_vm::{
    CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A3, A4, A5, A7,
};
use flatbuffers::FlatBufferBuilder;
use std::cmp;

#[derive(Debug)]
pub struct LoadCellByField<'a> {
    outputs: &'a [&'a CellOutput],
    input_cells: &'a [&'a CellOutput],
    current: &'a CellOutput,
}

impl<'a> LoadCellByField<'a> {
    pub fn new(
        outputs: &'a [&'a CellOutput],
        input_cells: &'a [&'a CellOutput],
        current: &'a CellOutput,
    ) -> LoadCellByField<'a> {
        LoadCellByField {
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

fn store_data<R: Register, M: Memory>(
    machine: &mut CoreMachine<R, M>,
    data: &[u8],
) -> Result<(), VMError> {
    let addr = machine.registers()[A0].to_usize();
    let size_addr = machine.registers()[A1].to_usize();
    let offset = machine.registers()[A2].to_usize();

    let size = machine.memory_mut().load64(size_addr)? as usize;
    let real_size = cmp::min(size, data.len() - offset);
    machine.memory_mut().store64(size_addr, real_size as u64)?;
    machine
        .memory_mut()
        .store_bytes(addr, &data[offset..offset + real_size])?;
    Ok(())
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for LoadCellByField<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_CELL_BY_FIELD_SYSCALL_NUMBER {
            return Ok(false);
        }

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;
        let field = Field::parse_from_u64(machine.registers()[A5].to_u64())?;

        let cell = self
            .fetch_cell(source, index)
            .ok_or_else(|| VMError::OutOfBound)?;

        let return_code = match field {
            Field::Capacity => {
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(cell.capacity)?;
                store_data(machine, &buffer)?;
                SUCCESS
            }
            Field::Data => {
                store_data(machine, &cell.data)?;
                SUCCESS
            }
            Field::LockHash => {
                store_data(machine, &cell.lock.as_bytes())?;
                SUCCESS
            }
            Field::Contract => match cell.contract {
                Some(ref contract) => {
                    let mut builder = FlatBufferBuilder::new();
                    let offset = FbsScript::build(&mut builder, &contract);
                    builder.finish(offset, None);
                    let data = builder.finished_data();
                    store_data(machine, data)?;
                    SUCCESS
                }
                None => ITEM_MISSING,
            },
            Field::ContractHash => match cell.contract {
                Some(ref contract) => {
                    store_data(machine, &contract.type_hash().as_bytes())?;
                    SUCCESS
                }
                None => ITEM_MISSING,
            },
        };
        machine.registers_mut()[A0] = R::from_u8(return_code);
        Ok(true)
    }
}
