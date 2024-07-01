use crate::syscalls::PROCESS_ID;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug, Default)]
pub struct ProcessID {
    id: u64,
}

impl ProcessID {
    pub fn new(id: u64) -> Self {
        Self { id }
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
