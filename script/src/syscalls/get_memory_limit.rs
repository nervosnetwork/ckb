use crate::syscalls::GET_MEMORY_LIMIT;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct GetMemoryLimit {
    memory_limit: u64,
}

impl GetMemoryLimit {
    pub fn new(memory_limit: u64) -> Self {
        Self { memory_limit }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for GetMemoryLimit {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != GET_MEMORY_LIMIT {
            return Ok(false);
        }
        machine.set_register(A0, Mac::REG::from_u64(self.memory_limit));
        Ok(true)
    }
}
