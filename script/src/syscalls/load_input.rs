use crate::syscalls::{
    utils::store_data, InputField, Source, INDEX_OUT_OF_BOUND, ITEM_MISSING,
    LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER, LOAD_INPUT_SYSCALL_NUMBER, SUCCESS,
};
use byteorder::{LittleEndian, WriteBytesExt};
use ckb_core::transaction::CellInput;
use ckb_protocol::CellInput as FbsCellInput;
use ckb_protocol::{Bytes as FbsBytes, CellInputBuilder, OutPoint as FbsOutPoint};
use ckb_vm::{
    registers::{A0, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use flatbuffers::FlatBufferBuilder;

#[derive(Debug)]
pub struct LoadInput<'a> {
    inputs: &'a [&'a CellInput],
}

impl<'a> LoadInput<'a> {
    pub fn new(inputs: &'a [&'a CellInput]) -> LoadInput<'a> {
        LoadInput { inputs }
    }

    fn fetch_input(&self, source: Source, index: usize) -> Result<&CellInput, u8> {
        match source {
            Source::Input => self.inputs.get(index).cloned().ok_or(INDEX_OUT_OF_BOUND),
            Source::Output => Err(ITEM_MISSING),
            Source::Dep => Err(ITEM_MISSING),
        }
    }

    fn load_full<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        input: &CellInput,
    ) -> Result<usize, VMError> {
        // NOTE: like LOAD_CELL, this could also be expensive assuming the
        // input has too many args. So right now we also charge for the full
        // serialized input size. IF there's a chance we can get partial read
        // working directly from storage to VM memory, we can revise the cycle
        // costs here.
        let mut builder = FlatBufferBuilder::new();
        let offset = FbsCellInput::build(&mut builder, &input);
        builder.finish(offset, None);
        let data = builder.finished_data();

        store_data(machine, &data)?;
        Ok(data.len())
    }

    fn load_by_field<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        input: &CellInput,
    ) -> Result<usize, VMError> {
        let field = InputField::parse_from_u64(machine.registers()[A5].to_u64())?;

        let result = match field {
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
            InputField::Since => {
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(input.since)?;
                store_data(machine, &buffer)?;
                buffer.len()
            }
        };
        Ok(result)
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadInput<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let (load_by_field, cycle_factor) = match machine.registers()[A7].to_u64() {
            LOAD_INPUT_SYSCALL_NUMBER => (false, 100),
            LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER => (true, 10),
            _ => return Ok(false),
        };
        machine.add_cycles(cycle_factor)?;

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let input = self.fetch_input(source, index);
        if input.is_err() {
            machine.set_register(A0, Mac::REG::from_u8(input.unwrap_err()));
            return Ok(true);
        }
        let input = input.unwrap();

        let len = if load_by_field {
            self.load_by_field(machine, &input)?
        } else {
            self.load_full(machine, &input)?
        };

        machine.add_cycles(len as u64 * cycle_factor)?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
