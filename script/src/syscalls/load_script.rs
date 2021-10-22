use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{utils::store_data, LOAD_SCRIPT_SYSCALL_NUMBER, SUCCESS},
};
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
        let wrote_size = store_data(machine, data)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
