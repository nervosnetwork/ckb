use crate::syscalls::{PIPE, SPAWN_YIELD_CYCLES_BASE};
use crate::types::{Message, PipeArgs, VmContext, VmId};
use ckb_traits::{CellDataProvider, ExtensionProvider, HeaderProvider};
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

impl<Mac: SupportMachine> Syscalls<Mac> for Pipe {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != PIPE {
            return Ok(false);
        }
        let fd1_addr = machine.registers()[A0].to_u64();
        let fd2_addr = fd1_addr.wrapping_add(8);
        machine.add_cycles_no_checking(SPAWN_YIELD_CYCLES_BASE)?;
        self.message_box
            .lock()
            .map_err(|e| VMError::Unexpected(e.to_string()))?
            .push(Message::Pipe(self.id, PipeArgs { fd1_addr, fd2_addr }));
        Err(VMError::Yield)
    }
}
