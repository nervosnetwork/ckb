use crate::syscalls::CLOSE;
use crate::types::{Message, PipeId, VmId};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Close {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl Close {
    pub fn new(id: VmId, message_box: Arc<Mutex<Vec<Message>>>) -> Self {
        Self { id, message_box }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Close {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != CLOSE {
            return Ok(false);
        }
        let pipe = PipeId(machine.registers()[A0].to_u64());
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::Close(self.id, pipe));
        Err(VMError::External("YIELD".to_string()))
    }
}
