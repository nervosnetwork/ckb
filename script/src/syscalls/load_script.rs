use crate::syscalls::{utils::store_data, LOAD_SCRIPT_SYSCALL_NUMBER, SUCCESS};
use ckb_types::{packed::Script, prelude::*};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadScript {
    script: Script,
}

impl LoadScript {
    pub fn new(script: Script) -> Self {
        Self { script }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for LoadScript {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_SCRIPT_SYSCALL_NUMBER {
            return Ok(false);
        }

        let data = self.script.as_slice();
        store_data(machine, &data)?;

        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 10)?;
        Ok(true)
    }
}
