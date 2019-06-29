use crate::syscalls::{
    utils::store_data, Source, SourceEntry, INDEX_OUT_OF_BOUND, LOAD_WITNESS_SYSCALL_NUMBER,
    SUCCESS,
};
use ckb_core::transaction::Witness;
use ckb_protocol::Witness as FbsWitness;
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use flatbuffers::FlatBufferBuilder;

#[derive(Debug)]
pub struct LoadWitness<'a> {
    witnesses: &'a [Witness],
    group_inputs: &'a [usize],
}

impl<'a> LoadWitness<'a> {
    pub fn new(witnesses: &'a [Witness], group_inputs: &'a [usize]) -> LoadWitness<'a> {
        LoadWitness {
            witnesses,
            group_inputs,
        }
    }

    fn fetch_witness(&self, source: Source, index: usize) -> Option<&Witness> {
        match source {
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .and_then(|actual_index| self.witnesses.get(*actual_index)),
            Source::Transaction(SourceEntry::Input) => self.witnesses.get(index),
            _ => None,
        }
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadWitness<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_WITNESS_SYSCALL_NUMBER {
            return Ok(false);
        }

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let witness = self.fetch_witness(source, index);
        if witness.is_none() {
            machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
            return Ok(true);
        }
        let witness = witness.unwrap();

        let mut builder = FlatBufferBuilder::new();
        let offset = FbsWitness::build(&mut builder, witness);
        builder.finish(offset, None);
        let data = builder.finished_data();

        store_data(machine, &data)?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 10)?;
        Ok(true)
    }
}
