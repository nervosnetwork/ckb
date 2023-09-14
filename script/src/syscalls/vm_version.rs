use crate::syscalls::VM_VERSION;
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug, Default)]
pub struct VMVersion {}

impl VMVersion {
    pub fn new() -> Self {
        Self {}
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for VMVersion {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != VM_VERSION {
            return Ok(false);
        }
        machine.set_register(A0, Mac::REG::from_u32(machine.version()));
        Ok(true)
    }
}
