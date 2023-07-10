use crate::syscalls::CURRENT_MEMORY;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug, Default)]
pub struct CurrentMemory {
    value: u64,
}

impl CurrentMemory {
    pub fn new(value: u64) -> Self {
        Self { value }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for CurrentMemory {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != CURRENT_MEMORY {
            return Ok(false);
        }
        machine.set_register(A0, Mac::REG::from_u64(self.value));
        Ok(true)
    }
}
