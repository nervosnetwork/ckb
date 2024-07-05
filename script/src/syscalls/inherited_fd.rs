use crate::syscalls::{INHERITED_FD, YIELD_CYCLES_BASE};
use crate::types::{Fd, FdArgs, Message, VmId};
use ckb_vm::{
    registers::{A0, A1, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct InheritedFd {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl InheritedFd {
    pub fn new(id: VmId, message_box: Arc<Mutex<Vec<Message>>>) -> Self {
        Self { id, message_box }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for InheritedFd {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != INHERITED_FD {
            return Ok(false);
        }
        let buffer_addr = machine.registers()[A0].to_u64();
        let length_addr = machine.registers()[A1].to_u64();
        machine.add_cycles_no_checking(YIELD_CYCLES_BASE)?;
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::InheritedFileDescriptor(
                self.id,
                FdArgs {
                    fd: Fd(0),
                    length: 0,
                    buffer_addr,
                    length_addr,
                },
            ));
        Err(VMError::Yield)
    }
}
