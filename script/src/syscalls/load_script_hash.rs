use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{utils::store_data, LOAD_SCRIPT_HASH_SYSCALL_NUMBER, SUCCESS},
    types::VmData,
};
use ckb_types::packed::Byte32;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct LoadScriptHash {
    hash: Byte32,
}

impl LoadScriptHash {
    pub fn new<DL>(vm_data: &Arc<VmData<DL>>) -> LoadScriptHash {
        LoadScriptHash {
            hash: vm_data.sg_data.script_group.script.calc_script_hash(),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for LoadScriptHash {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_SCRIPT_HASH_SYSCALL_NUMBER {
            return Ok(false);
        }

        let data = self.hash.as_reader().raw_data();
        let wrote_size = store_data(machine, data)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
