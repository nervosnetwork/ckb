use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::store_data, InputField, Source, SourceEntry, INDEX_OUT_OF_BOUND,
        LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER, LOAD_INPUT_SYSCALL_NUMBER, SUCCESS,
    },
};
use byteorder::{LittleEndian, WriteBytesExt};
use ckb_types::{
    packed::{CellInput, CellInputVec},
    prelude::*,
};
use ckb_vm::{
    registers::{A0, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadInput<'a> {
    inputs: CellInputVec,
    group_inputs: &'a [usize],
}

impl<'a> LoadInput<'a> {
    pub fn new(inputs: CellInputVec, group_inputs: &'a [usize]) -> LoadInput<'a> {
        LoadInput {
            inputs,
            group_inputs,
        }
    }

    fn fetch_input(&self, source: Source, index: usize) -> Result<CellInput, u8> {
        match source {
            Source::Transaction(SourceEntry::Input) => {
                self.inputs.get(index).ok_or(INDEX_OUT_OF_BOUND)
            }
            Source::Transaction(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| self.inputs.get(*actual_index).ok_or(INDEX_OUT_OF_BOUND)),
            Source::Group(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
        }
    }

    fn load_full<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        input: &CellInput,
    ) -> Result<u64, VMError> {
        let data = input.as_slice();
        let wrote_size = store_data(machine, data)?;
        Ok(wrote_size)
    }

    fn load_by_field<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        input: &CellInput,
    ) -> Result<u64, VMError> {
        let field = InputField::parse_from_u64(machine.registers()[A5].to_u64())?;

        match field {
            InputField::OutPoint => {
                let previous_output = input.previous_output();
                let data = previous_output.as_slice();
                store_data(machine, data)
            }
            InputField::Since => {
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(input.since().unpack())?;
                store_data(machine, &buffer)
            }
        }
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadInput<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let load_by_field = match machine.registers()[A7].to_u64() {
            LOAD_INPUT_SYSCALL_NUMBER => false,
            LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER => true,
            _ => return Ok(false),
        };

        let index = machine.registers()[A3].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let input = self.fetch_input(source, index as usize);
        if let Err(err) = input {
            machine.set_register(A0, Mac::REG::from_u8(err));
            return Ok(true);
        }
        let input = input.unwrap();

        let len = if load_by_field {
            self.load_by_field(machine, &input)?
        } else {
            self.load_full(machine, &input)?
        };

        machine.add_cycles_no_checking(transferred_byte_cycles(len))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
