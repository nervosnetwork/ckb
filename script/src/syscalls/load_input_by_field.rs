use crate::syscalls::{
    utils::store_data, InputField, Source, ITEM_MISSING, LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER,
    SUCCESS,
};
use ckb_core::transaction::CellInput;
use ckb_protocol::{OutPoint as FbsOutPoint, Script as FbsScript};
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A3, A4, A5, A7};
use flatbuffers::FlatBufferBuilder;

#[derive(Debug)]
pub struct LoadInputByField<'a> {
    inputs: &'a [&'a CellInput],
    current: Option<&'a CellInput>,
}

impl<'a> LoadInputByField<'a> {
    pub fn new(
        inputs: &'a [&'a CellInput],
        current: Option<&'a CellInput>,
    ) -> LoadInputByField<'a> {
        LoadInputByField { inputs, current }
    }

    fn fetch_input(&self, source: Source, index: usize) -> Option<&CellInput> {
        match source {
            Source::Input => self.inputs.get(index).cloned(),
            Source::Output => None,
            Source::Current => self.current,
            Source::Dep => None,
        }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for LoadInputByField<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER {
            return Ok(false);
        }

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;
        let field = InputField::parse_from_u64(machine.registers()[A5].to_u64())?;

        let input = self.fetch_input(source, index);
        if input.is_none() {
            machine.registers_mut()[A0] = R::from_u8(ITEM_MISSING);
            return Ok(true);
        }
        let input = input.unwrap();

        match field {
            InputField::Unlock => {
                let mut builder = FlatBufferBuilder::new();
                let offset = FbsScript::build(&mut builder, &input.unlock);
                builder.finish(offset, None);
                let data = builder.finished_data();
                store_data(machine, data)?;
            }
            InputField::OutPoint => {
                let mut builder = FlatBufferBuilder::new();
                let offset = FbsOutPoint::build(&mut builder, &input.previous_output);
                builder.finish(offset, None);
                let data = builder.finished_data();
                store_data(machine, data)?;
            }
        };
        machine.registers_mut()[A0] = R::from_u8(SUCCESS);
        Ok(true)
    }
}
