use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        INDEX_OUT_OF_BOUND, LOAD_WITNESS_SYSCALL_NUMBER, SUCCESS, Source, SourceEntry,
        utils::store_data,
    },
    types::SgData,
};
use ckb_types::packed::{Bytes, BytesVec};
use ckb_vm::{
    Error as VMError, Register, SupportMachine, Syscalls,
    registers::{A0, A3, A4, A7},
};

#[derive(Debug)]
pub struct LoadWitness<DL> {
    sg_data: SgData<DL>,
}

impl<DL: Clone> LoadWitness<DL> {
    pub fn new(sg_data: &SgData<DL>) -> Self {
        LoadWitness {
            sg_data: sg_data.clone(),
        }
    }

    #[inline]
    fn witnesses(&self) -> BytesVec {
        self.sg_data.rtx.transaction.witnesses()
    }

    fn fetch_witness(&self, source: Source, index: usize) -> Option<Bytes> {
        match source {
            Source::Group(SourceEntry::Input) => self
                .sg_data
                .group_inputs()
                .get(index)
                .and_then(|actual_index| self.witnesses().get(*actual_index)),
            Source::Group(SourceEntry::Output) => self
                .sg_data
                .group_outputs()
                .get(index)
                .and_then(|actual_index| self.witnesses().get(*actual_index)),
            Source::Transaction(SourceEntry::Input) => self.witnesses().get(index),
            Source::Transaction(SourceEntry::Output) => self.witnesses().get(index),
            _ => None,
        }
    }
}

impl<Mac: SupportMachine, DL: Sync + Send + Clone> Syscalls<Mac> for LoadWitness<DL> {
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
