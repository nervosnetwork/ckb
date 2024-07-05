use crate::syscalls::{INVALID_FD, READ, YIELD_CYCLES_BASE};
use crate::types::{Fd, FdArgs, Message, VmId};
use ckb_vm::{
    registers::{A0, A1, A2, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Read {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl Read {
    pub fn new(id: VmId, message_box: Arc<Mutex<Vec<Message>>>) -> Self {
        Self { id, message_box }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Read {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != READ {
            return Ok(false);
        }
        let fd = Fd(machine.registers()[A0].to_u64());
        let buffer_addr = machine.registers()[A1].to_u64();
        let length_addr = machine.registers()[A2].to_u64();
        let length = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(length_addr))?
            .to_u64();

        // We can only do basic checks here, when the message is actually processed,
        // more complete checks will be performed.
        if !fd.is_read() {
            machine.set_register(A0, Mac::REG::from_u8(INVALID_FD));
            return Ok(true);
        }
        machine.add_cycles_no_checking(YIELD_CYCLES_BASE)?;
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::FdRead(
                self.id,
                FdArgs {
                    fd,
                    length,
                    buffer_addr,
                    length_addr,
                },
            ));
        Err(VMError::Yield)
    }
}
