use crate::syscalls::{SPAWN_YIELD_CYCLES_BASE, WAIT};
use crate::types::{Message, VmId, WaitArgs};
use ckb_vm::{
    registers::{A0, A1, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Wait {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl Wait {
    pub fn new(id: VmId, message_box: Arc<Mutex<Vec<Message>>>) -> Self {
        Self { id, message_box }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Wait {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != WAIT {
            return Ok(false);
        }
        let target_id = machine.registers()[A0].to_u64();
        let exit_code_addr = machine.registers()[A1].to_u64();
        machine.add_cycles_no_checking(SPAWN_YIELD_CYCLES_BASE)?;
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::Wait(
                self.id,
                WaitArgs {
                    target_id,
                    exit_code_addr,
                },
            ));
        Err(VMError::Yield)
    }
}
