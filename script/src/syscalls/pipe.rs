use crate::syscalls::PIPE;
use crate::v2_types::{Message, PipeArgs, VmId};
use ckb_vm::{
    registers::{A0, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Pipe {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl Pipe {
    pub fn new(id: VmId, message_box: Arc<Mutex<Vec<Message>>>) -> Self {
        Self { id, message_box }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Pipe {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != PIPE {
            return Ok(false);
        }
        let pipe1_addr = machine.registers()[A0].to_u64();
        let pipe2_addr = pipe1_addr.wrapping_add(8);
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::Pipe(
                self.id,
                PipeArgs {
                    pipe1_addr,
                    pipe2_addr,
                },
            ));
        Err(VMError::External("YIELD".to_string()))
    }
}
