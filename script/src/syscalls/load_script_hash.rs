use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{utils::store_data, LOAD_SCRIPT_HASH_SYSCALL_NUMBER, SUCCESS},
    types::VmData,
};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct LoadScriptHash<DL> {
    vm_data: Arc<VmData<DL>>,
}

impl<DL> LoadScriptHash<DL> {
    pub fn new(vm_data: &Arc<VmData<DL>>) -> LoadScriptHash<DL> {
        LoadScriptHash {
            vm_data: Arc::clone(vm_data),
        }
    }
}

impl<Mac: SupportMachine, DL: Send + Sync> Syscalls<Mac> for LoadScriptHash<DL> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_SCRIPT_HASH_SYSCALL_NUMBER {
            return Ok(false);
        }

        let data = self.vm_data.current_script_hash().as_reader().raw_data();
        let wrote_size = store_data(machine, data)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
