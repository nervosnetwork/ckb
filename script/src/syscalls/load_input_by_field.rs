use crate::syscalls::{
    utils::store_data, InputField, Source, ITEM_MISSING, LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER,
    SUCCESS,
};
use ckb_core::transaction::CellInput;
use ckb_protocol::{Bytes as FbsBytes, CellInputBuilder, OutPoint as FbsOutPoint};
use ckb_vm::{
    registers::{A0, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
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

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadInputByField<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER {
            return Ok(false);
        }
        machine.add_cycles(10)?;

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;
        let field = InputField::parse_from_u64(machine.registers()[A5].to_u64())?;

        let input = self.fetch_input(source, index);
        if input.is_none() {
            machine.set_register(A0, Mac::REG::from_u8(ITEM_MISSING));
            return Ok(true);
        }
        let input = input.unwrap();

        let data_length = match field {
            InputField::Args => {
                let mut builder = FlatBufferBuilder::new();
                let vec = input
                    .args
                    .iter()
                    .map(|argument| FbsBytes::build(&mut builder, argument))
                    .collect::<Vec<_>>();
                let args = builder.create_vector(&vec);
                // Since a vector cannot be root FlatBuffer type, we have
                // to wrap args here inside a CellInput struct.
                let mut input_builder = CellInputBuilder::new(&mut builder);
                input_builder.add_args(args);
                let offset = input_builder.finish();
                builder.finish(offset, None);
                let data = builder.finished_data();
                store_data(machine, data)?;
                data.len()
            }
            InputField::OutPoint => {
                let mut builder = FlatBufferBuilder::new();
                let offset = FbsOutPoint::build(&mut builder, &input.previous_output);
                builder.finish(offset, None);
                let data = builder.finished_data();
                store_data(machine, data)?;
                data.len()
            }
        };
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data_length as u64 * 10)?;
        Ok(true)
    }
}
