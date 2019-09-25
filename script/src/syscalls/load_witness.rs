use crate::syscalls::{
    utils::store_data, Source, SourceEntry, INDEX_OUT_OF_BOUND, LOAD_WITNESS_SYSCALL_NUMBER,
    SUCCESS,
};
use ckb_types::packed::{Bytes, BytesVec};
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadWitness<'a> {
    witnesses: BytesVec,
    group_inputs: &'a [usize],
}

impl<'a> LoadWitness<'a> {
    pub fn new(witnesses: BytesVec, group_inputs: &'a [usize]) -> LoadWitness<'a> {
        LoadWitness {
            witnesses,
            group_inputs,
        }
    }

    fn fetch_witness(&self, source: Source, index: usize) -> Option<Bytes> {
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

        let index = machine.registers()[A3].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let witness = self.fetch_witness(source, index as usize);
        if witness.is_none() {
            machine.set_register(A0, Mac::REG::from_u8(INDEX_OUT_OF_BOUND));
            return Ok(true);
        }
        let witness = witness.unwrap();
        let data = witness.raw_data();

        store_data(machine, &data)?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 10)?;
        Ok(true)
    }
}
