use crate::syscalls::{INVALID_FD, SPAWN_YIELD_CYCLES_BASE, WRITE};
use crate::types::{Fd, FdArgs, Message, VmContext, VmId};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
use ckb_vm::{
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
    registers::{A0, A1, A2, A7},
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct Write {
    id: VmId,
    message_box: Arc<Mutex<Vec<Message>>>,
}

impl Write {
    pub fn new<DL>(vm_id: &VmId, vm_context: &VmContext<DL>) -> Self
    where
        DL: CellDataProvider + HeaderProvider + ExtensionProvider + Send + Sync + Clone + 'static,
    {
        Self {
            id: *vm_id,
            message_box: Arc::clone(&vm_context.message_box),
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Write {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != WRITE {
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
        // We will also leave to the actual write operation to test memory permissions.
        if !fd.is_write() {
            machine.set_register(A0, Mac::REG::from_u8(INVALID_FD));
            return Ok(true);
        }
        machine.add_cycles_no_checking(SPAWN_YIELD_CYCLES_BASE)?;
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::FdWrite(
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
