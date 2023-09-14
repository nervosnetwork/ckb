use crate::cost_model::transferred_byte_cycles;
use crate::syscalls::utils::load_bytes;
use crate::syscalls::SET_CONTENT;
use ckb_vm::{
    registers::{A0, A1, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub struct SetContent {
    content: Arc<Mutex<Vec<u8>>>,
    content_size: u64,
}

impl SetContent {
    pub fn new(content: Arc<Mutex<Vec<u8>>>, content_size: u64) -> Self {
        Self {
            content,
            content_size,
        }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for SetContent {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != SET_CONTENT {
            return Ok(false);
        }
        let content_addr = machine.registers()[A0].to_u64();
        let request_size_addr = machine.registers()[A1].to_u64();
        let request_size = machine
            .memory_mut()
            .load64(&Mac::REG::from_u64(request_size_addr))?;
        let size = std::cmp::min(self.content_size, request_size.to_u64());
        self.content.lock().unwrap().resize(size as usize, 0);
        let content = load_bytes(machine, content_addr, size)?;
        self.content.lock().unwrap().copy_from_slice(&content);
        machine.memory_mut().store64(
            &Mac::REG::from_u64(request_size_addr),
            &Mac::REG::from_u64(size),
        )?;
        machine.add_cycles_no_checking(transferred_byte_cycles(size))?;
        machine.set_register(A0, Mac::REG::from_u64(0));
        Ok(true)
    }
}
