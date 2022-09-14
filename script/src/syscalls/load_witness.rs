use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::store_data, Source, SourceEntry, INDEX_OUT_OF_BOUND, LOAD_WITNESS_SYSCALL_NUMBER,
        SUCCESS,
    },
};
use ckb_types::core::cell::ResolvedTransaction;
use ckb_types::packed::{Bytes, BytesVec};
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::rc::Rc;

#[derive(Debug)]
pub struct LoadWitness {
    rtx: Rc<ResolvedTransaction>,
    group_inputs: Rc<Vec<usize>>,
    group_outputs: Rc<Vec<usize>>,
}

impl LoadWitness {
    pub fn new(
        rtx: Rc<ResolvedTransaction>,
        group_inputs: Rc<Vec<usize>>,
        group_outputs: Rc<Vec<usize>>,
    ) -> LoadWitness {
        LoadWitness {
            rtx,
            group_inputs,
            group_outputs,
        }
    }

    #[inline]
    fn witnesses(&self) -> BytesVec {
        self.rtx.transaction.witnesses()
    }

    fn fetch_witness(&self, source: Source, index: usize) -> Option<Bytes> {
        match source {
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .and_then(|actual_index| self.witnesses().get(*actual_index)),
            Source::Group(SourceEntry::Output) => self
                .group_outputs
                .get(index)
                .and_then(|actual_index| self.witnesses().get(*actual_index)),
            Source::Transaction(SourceEntry::Input) => self.witnesses().get(index),
            Source::Transaction(SourceEntry::Output) => self.witnesses().get(index),
            _ => None,
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for LoadWitness {
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
        let wrote_size = store_data(machine, &data)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
