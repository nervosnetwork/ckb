use crate::syscalls::PROCESS_ID;
use crate::types::VmId;
use ckb_vm::{
    Error as VMError, Register, SupportMachine, Syscalls,
    registers::{A0, A7},
};

#[derive(Debug, Default)]
pub struct ProcessID {
    id: u64,
}

impl ProcessID {
    pub fn new(vm_id: &VmId) -> Self {
        Self { id: *vm_id }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for ProcessID {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != PROCESS_ID {
            return Ok(false);
        }
        machine.set_register(A0, Mac::REG::from_u64(self.id));
        Ok(true)
    }
}
